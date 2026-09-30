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
//! write). So a mutating stream lives on a dedicated thread (an actor, a
//! pooled thread from `lora_binding_buffer::stream::WriteStream`): the
//! thread opens the stream, sends its rows in chunks, reading one chunk
//! ahead without committing, and drops the stream (committing once the
//! caller has pulled past the last row, rolling back an unfinished one)
//! before it is done. Once open, the actor waits only for its handle's requests and
//! never needs the GIL, so a request is always answered promptly.
//!
//! Whether a query streams as a write comes from a per-database
//! `ShapeCache`, so opening a stream does not re-plan it. A read-only
//! stream opens with the GIL held, as it never waits for a lock.
//!
//! Both kinds drop the stream before the database `Arc` it borrows from,
//! so closing (or dropping) the `Database` before the stream is safe.

use std::collections::BTreeMap;
use std::sync::Arc;

use lora_binding_buffer::stream::{OpeningStream, ShapeCache, WriteStream};
use lora_database::{
    Database as InnerDatabase, InMemoryGraph, LoraError, LoraValue, QueryStream, Row,
};
use pyo3::Python;

use crate::gil::without_gil;

type Db = Arc<InnerDatabase<InMemoryGraph>>;

/// One pull: the next row, `None` at the end of the stream.
pub(crate) type Pulled = Result<Option<Row>, PullError>;

/// Why a pull failed. Kept one word wide: a read-only stream's rows come
/// back through `Pulled` one at a time, and a wide error type made every
/// row pay for moving it.
pub(crate) enum PullError {
    Engine(anyhow::Error),
    Lora(Box<LoraError>),
}

impl From<PullError> for LoraError {
    fn from(err: PullError) -> Self {
        match err {
            PullError::Engine(e) => LoraError::from_anyhow(e),
            PullError::Lora(e) => *e,
        }
    }
}

/// An open stream of either kind.
pub(crate) enum NativeStream {
    /// A read-only stream, pulled on the caller's thread.
    Local(LocalStream),
    /// A mutating stream, pulled on its actor thread.
    Actor(WriteStream<Row, LoraError>),
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
// without a `Send` bound. The handle serializes access (pyo3's borrow
// checking under the GIL), as lora-ffi's `LocalStream` does.
unsafe impl Send for LocalStream {}

impl NativeStream {
    /// Open a stream for `query`. A mutating query waits (its actor waits
    /// for the writer lock) with the GIL released: the thread holding the
    /// lock may need the GIL to finish.
    pub(crate) fn open(
        py: Python<'_>,
        db: Db,
        shapes: &ShapeCache,
        query: String,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Self, LoraError> {
        if shapes.classify(&db, &query)?.mutating {
            let actor = without_gil(py, move || {
                WriteStream::open(db, query, params, Ok::<Row, LoraError>)
            })?;
            return Ok(NativeStream::Actor(actor));
        }
        // SAFETY: `LocalStream` drops the stream before `_db`, the exact
        // `Arc` it borrows from.
        let stream = unsafe { db.stream_with_params_owned(&query, params) }?;
        Ok(NativeStream::Local(LocalStream { stream, _db: db }))
    }

    /// Like [`Self::open`], but a mutating stream still waiting for the
    /// writer lock after a brief spin is returned as `Err`, opening; its
    /// `wait` finishes the open. Used by `AsyncDatabase.stream`, which must
    /// not block its event loop on the lock.
    pub(crate) fn open_nowait(
        py: Python<'_>,
        db: Db,
        shapes: &ShapeCache,
        query: String,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Result<Self, OpeningStream<Row, LoraError>>, LoraError> {
        if !shapes.classify(&db, &query)?.mutating {
            return Self::open(py, db, shapes, query, params).map(Ok);
        }
        let opening = WriteStream::start(db, query, params, Ok::<Row, LoraError>)?;
        match without_gil(py, move || opening.try_open()) {
            Ok(opened) => Ok(Ok(NativeStream::Actor(opened?))),
            Err(opening) => Ok(Err(opening)),
        }
    }

    pub(crate) fn columns(&self) -> &[String] {
        match self {
            NativeStream::Local(local) => local.stream.columns(),
            NativeStream::Actor(actor) => actor.columns(),
        }
    }

    /// Pull the next row. Never waits for a lock: a mutating stream's actor
    /// already holds the writer lock and needs nothing else to answer.
    #[inline]
    pub(crate) fn next_row(&mut self) -> Pulled {
        match self {
            NativeStream::Local(local) => local.stream.next_row().map_err(PullError::Engine),
            NativeStream::Actor(actor) => {
                actor.next_row().map_err(|e| PullError::Lora(Box::new(e)))
            }
        }
    }
}
