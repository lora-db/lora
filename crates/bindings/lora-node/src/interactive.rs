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
//!
//! A command the actor never runs (queued behind a failed statement or a
//! commit, or sent after the handle closed) still settles its promise: its
//! [`Done`] reports the transaction closed when dropped. The JS thread
//! never waits for an actor that is opening or running a statement (see
//! [`crate::actor`]).

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use napi::{Error as NapiError, Status};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
    TransactionMode,
};

use crate::actor::{self, Lifecycle};
use crate::errors::format_lora_error;
use crate::stream::JsBoundWriter;
use crate::QueryLimit;

/// Completion callback of a transaction command; reports the transaction
/// closed if the command never runs.
pub(crate) type Done<T> = actor::Done<T>;

/// Wrap `callback` as a transaction command's [`Done`].
pub(crate) fn done<T>(callback: impl FnOnce(Result<T, String>) + Send + 'static) -> Done<T> {
    actor::Done::new(closed, callback)
}

enum Command {
    Execute {
        query: String,
        params: BTreeMap<String, LoraValue>,
        limit: QueryLimit,
        done: Done<QueryResult>,
    },
    ExecuteMany {
        statements: Vec<(String, BTreeMap<String, LoraValue>)>,
        limit: QueryLimit,
        done: Done<Vec<QueryResult>>,
    },
    Commit {
        done: Done<()>,
    },
    Rollback {
        done: Done<()>,
    },
}

/// Handle to one open transaction's actor thread.
pub(crate) struct TxActor {
    commands: Mutex<Option<Sender<Command>>>,
    /// Settles `begin()`; taken by the actor once the transaction is open,
    /// or by [`Self::close`] (rejecting it) if that comes first.
    opened: Arc<Mutex<Option<Done<()>>>>,
    lifecycle: Lifecycle,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for TxActor {
    /// Close the actor. An idle one is joined, so its transaction is rolled
    /// back and its writer lock (and database handle) released before this
    /// returns, and a `dispose()` followed by a re-open of the same
    /// directory never races the rollback. One still opening (waiting for
    /// the writer lock) or running a statement is not joined: the JS thread
    /// must not wait for it (the lock holder may need the JS thread for its
    /// next command). It rolls back and exits by itself.
    fn drop(&mut self) {
        self.close();
        let thread = self.thread.get_mut().ok().and_then(Option::take);
        actor::finish(&self.lifecycle, thread);
    }
}

impl TxActor {
    /// Spawn the actor, which opens the transaction; returns at once.
    /// `opened` is called when the transaction is open (for a read-write
    /// one, once the writer lock is held), failed to open, or was closed
    /// before it opened.
    pub(crate) fn spawn(
        db: Arc<InnerDatabase<InMemoryGraph>>,
        mode: TransactionMode,
        opened: Done<()>,
    ) -> Self {
        let (commands, inbox) = mpsc::channel::<Command>();
        let opened = Arc::new(Mutex::new(Some(opened)));
        let lifecycle = Lifecycle::new();
        // A read-write transaction needs the JS thread to release the
        // writer lock; registering keeps a mutating stream from waiting for
        // the lock on the JS thread meanwhile (see `crate::stream`).
        let writer = matches!(mode, TransactionMode::ReadWrite).then(JsBoundWriter::register);
        let thread = {
            let opened = opened.clone();
            let lifecycle = lifecycle.clone();
            std::thread::Builder::new()
                .name("lora-tx".into())
                .spawn(move || {
                    // Released once `run` has dropped the transaction.
                    let _writer = writer;
                    run(db, mode, inbox, opened, lifecycle)
                })
        };
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(e) => {
                // Nothing will open the transaction: settle `begin()` now.
                lifecycle.ending();
                if let Some(opened) = take(&opened) {
                    opened.call(Err(format!(
                        "LORA_INTERNAL: could not start transaction thread: {e}"
                    )));
                }
                None
            }
        };
        Self {
            commands: Mutex::new(Some(commands)),
            opened,
            lifecycle,
            thread: Mutex::new(thread),
        }
    }

    /// Stop accepting commands. Commands already queued are not run: they
    /// and a pending `begin()` settle with the "closed" error. The actor
    /// rolls back once it has the transaction and exits.
    pub(crate) fn close(&self) {
        self.lifecycle.close();
        if let Ok(mut commands) = self.commands.lock() {
            commands.take();
        }
        drop(take(&self.opened));
    }

    fn send(&self, command: Command) {
        let sender = self.commands.lock().ok().and_then(|c| c.clone());
        if let Some(sender) = sender {
            if let Err(SendError(command)) = sender.send(command) {
                // The actor has stopped; dropping the command settles it.
                drop(command);
            }
        }
        // No sender: the command drops here and settles as closed.
    }

    /// Run one statement. `limit`'s deadline is computed when the statement
    /// starts, so time queued behind other commands does not count.
    pub(crate) fn execute(
        &self,
        query: String,
        params: BTreeMap<String, LoraValue>,
        limit: QueryLimit,
        done: Done<QueryResult>,
    ) {
        self.send(Command::Execute {
            query,
            params,
            limit,
            done,
        })
    }

    /// Run `statements` in order in one round trip to the actor. Stops at
    /// the first failure, which rolls the transaction back like a failed
    /// [`Self::execute`]; one deadline, computed when the batch starts,
    /// bounds the whole batch.
    pub(crate) fn execute_many(
        &self,
        statements: Vec<(String, BTreeMap<String, LoraValue>)>,
        limit: QueryLimit,
        done: Done<Vec<QueryResult>>,
    ) {
        self.send(Command::ExecuteMany {
            statements,
            limit,
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

fn take(opened: &Mutex<Option<Done<()>>>) -> Option<Done<()>> {
    opened.lock().unwrap_or_else(|p| p.into_inner()).take()
}

pub(crate) fn closed() -> String {
    "LORA_TRANSACTION: transaction is no longer open (already committed, rolled back, or failed)"
        .to_string()
}

fn run(
    db: Arc<InnerDatabase<InMemoryGraph>>,
    mode: TransactionMode,
    inbox: Receiver<Command>,
    opened: Arc<Mutex<Option<Done<()>>>>,
    lifecycle: Lifecycle,
) {
    // SAFETY: `db` lives on this thread's stack for as long as `tx`
    // does (it is declared first, so it is dropped last), and the
    // transaction never leaves this thread.
    let tx = unsafe { db.begin_transaction_owned(mode) };
    let opened = take(&opened);
    let mut tx = match tx {
        Ok(tx) => {
            if opened.is_none() || !lifecycle.opened() {
                // Closed while waiting for the writer lock: `tx` rolls back
                // here and `opened`, if still ours, reports the closure.
                lifecycle.ending();
                drop(tx);
                drop(opened);
                return;
            }
            if let Some(opened) = opened {
                opened.call(Ok(()));
            }
            tx
        }
        Err(e) => {
            lifecycle.ending();
            if let Some(opened) = opened {
                opened.call(Err(format_lora_error(&e)));
            }
            return;
        }
    };
    let options = Some(ExecuteOptions {
        format: ResultFormat::RowArrays,
    });

    // `recv` fails once the handle is dropped: fall out of the loop and
    // drop `tx`, which rolls back. Returning drops `inbox`, and with it any
    // queued command, which then settles as closed.
    while let Ok(command) = inbox.recv() {
        if !lifecycle.begin_command() {
            // Closed: `command` settles as closed; `tx` rolls back.
            break;
        }
        match command {
            Command::Execute {
                query,
                params,
                limit,
                done,
            } => {
                let result = match limit.deadline() {
                    Some(deadline) => {
                        tx.execute_with_params_deadline(&query, options, params, deadline)
                    }
                    None => tx.execute_with_params(&query, options, params),
                };
                match result {
                    Ok(result) => done.call(Ok(result)),
                    Err(e) => {
                        // A failed statement poisons the transaction, as in
                        // `transaction()`: roll back instead of letting later
                        // statements build on a partial state. The writer
                        // lock is released before the caller hears of it.
                        let message = format_lora_error(&e);
                        let _ = tx.rollback();
                        lifecycle.ending();
                        done.call(Err(message));
                        return;
                    }
                }
            }
            Command::ExecuteMany {
                statements,
                limit,
                done,
            } => {
                let deadline = limit.deadline();
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
                    None => done.call(Ok(results)),
                    Some(message) => {
                        // Same as a failed `Execute`: the transaction is
                        // rolled back and closed.
                        let _ = tx.rollback();
                        lifecycle.ending();
                        done.call(Err(message));
                        return;
                    }
                }
            }
            Command::Commit { done } => {
                let result = tx.commit().map_err(|e| format_lora_error(&e));
                lifecycle.ending();
                done.call(result);
                return;
            }
            Command::Rollback { done } => {
                let result = tx.rollback().map_err(|e| format_lora_error(&e));
                lifecycle.ending();
                done.call(result);
                return;
            }
        }
        lifecycle.idle();
    }
}

pub(crate) fn napi_err(message: String) -> NapiError {
    NapiError::new(Status::GenericFailure, message)
}
