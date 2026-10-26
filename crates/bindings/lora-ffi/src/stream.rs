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
//! Rows cross the boundary as JSON one at a time (`lora_stream_next_json`)
//! or as `lora_binding_buffer` batches (`lora_stream_next_buffer`). A batch
//! of a mutating stream never extends past the rows the actor has already
//! sent, so the pull that commits (the one past the last row) is always a
//! call of its own, exactly as with single-row pulls: freeing the stream
//! after the last row but before that pull still rolls it back.
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
use lora_database::{
    Database as InnerDatabase, InMemoryGraph, LoraError, LoraValue, QueryStream, Row,
};

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

/// One batch pull: up to `max_rows` rows as a `lora_binding_buffer`
/// payload, `None` at the end of the stream.
pub(crate) type PulledBatch = Result<Option<Vec<u8>>, StreamError>;

/// Opaque native row stream handle.
pub struct LoraQueryStream {
    columns: Vec<String>,
    state: Mutex<State>,
}

struct State {
    /// `None` once the stream is closed.
    native: Option<Native>,
    /// An error that ended the stream after rows that were already handed
    /// out in the same batch; reported by the next pull.
    pending: Option<StreamError>,
}

enum Native {
    /// A read-only stream, pulled on the caller's thread.
    Local(LocalStream),
    /// A mutating stream, pulled on its actor thread.
    Actor(WriteStream<Row, StreamError>),
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
            let actor = WriteStream::open(db.clone(), query.to_owned(), params, Ok)?;
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
            state: Mutex::new(State {
                native: Some(native),
                pending: None,
            }),
        })
    }

    /// The column names, or `None` once the stream is closed.
    pub(crate) fn columns(&self) -> Result<Option<&[String]>, StreamError> {
        let state = self.lock()?;
        Ok(state.native.as_ref().map(|_| self.columns.as_slice()))
    }

    /// Pull the next row as JSON. The stream closes at its end or on an
    /// error; later pulls report the end.
    pub(crate) fn next(&self) -> Pulled {
        let mut state = self.lock()?;
        if let Some(e) = state.pending.take() {
            return Err(e);
        }
        let Some(native) = state.native.as_mut() else {
            return Ok(None);
        };
        match native.pull() {
            Ok(Some(row)) => serde_json::to_string(&row_to_json(&row))
                .map(Some)
                .map_err(|e| StreamError::Internal(e.to_string())),
            other => {
                state.native.take();
                other.map(|_| None)
            }
        }
    }

    /// Pull up to `max_rows` rows (at least one) as one binary batch. A
    /// mutating stream's batch stops at the rows already received from its
    /// actor (see the module docs). An error after some rows is reported by
    /// the next pull.
    pub(crate) fn next_batch(&self, max_rows: usize) -> PulledBatch {
        let max_rows = max_rows.max(1);
        let mut state = self.lock()?;
        if let Some(e) = state.pending.take() {
            return Err(e);
        }
        let Some(native) = state.native.as_mut() else {
            return Ok(None);
        };
        let mut rows = Vec::new();
        let mut ended = None;
        while rows.len() < max_rows {
            if !rows.is_empty() && !native.has_buffered_row() {
                break;
            }
            match native.pull() {
                Ok(Some(row)) => rows.push(row),
                Ok(None) => {
                    ended = Some(Ok(()));
                    break;
                }
                Err(e) => {
                    ended = Some(Err(e));
                    break;
                }
            }
        }
        if let Some(end) = ended {
            state.native.take();
            if let Err(e) = end {
                if rows.is_empty() {
                    return Err(e);
                }
                state.pending = Some(e);
            }
        }
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(lora_binding_buffer::encode_query_rows(&rows)))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, StreamError> {
        self.state
            .lock()
            .map_err(|_| StreamError::Internal("stream lock poisoned".into()))
    }
}

impl Native {
    fn pull(&mut self) -> Result<Option<Row>, StreamError> {
        match self {
            Native::Local(local) => local
                .stream
                .next_row()
                .map_err(|e| StreamError::Lora(LoraError::from_anyhow(e))),
            Native::Actor(actor) => actor.next_row(),
        }
    }

    /// Whether the next pull is answered without waiting: always for a
    /// local stream, only from already received rows for an actor.
    fn has_buffered_row(&self) -> bool {
        match self {
            Native::Local(_) => true,
            Native::Actor(actor) => actor.has_buffered_row(),
        }
    }
}
