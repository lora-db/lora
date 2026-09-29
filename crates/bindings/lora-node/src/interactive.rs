//! Interactive transactions: `begin` → `execute`* → `commit` / `rollback`,
//! with arbitrary JS code between the statements.
//!
//! A read-write transaction holds the database's writer lock for its whole
//! lifetime, and that lock guard must be released on the thread that took
//! it. N-API calls, on the other hand, run on whichever libuv worker is
//! free. So each open transaction lives on a dedicated thread (an actor):
//! the thread opens the transaction and then serves commands sent over a
//! channel. The async N-API methods only send a command and wait for the
//! reply on a libuv worker, never touching the transaction themselves.
//!
//! Dropping the handle closes the channel; the thread then drops the
//! transaction, which rolls it back and releases the writer lock. That is
//! how an abandoned transaction rolls back on `dispose()`, on GC of its
//! database handle, or at process exit (nothing was ever committed).

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::time::Instant;

use napi::{Error as NapiError, Status};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
    TransactionMode,
};

use crate::errors::format_lora_error;

type Reply<T> = SyncSender<Result<T, String>>;

enum Command {
    Execute {
        query: String,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        reply: Reply<QueryResult>,
    },
    ExecuteMany {
        statements: Vec<(String, BTreeMap<String, LoraValue>)>,
        deadline: Option<Instant>,
        reply: Reply<Vec<QueryResult>>,
    },
    Commit {
        reply: Reply<()>,
    },
    Rollback {
        reply: Reply<()>,
    },
}

/// Handle to one open transaction's actor thread.
pub(crate) struct TxActor {
    commands: Option<Sender<Command>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for TxActor {
    /// Close the channel and wait for the actor to finish. An unfinished
    /// transaction is rolled back and its writer lock (and database
    /// handle) released before this returns, so a `dispose()` followed by
    /// a re-open of the same directory never races the rollback.
    fn drop(&mut self) {
        self.commands.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl TxActor {
    /// Spawn the actor and open the transaction on it. Blocks the calling
    /// (libuv worker) thread until the transaction is open, i.e. until the
    /// writer lock is acquired for a read-write transaction.
    pub(crate) fn begin(
        db: Arc<InnerDatabase<InMemoryGraph>>,
        mode: TransactionMode,
    ) -> Result<Self, String> {
        let (commands, inbox) = mpsc::channel::<Command>();
        let (opened_tx, opened) = mpsc::sync_channel::<Result<(), String>>(1);
        let thread = std::thread::Builder::new()
            .name("lora-tx".into())
            .spawn(move || run(db, mode, inbox, opened_tx))
            .map_err(|e| format!("LORA_INTERNAL: could not start transaction thread: {e}"))?;
        let actor = Self {
            commands: Some(commands),
            thread: Some(thread),
        };
        opened
            .recv()
            .map_err(|_| "LORA_INTERNAL: transaction thread exited".to_string())??;
        Ok(actor)
    }

    fn call<T>(&self, make: impl FnOnce(Reply<T>) -> Command) -> Result<T, String> {
        let (reply, response) = mpsc::sync_channel(1);
        self.commands
            .as_ref()
            .ok_or_else(closed)?
            .send(make(reply))
            .map_err(|_| closed())?;
        response.recv().map_err(|_| closed())?
    }

    pub(crate) fn execute(
        &self,
        query: String,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<QueryResult, String> {
        self.call(|reply| Command::Execute {
            query,
            params,
            deadline,
            reply,
        })
    }

    /// Run `statements` in order in one round trip to the actor. Stops at
    /// the first failure, which rolls the transaction back like a failed
    /// [`Self::execute`]; one `deadline` bounds the whole batch.
    pub(crate) fn execute_many(
        &self,
        statements: Vec<(String, BTreeMap<String, LoraValue>)>,
        deadline: Option<Instant>,
    ) -> Result<Vec<QueryResult>, String> {
        self.call(|reply| Command::ExecuteMany {
            statements,
            deadline,
            reply,
        })
    }

    pub(crate) fn commit(&self) -> Result<(), String> {
        self.call(|reply| Command::Commit { reply })
    }

    pub(crate) fn rollback(&self) -> Result<(), String> {
        self.call(|reply| Command::Rollback { reply })
    }
}

fn closed() -> String {
    "LORA_TRANSACTION: transaction is no longer open (already committed, rolled back, or failed)"
        .to_string()
}

fn run(
    db: Arc<InnerDatabase<InMemoryGraph>>,
    mode: TransactionMode,
    inbox: Receiver<Command>,
    opened: SyncSender<Result<(), String>>,
) {
    // SAFETY: `db` lives on this thread's stack for as long as `tx`
    // does (it is declared first, so it is dropped last), and the
    // transaction never leaves this thread.
    let tx = unsafe { db.begin_transaction_owned(mode) };
    let mut tx = match tx {
        Ok(tx) => {
            let _ = opened.send(Ok(()));
            tx
        }
        Err(e) => {
            let _ = opened.send(Err(format_lora_error(&e)));
            return;
        }
    };
    let options = Some(ExecuteOptions {
        format: ResultFormat::RowArrays,
    });

    // `recv` fails once every handle is dropped: fall out of the loop and
    // drop `tx`, which rolls back.
    while let Ok(command) = inbox.recv() {
        match command {
            Command::Execute {
                query,
                params,
                deadline,
                reply,
            } => {
                let result = match deadline {
                    Some(deadline) => {
                        tx.execute_with_params_deadline(&query, options, params, deadline)
                    }
                    None => tx.execute_with_params(&query, options, params),
                };
                let failed = result.is_err();
                let _ = reply.send(result.map_err(|e| format_lora_error(&e)));
                if failed {
                    // A failed statement poisons the transaction, as in
                    // `transaction()`: roll back instead of letting later
                    // statements build on a partial state.
                    let _ = tx.rollback();
                    return;
                }
            }
            Command::ExecuteMany {
                statements,
                deadline,
                reply,
            } => {
                let total = statements.len();
                let mut results = Vec::with_capacity(total);
                let mut failure = None;
                for (index, (query, params)) in statements.into_iter().enumerate() {
                    let result = match deadline {
                        Some(deadline) => {
                            tx.execute_with_params_deadline(&query, options, params, deadline)
                        }
                        None => tx.execute_with_params(&query, options, params),
                    };
                    match result {
                        Ok(result) => results.push(result),
                        Err(e) => {
                            failure = Some(format!(
                                "{} (statement {} of {total})",
                                format_lora_error(&e),
                                index + 1
                            ));
                            break;
                        }
                    }
                }
                match failure {
                    None => {
                        let _ = reply.send(Ok(results));
                    }
                    Some(message) => {
                        let _ = reply.send(Err(message));
                        // Same as a failed `Execute`: the transaction is
                        // rolled back and closed.
                        let _ = tx.rollback();
                        return;
                    }
                }
            }
            Command::Commit { reply } => {
                let _ = reply.send(tx.commit().map_err(|e| format_lora_error(&e)));
                return;
            }
            Command::Rollback { reply } => {
                let _ = reply.send(tx.rollback().map_err(|e| format_lora_error(&e)));
                return;
            }
        }
    }
}

pub(crate) fn napi_err(message: String) -> NapiError {
    NapiError::new(Status::GenericFailure, message)
}
