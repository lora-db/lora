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
//! mutating stream lives on a dedicated thread (an actor, a pooled thread
//! from `lora_binding_buffer::stream::WriteStream`): the thread opens the
//! stream, sends its rows in chunks, reading one chunk ahead without
//! committing, and drops the stream (committing once the caller has pulled
//! past the last row, rolling back an unfinished one) before it is done. Once
//! open, the actor waits only for the caller's requests, never for another
//! caller thread, so a request is always answered promptly.
//!
//! Whether a query streams as a write comes from a per-database
//! `ShapeCache`, so opening a stream does not re-plan it.
//!
//! Both kinds keep the database alive for as long as the stream: the
//! stream is dropped before the database `Arc` it borrows from, so
//! `lora_db_free` may run before `lora_stream_free`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lora_binding_buffer::stream::{ShapeCache, WriteStream};
use lora_database::{Database as InnerDatabase, InMemoryGraph, LoraError, LoraValue, QueryStream};

use crate::json::row_to_json;

type Db = Arc<InnerDatabase<InMemoryGraph>>;

/// Why a stream could not be opened or pulled.
pub(crate) enum StreamError {
    Lora(LoraError),
    Internal(String),
}

impl From<LoraError> for StreamError {
    fn from(e: LoraError) -> Self {
        StreamError::Lora(e)
    }
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
    Actor(WriteStream<String, StreamError>),
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
        shapes: &ShapeCache,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Self, StreamError> {
        let (native, columns) = if shapes.classify(db, query)?.mutating {
            let actor = WriteStream::open(db.clone(), query.to_owned(), params, |row| {
                serde_json::to_string(&row_to_json(&row))
                    .map_err(|e| StreamError::Internal(e.to_string()))
            })?;
            let columns = actor.columns().to_vec();
            (Native::Actor(actor), columns)
        } else {
            let db = db.clone();
            // SAFETY: `LocalStream` drops the stream before `_db`, the
            // exact `Arc` it borrows from. A read-only stream holds no
            // thread-affine lock guard.
            let stream =
                unsafe { db.stream_with_params_owned(query, params) }.map_err(StreamError::Lora)?;
            let columns = stream.columns().to_vec();
            (Native::Local(LocalStream { stream, _db: db }), columns)
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
            Some(Native::Actor(actor)) => actor.next_row(),
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
