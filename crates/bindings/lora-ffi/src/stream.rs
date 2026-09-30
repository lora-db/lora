//! Native row streams behind `LoraQueryStream`.
//!
//! A read-only stream holds no lock (its cursor reads a snapshot `Arc`), so
//! it lives in the handle and is pulled on whichever thread the caller uses.
//!
//! A mutating stream runs inside a hidden read-write transaction that holds
//! the database's writer lock until the stream is exhausted or freed, and
//! that lock guard must be released on the thread that took it. A C host
//! calls `lora_stream_next_json` / `lora_stream_free` from any thread (a Go
//! goroutine migrates between OS threads, a finalizer runs on its own), so a
//! mutating stream lives on a dedicated thread (an actor): the thread opens
//! the stream, pulls a row per request, and drops the stream (committing an
//! exhausted one, rolling back an unfinished one) before it exits. Once
//! open, the actor waits only for the caller's requests, never for another
//! caller thread, so a request is always answered promptly.
//!
//! Both kinds keep the database alive for as long as the stream: the
//! stream is dropped before the database `Arc` it borrows from, so
//! `lora_db_free` may run before `lora_stream_free`.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use lora_database::{
    Database as InnerDatabase, InMemoryGraph, LoraError, LoraValue, PlanShape, QueryStream,
};

use crate::json::row_to_json;

type Db = Arc<InnerDatabase<InMemoryGraph>>;

/// Why a stream could not be opened or pulled.
pub(crate) enum StreamError {
    Lora(LoraError),
    Internal(String),
}

/// One pull: the next row as JSON, `None` at the end of the stream.
pub(crate) type Pulled = Result<Option<String>, StreamError>;

/// Opaque native row stream handle.
pub struct LoraQueryStream {
    columns: Vec<String>,
    state: Mutex<Option<Native>>,
}

enum Native {
    /// A read-only stream, pulled on the caller's thread.
    Local(LocalStream),
    /// A mutating stream, pulled on its actor thread.
    Actor(StreamActor),
}

struct LocalStream {
    // Declared (and so dropped) before the database it borrows from.
    stream: QueryStream<'static>,
    _db: Db,
}

impl LoraQueryStream {
    /// Open a stream for `query`. A mutating query waits here (on its actor
    /// thread) until the writer lock is free.
    pub(crate) fn open(
        db: &Db,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Self, StreamError> {
        let shape = db.explain(query, None).map_err(StreamError::Lora)?.shape;
        let (native, columns) = match shape {
            PlanShape::ReadOnly => {
                let db = db.clone();
                // SAFETY: `LocalStream` drops the stream before `_db`, the
                // exact `Arc` it borrows from. A read-only stream holds no
                // thread-affine lock guard.
                let stream = unsafe { db.stream_with_params_owned(query, params) }
                    .map_err(StreamError::Lora)?;
                let columns = stream.columns().to_vec();
                (Native::Local(LocalStream { stream, _db: db }), columns)
            }
            PlanShape::Mutating => {
                let (actor, columns) = StreamActor::spawn(db.clone(), query.to_owned(), params)?;
                (Native::Actor(actor), columns)
            }
        };
        Ok(Self {
            columns,
            state: Mutex::new(Some(native)),
        })
    }

    /// The column names, or `None` once the stream is closed.
    pub(crate) fn columns(&self) -> Result<Option<&[String]>, StreamError> {
        let state = self.lock()?;
        Ok(state.as_ref().map(|_| self.columns.as_slice()))
    }

    /// Pull the next row. The stream closes at its end or on an error;
    /// later pulls report the end.
    pub(crate) fn next(&self) -> Pulled {
        let mut state = self.lock()?;
        let pulled = match state.as_mut() {
            None => return Ok(None),
            Some(Native::Local(local)) => pull(&mut local.stream),
            Some(Native::Actor(actor)) => actor.pull(),
        };
        if !matches!(pulled, Ok(Some(_))) {
            state.take();
        }
        pulled
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Option<Native>>, StreamError> {
        self.state
            .lock()
            .map_err(|_| StreamError::Internal("stream lock poisoned".into()))
    }
}

fn pull(stream: &mut QueryStream<'_>) -> Pulled {
    match stream.next_row() {
        Ok(Some(row)) => serde_json::to_string(&row_to_json(&row))
            .map(Some)
            .map_err(|e| StreamError::Internal(e.to_string())),
        Ok(None) => Ok(None),
        Err(e) => Err(StreamError::Lora(LoraError::from_anyhow(e))),
    }
}

/// Handle to a mutating stream's actor thread.
struct StreamActor {
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
    ) -> Result<(Self, Vec<String>), StreamError> {
        let (requests, inbox) = mpsc::channel();
        let (opened_tx, opened) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("lora-stream".into())
            .spawn(move || run(db, query, params, inbox, opened_tx))
            .map_err(|e| StreamError::Internal(format!("could not start stream thread: {e}")))?;
        let actor = Self {
            requests: Some(requests),
            thread: Some(thread),
        };
        let columns = opened
            .recv()
            .map_err(|_| StreamError::Internal("stream thread exited".into()))??;
        Ok((actor, columns))
    }

    fn pull(&self) -> Pulled {
        let closed = || StreamError::Internal("query stream thread exited".into());
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
    /// lock before this returns.
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
    opened: SyncSender<Result<Vec<String>, StreamError>>,
) {
    // SAFETY: `db` is declared before `stream`, so it is dropped after it,
    // and the stream (with its writer lock guard) never leaves this thread.
    let mut stream = match unsafe { db.stream_with_params_owned(&query, params) } {
        Ok(stream) => stream,
        Err(e) => {
            let _ = opened.send(Err(StreamError::Lora(e)));
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
    // The handle was freed: `stream` drops here, rolling back if it was
    // not exhausted.
}
