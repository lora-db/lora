//! Interactive transactions: `begin` → `execute`* → `commit` / `rollback`,
//! with arbitrary JS code between the statements.
//!
//! A read-write transaction holds the database's writer lock for its whole
//! lifetime, and that lock guard must be released on the thread that took
//! it. So each open transaction lives on a dedicated thread (an actor): the
//! thread opens the transaction and then serves commands sent over a
//! channel.
//!
//! No libuv worker ever waits for an actor. Each command carries a
//! completion callback that the actor calls when the command is done; the
//! N-API methods turn it into the settling of a JS promise (a napi deferred,
//! settled on the JS thread). Waiting on the pool instead deadlocked the
//! process: with more transactions waiting for the writer lock than pool
//! threads, every thread was blocked, and the transaction holding the lock
//! could never get a thread for its next statement (E-3 in Festimap's
//! brief).
//!
//! Dropping the handle closes the channel; the thread then drops the
//! transaction, which rolls it back and releases the writer lock. That is
//! how an abandoned transaction rolls back on `dispose()`, on GC of its
//! database handle, or at process exit (nothing was ever committed).

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use napi::{Error as NapiError, Status};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
    TransactionMode,
};

use crate::errors::format_lora_error;

/// Called by the actor, on its thread, with a command's outcome.
pub(crate) type Done<T> = Box<dyn FnOnce(Result<T, String>) + Send + 'static>;

enum Command {
    Execute {
        query: String,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        done: Done<QueryResult>,
    },
    ExecuteMany {
        statements: Vec<(String, BTreeMap<String, LoraValue>)>,
        deadline: Option<Instant>,
        done: Done<Vec<QueryResult>>,
    },
    Commit {
        done: Done<()>,
    },
    Rollback {
        done: Done<()>,
    },
}

impl Command {
    /// Complete a command the actor will never see: it had already stopped.
    fn fail(self) {
        match self {
            Command::Execute { done, .. } => done(Err(closed())),
            Command::ExecuteMany { done, .. } => done(Err(closed())),
            Command::Commit { done } | Command::Rollback { done } => done(Err(closed())),
        }
    }
}

/// Handle to one open transaction's actor thread.
pub(crate) struct TxActor {
    commands: Mutex<Option<Sender<Command>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for TxActor {
    /// Close the channel and wait for the actor to finish. An unfinished
    /// transaction is rolled back and its writer lock (and database
    /// handle) released before this returns, so a `dispose()` followed by
    /// a re-open of the same directory never races the rollback.
    fn drop(&mut self) {
        self.close();
        let thread = self.thread.get_mut().ok().and_then(Option::take);
        if let Some(thread) = thread {
            // Never join from the actor's own thread.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

impl TxActor {
    /// Spawn the actor, which opens the transaction; returns at once.
    /// `opened` is called on the actor thread when the transaction is open —
    /// for a read-write one, once the writer lock is held — or failed to open.
    pub(crate) fn spawn(
        db: Arc<InnerDatabase<InMemoryGraph>>,
        mode: TransactionMode,
        opened: Done<()>,
    ) -> Result<Self, String> {
        let (commands, inbox) = mpsc::channel::<Command>();
        let thread = std::thread::Builder::new()
            .name("lora-tx".into())
            .spawn(move || run(db, mode, inbox, opened))
            .map_err(|e| format!("LORA_INTERNAL: could not start transaction thread: {e}"))?;
        Ok(Self {
            commands: Mutex::new(Some(commands)),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Stop accepting commands. The actor rolls back once it has the
    /// transaction and finds the channel closed. `dispose()` closes every
    /// actor before joining any, so an actor still waiting for the writer
    /// lock is never joined while the one holding the lock waits for a
    /// command.
    pub(crate) fn close(&self) {
        if let Ok(mut commands) = self.commands.lock() {
            commands.take();
        }
    }

    fn send(&self, command: Command) {
        let sender = self.commands.lock().ok().and_then(|c| c.clone());
        match sender {
            Some(sender) => {
                if let Err(SendError(command)) = sender.send(command) {
                    command.fail();
                }
            }
            None => command.fail(),
        }
    }

    pub(crate) fn execute(
        &self,
        query: String,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        done: Done<QueryResult>,
    ) {
        self.send(Command::Execute {
            query,
            params,
            deadline,
            done,
        })
    }

    /// Run `statements` in order in one round trip to the actor. Stops at
    /// the first failure, which rolls the transaction back like a failed
    /// [`Self::execute`]; one `deadline` bounds the whole batch.
    pub(crate) fn execute_many(
        &self,
        statements: Vec<(String, BTreeMap<String, LoraValue>)>,
        deadline: Option<Instant>,
        done: Done<Vec<QueryResult>>,
    ) {
        self.send(Command::ExecuteMany {
            statements,
            deadline,
            done,
        })
    }

    pub(crate) fn commit(&self, done: Done<()>) {
        self.send(Command::Commit { done })
    }

    pub(crate) fn rollback(&self, done: Done<()>) {
        self.send(Command::Rollback { done })
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
    opened: Done<()>,
) {
    // SAFETY: `db` lives on this thread's stack for as long as `tx`
    // does (it is declared first, so it is dropped last), and the
    // transaction never leaves this thread.
    let tx = unsafe { db.begin_transaction_owned(mode) };
    let mut tx = match tx {
        Ok(tx) => {
            opened(Ok(()));
            tx
        }
        Err(e) => {
            opened(Err(format_lora_error(&e)));
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
                done,
            } => {
                let result = match deadline {
                    Some(deadline) => {
                        tx.execute_with_params_deadline(&query, options, params, deadline)
                    }
                    None => tx.execute_with_params(&query, options, params),
                };
                match result {
                    Ok(result) => done(Ok(result)),
                    Err(e) => {
                        // A failed statement poisons the transaction, as in
                        // `transaction()`: roll back instead of letting later
                        // statements build on a partial state. The writer
                        // lock is released before the caller hears of it.
                        let message = format_lora_error(&e);
                        let _ = tx.rollback();
                        done(Err(message));
                        return;
                    }
                }
            }
            Command::ExecuteMany {
                statements,
                deadline,
                done,
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
                    None => done(Ok(results)),
                    Some(message) => {
                        // Same as a failed `Execute`: the transaction is
                        // rolled back and closed.
                        let _ = tx.rollback();
                        done(Err(message));
                        return;
                    }
                }
            }
            Command::Commit { done } => {
                let result = tx.commit().map_err(|e| format_lora_error(&e));
                done(result);
                return;
            }
            Command::Rollback { done } => {
                let result = tx.rollback().map_err(|e| format_lora_error(&e));
                done(result);
                return;
            }
        }
    }
}

pub(crate) fn napi_err(message: String) -> NapiError {
    NapiError::new(Status::GenericFailure, message)
}
