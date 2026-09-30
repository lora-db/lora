//! Native row streams behind `lora_python.QueryStream`.
//!
//! A read-only stream holds no lock (its cursor reads a snapshot `Arc`), so
//! it lives in the handle and is pulled on whichever thread calls `next()`.
//!
//! A mutating stream runs inside a hidden read-write transaction that holds
//! the database's writer lock until the stream is exhausted or dropped, and
//! that lock guard must be released on the thread that took it. Python may
//! call `next()` / `close()` from any thread, and the garbage collector may
//! drop the handle on any thread (pyo3 leaked an `unsendable` object
//! dropped off its thread, and with it the writer lock, hanging every later
//! write). So a mutating stream lives on a dedicated thread (an actor): the
//! thread opens the stream, pulls a row per request, and drops the stream
//! (committing an exhausted one, rolling back an unfinished one) before it
//! exits. Once open, the actor waits only for its handle's requests and
//! never needs the GIL, so a request is always answered promptly.
//!
//! Both kinds drop the stream before the database `Arc` it borrows from,
//! so closing (or dropping) the `Database` before the stream is safe.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;

use lora_database::{
    Database as InnerDatabase, InMemoryGraph, LoraError, LoraErrorCode, LoraValue, PlanShape,
    QueryStream, Row,
};

type Db = Arc<InnerDatabase<InMemoryGraph>>;

/// One pull: the next row, `None` at the end of the stream.
pub(crate) type Pulled = Result<Option<Row>, LoraError>;

/// An open stream of either kind.
pub(crate) enum NativeStream {
    /// A read-only stream, pulled on the caller's thread.
    Local(LocalStream),
    /// A mutating stream, pulled on its actor thread.
    Actor(StreamActor),
}

pub(crate) struct LocalStream {
    // Declared (and so dropped) before the database it borrows from.
    stream: QueryStream<'static>,
    _db: Db,
}

// SAFETY: a read-only stream holds a snapshot `Arc` of the store and a
// cursor over it: no lock guard, no `Rc`, and no thread-local state kept
// between pulls (the executor's thread-locals are set and read within one
// pull). It is `!Send` only because its cursor is a `Box<dyn RowSource>`
// without a `Send` bound. The handle serializes access (the `Mutex` in
// `PyQueryStream`), as lora-ffi's `LocalStream` does.
unsafe impl Send for LocalStream {}

impl NativeStream {
    /// Open a stream for `query` and return it with its column names. A
    /// mutating query waits here (the actor waits for the writer lock)
    /// until the lock is free; call this with the GIL released.
    pub(crate) fn open(
        db: &Db,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<(Self, Vec<String>), LoraError> {
        match db.explain(query, None)?.shape {
            PlanShape::ReadOnly => {
                let db = db.clone();
                // SAFETY: `LocalStream` drops the stream before `_db`, the
                // exact `Arc` it borrows from.
                let stream = unsafe { db.stream_with_params_owned(query, params) }?;
                let columns = stream.columns().to_vec();
                Ok((
                    NativeStream::Local(LocalStream { stream, _db: db }),
                    columns,
                ))
            }
            PlanShape::Mutating => {
                let (actor, columns) = StreamActor::spawn(db.clone(), query.to_owned(), params)?;
                Ok((NativeStream::Actor(actor), columns))
            }
        }
    }

    /// Pull the next row. Never waits for a lock: a mutating stream's actor
    /// already holds the writer lock and needs nothing else to answer.
    pub(crate) fn next(&mut self) -> Pulled {
        match self {
            NativeStream::Local(local) => pull(&mut local.stream),
            NativeStream::Actor(actor) => actor.pull(),
        }
    }
}

fn pull(stream: &mut QueryStream<'_>) -> Pulled {
    stream.next_row().map_err(LoraError::from_anyhow)
}

/// Handle to a mutating stream's actor thread.
pub(crate) struct StreamActor {
    requests: Option<Sender<SyncSender<Pulled>>>,
    thread: Option<JoinHandle<()>>,
}

impl StreamActor {
    /// Spawn the actor and wait until it has opened the stream, i.e. holds
    /// the writer lock, or failed to.
    fn spawn(
        db: Db,
        query: String,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<(Self, Vec<String>), LoraError> {
        let (requests, inbox) = mpsc::channel();
        let (opened_tx, opened) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("lora-stream".into())
            .spawn(move || run(db, query, params, inbox, opened_tx))
            .map_err(|e| internal(format!("could not start stream thread: {e}")))?;
        let actor = Self {
            requests: Some(requests),
            thread: Some(thread),
        };
        let columns = opened
            .recv()
            .map_err(|_| internal("stream thread exited".into()))??;
        Ok((actor, columns))
    }

    fn pull(&self) -> Pulled {
        let closed = || internal("query stream thread exited".into());
        let (reply, response) = mpsc::sync_channel(1);
        self.requests
            .as_ref()
            .ok_or_else(closed)?
            .send(reply)
            .map_err(|_| closed())?;
        response.recv().map_err(|_| closed())?
    }
}

impl Drop for StreamActor {
    /// Close the request channel and join the actor, which drops the
    /// stream (rolling back an unfinished one) and releases the writer
    /// lock before this returns. The actor is idle (no pull is in flight
    /// while the handle is being dropped) and needs no GIL, so the join is
    /// prompt.
    fn drop(&mut self) {
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

fn run(
    db: Db,
    query: String,
    params: BTreeMap<String, LoraValue>,
    inbox: Receiver<SyncSender<Pulled>>,
    opened: SyncSender<Result<Vec<String>, LoraError>>,
) {
    // SAFETY: `db` is declared before `stream`, so it is dropped after it,
    // and the stream (with its writer lock guard) never leaves this thread.
    let mut stream = match unsafe { db.stream_with_params_owned(&query, params) } {
        Ok(stream) => stream,
        Err(e) => {
            let _ = opened.send(Err(e));
            return;
        }
    };
    if opened.send(Ok(stream.columns().to_vec())).is_err() {
        return;
    }
    while let Ok(reply) = inbox.recv() {
        let pulled = pull(&mut stream);
        if matches!(pulled, Ok(Some(_))) {
            let _ = reply.send(pulled);
        } else {
            // End or error: finish the stream (commit or roll back) and
            // release the writer lock before the caller hears of it.
            drop(stream);
            let _ = reply.send(pulled);
            return;
        }
    }
    // The handle was dropped: `stream` drops here, rolling back if it was
    // not exhausted.
}

fn internal(message: String) -> LoraError {
    LoraError::new(LoraErrorCode::Internal, message)
}
