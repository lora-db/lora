//! Native row streams behind `db.stream()`.
//!
//! A read-only stream holds no lock (its cursor reads a snapshot `Arc`), so
//! it lives in the handle and is pulled synchronously on the JS thread.
//!
//! A mutating stream runs inside a hidden read-write transaction that holds
//! the database's writer lock from the moment it opens until it is
//! exhausted or closed. Opening it on the JS thread deadlocked the process
//! whenever an interactive transaction held the lock: the JS thread waited
//! for the lock, and the transaction needed the JS thread for its next
//! command. So a mutating stream lives on a dedicated thread (an actor, see
//! [`crate::actor`]): `open` returns at once, the actor waits for the lock
//! and opens the stream, and each pull is a promise the actor settles. The
//! actor drops the stream (committing an exhausted one, rolling back an
//! unfinished one) before it exits, on the thread that took the lock.
//!
//! Both kinds drop the stream before the database `Arc` it borrows from.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use lora_database::{
    Database as InnerDatabase, InMemoryGraph, LoraError, LoraErrorCode, LoraValue, QueryStream, Row,
};

use crate::actor::{self, Lifecycle};
use crate::errors::format_lora_error;

type Db = Arc<InnerDatabase<InMemoryGraph>>;

/// Completion of one pull: the next row, `None` at the end of the stream.
pub(crate) type Pull = actor::Done<Option<Row>>;

pub(crate) fn pull_done(
    callback: impl FnOnce(Result<Option<Row>, String>) + Send + 'static,
) -> Pull {
    actor::Done::new(closed, callback)
}

pub(crate) fn closed() -> String {
    "query stream is closed".to_string()
}

pub(crate) fn timed_out() -> String {
    format_lora_error(&LoraError::new(
        LoraErrorCode::Timeout,
        "query exceeded its deadline or was cancelled",
    ))
}

/// A read-only stream, pulled on the JS thread.
pub(crate) struct LocalStream {
    // Declared (and so dropped) before the database it borrows from: Rust
    // drops fields in declaration order, and if this `Arc` were the last
    // reference, dropping the stream after it would read freed memory.
    pub(crate) stream: QueryStream<'static>,
    _db: Db,
}

impl LocalStream {
    /// Open a read-only stream on the calling thread.
    pub(crate) fn open(
        db: Db,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Self, LoraError> {
        // SAFETY: `stream` is dropped before `_db`, the exact `Arc` it
        // borrows from.
        let stream = unsafe { db.stream_with_params_owned(query, params) }?;
        Ok(Self { stream, _db: db })
    }
}

/// Handle to a mutating stream's actor thread.
pub(crate) struct StreamActor {
    requests: Option<Sender<Pull>>,
    lifecycle: Lifecycle,
    thread: Option<JoinHandle<()>>,
}

impl StreamActor {
    /// Spawn the actor, which waits for the writer lock and opens the
    /// stream; returns at once. An open failure is reported by the first
    /// pull.
    pub(crate) fn spawn(
        db: Db,
        query: String,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Self, String> {
        let (requests, inbox) = mpsc::channel();
        let lifecycle = Lifecycle::new();
        let thread = {
            let lifecycle = lifecycle.clone();
            std::thread::Builder::new()
                .name("lora-stream".into())
                .spawn(move || run(db, query, params, deadline, inbox, lifecycle))
                .map_err(|e| format!("LORA_INTERNAL: could not start stream thread: {e}"))?
        };
        Ok(Self {
            requests: Some(requests),
            lifecycle,
            thread: Some(thread),
        })
    }

    /// Ask for the next row; `done` is called on the actor thread (or with
    /// the "closed" error, if the actor has stopped).
    pub(crate) fn pull(&self, done: Pull) {
        if let Some(requests) = &self.requests {
            // A send error drops the request, which settles it as closed.
            let _ = requests.send(done);
        }
    }
}

impl Drop for StreamActor {
    /// Close the actor: joined when idle (the stream is rolled back and the
    /// writer lock released before this returns), left to finish by itself
    /// while it still waits for the writer lock or pulls a row, so the JS
    /// thread never waits for either.
    fn drop(&mut self) {
        self.lifecycle.close();
        self.requests.take();
        actor::finish(&self.lifecycle, self.thread.take());
    }
}

fn run(
    db: Db,
    query: String,
    params: BTreeMap<String, LoraValue>,
    deadline: Option<Instant>,
    inbox: Receiver<Pull>,
    lifecycle: Lifecycle,
) {
    // SAFETY: `db` is declared before `stream`, so it is dropped after it,
    // and the stream (with its writer lock guard) never leaves this thread.
    let opened = unsafe { db.stream_with_params_owned(&query, params) };
    if !lifecycle.opened() {
        // Closed while waiting for the lock: an open stream rolls back here.
        lifecycle.ending();
        return;
    }
    let mut stream = match opened {
        Ok(stream) => stream,
        Err(e) => {
            // Report the failure to the first pull.
            if let Ok(done) = inbox.recv() {
                lifecycle.ending();
                done.call(Err(format_lora_error(&e)));
            }
            return;
        }
    };
    // Returning drops `inbox`, and with it any queued pull, which then
    // settles as closed.
    while let Ok(done) = inbox.recv() {
        if !lifecycle.begin_command() {
            break;
        }
        let pulled = if deadline.is_some_and(lora_database::deadline_reached) {
            Err(timed_out())
        } else {
            stream
                .next_row()
                .map_err(|e| format_lora_error(&LoraError::from_anyhow(e)))
        };
        if matches!(pulled, Ok(Some(_))) {
            done.call(pulled);
            lifecycle.idle();
        } else {
            // End or error: finish the stream (commit or roll back) and
            // release the writer lock before the caller hears of it.
            drop(stream);
            lifecycle.ending();
            done.call(pulled);
            return;
        }
    }
    // The handle was dropped: `stream` drops here, rolling back if it was
    // not exhausted.
}
