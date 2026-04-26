use std::collections::BTreeMap;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Result};
use lora_compiler::CompiledQuery;
use lora_executor::{ExecResult, LoraValue, PullExecutor, Row, RowSource};
use lora_store::InMemoryGraph;

use crate::transaction::{Transaction, TxInner};

/// Owning row stream returned by [`crate::Database::stream`] and transaction
/// streaming methods.
///
/// The cursor is fallible (`next_row()` surfaces execution errors)
/// and exposes plan-derived column names populated even for empty
/// results. The lifetime parameter `'a` is bound to the source the
/// cursor borrows from — typically the database for auto-commit
/// write streams that hold the live mutex guard until exhaustion or
/// drop. Read-only and transaction-bound streams need no live
/// borrow and use `'static` (the buffered variant).
pub struct QueryStream<'a> {
    columns: Vec<String>,
    inner: StreamInner<'a>,
}

enum StreamInner<'a> {
    /// Pre-materialized rows. Used for transaction-bound streams
    /// and as a fallback whenever the streaming pipeline cannot
    /// be exposed directly (e.g. UNION queries, mutating-read
    /// hybrids that don't fit the auto-commit pattern).
    Buffered {
        rows: std::vec::IntoIter<Row>,
        state: StreamState,
        /// Optional tx-bound cursor handle. The `Drop` impl uses it
        /// to release the tx's cursor token and signal "rollback
        /// this statement" on premature drop.
        tx_handle: Option<Arc<Mutex<TxInner>>>,
    },
    /// True pull-based read-only stream. Holds the live store
    /// mutex through the cursor's lifetime and emits rows as the
    /// caller pulls them, without any intermediate
    /// materialization. Backed by a [`LiveCursor`] which uses
    /// `self_cell` to safely co-own the lock guard and the
    /// borrowing cursor.
    Live {
        cursor: LiveCursor,
        state: StreamState,
        // The 'a parameter is unused for this variant — the
        // self-cell hides the borrow. We carry a phantom to keep
        // the enum's lifetime parameter consistent with the
        // other variants.
        _phantom: std::marker::PhantomData<&'a ()>,
    },
    /// Auto-commit write stream backed by a hidden staged
    /// transaction. The graph is mutated on a clone held in
    /// `guard.staged`; the live store mutex stays locked through
    /// `guard.live` so no other writer races. On full exhaustion
    /// the staged graph is published and the WAL replays the
    /// buffered events; on premature drop or error the staged
    /// graph and buffer are discarded and the live store is
    /// untouched.
    AutoCommit {
        rows: std::vec::IntoIter<Row>,
        state: StreamState,
        guard: AutoCommitGuard<'a>,
    },
}

/// Self-referential cursor that pulls rows directly from the live
/// store. The `Arc<Mutex<...>>` keeps the storage alive, the
/// `MutexGuard` keeps it locked, and the boxed `RowSource`
/// borrows from the locked storage. Drop releases them in
/// declaration order — `cursor` first, then `guard`, finally the
/// `Arc` — so the cursor never sees a dropped guard and the
/// guard never sees a dropped mutex.
///
/// `self_cell` can't model this because the cursor borrows from
/// the guard's deref while the guard itself borrows from the
/// owner — two levels of nested borrow inside the dependent. The
/// unsafe scope is small (one constructor, one drop) and
/// fully encapsulated; nothing outside this module can observe
/// the lifetime extension.
pub(crate) struct LiveCursor {
    /// SAFETY invariant: borrows from `*guard`. Must drop before
    /// `guard`.
    cursor: ManuallyDrop<Box<dyn RowSource + 'static>>,
    /// SAFETY invariant: borrows from the `Mutex` inside `_store`.
    /// Must drop before `_store`.
    guard: ManuallyDrop<MutexGuard<'static, InMemoryGraph>>,
    /// Keeps the underlying `Mutex` alive. Dropped after `guard`.
    _store: Arc<Mutex<InMemoryGraph>>,
    /// Keeps the compiled plan alive — operator sources hold
    /// references into it (e.g. predicate `ResolvedExpr`s). Boxed
    /// so the plan address is stable across the move into the
    /// struct.
    _compiled: Box<CompiledQuery>,
}

impl LiveCursor {
    /// Lock the live store and open a streaming cursor against
    /// the given compiled query. Internal helper for
    /// `Database::stream_with_params` — never expose the
    /// constructed `LiveCursor` to callers without the
    /// surrounding `QueryStream`, which makes the `'static`
    /// transmutes invisible.
    pub(crate) fn open(
        store: Arc<Mutex<InMemoryGraph>>,
        compiled: CompiledQuery,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Self> {
        let compiled = Box::new(compiled);

        let guard = store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // SAFETY: We extend the lifetime of the guard and the
        // borrows into `*guard` / `*compiled` to `'static`. This
        // is sound because the surrounding `LiveCursor` keeps
        // (a) the `Arc<Mutex<...>>` alive while the guard is
        // alive — the mutex behind the guard never gets freed —
        // and (b) the `Box<CompiledQuery>` alive while the
        // cursor is alive. The `Drop` impl below releases
        // `cursor` before `guard` before `_store`, so neither
        // borrow can outlive its backing storage.
        let guard: MutexGuard<'static, InMemoryGraph> = unsafe { std::mem::transmute(guard) };
        let storage_ref: &'static InMemoryGraph =
            unsafe { std::mem::transmute::<&InMemoryGraph, _>(&*guard) };
        let compiled_ref: &'static CompiledQuery =
            unsafe { std::mem::transmute::<&CompiledQuery, _>(&*compiled) };

        let cursor = PullExecutor::new(storage_ref, params)
            .open_compiled(compiled_ref)
            .map_err(|e| anyhow!(e))?;

        Ok(Self {
            cursor: ManuallyDrop::new(cursor),
            guard: ManuallyDrop::new(guard),
            _store: store,
            _compiled: compiled,
        })
    }

    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        self.cursor.next_row()
    }
}

impl Drop for LiveCursor {
    fn drop(&mut self) {
        // SAFETY: drop in the documented order — cursor first
        // (releases its borrow into `*guard`), then guard
        // (releases the mutex). After these calls we never touch
        // `cursor` or `guard` again. `_store` and `_compiled`
        // drop naturally afterwards via the normal field-drop
        // sequence.
        unsafe {
            ManuallyDrop::drop(&mut self.cursor);
            ManuallyDrop::drop(&mut self.guard);
        }
    }
}

/// Per-stream state held only by auto-commit write streams.
///
/// The auto-commit guard is a thin wrapper around an explicit
/// [`Transaction`]: full cursor exhaustion calls `commit`,
/// premature drop or error calls `rollback`. All staged-graph,
/// savepoint, and WAL replay logic lives on `Transaction` itself,
/// so the guard contributes no behavior of its own beyond the
/// commit-vs-rollback decision.
pub(crate) struct AutoCommitGuard<'a> {
    /// The hidden transaction. `None` once the guard has finalized
    /// (commit consumes the tx; rollback consumes it; both leave
    /// `None` behind).
    pub(crate) tx: Option<Transaction<'a>>,
    /// Set once a finalization (commit or rollback) has run so
    /// duplicate calls — including the `Drop` path after a
    /// successful `next_row`-driven commit — are no-ops.
    pub(crate) finalized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamState {
    Active,
    Exhausted,
    Errored,
}

impl<'a> std::fmt::Debug for QueryStream<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match &self.inner {
            StreamInner::Buffered { state, .. }
            | StreamInner::AutoCommit { state, .. }
            | StreamInner::Live { state, .. } => *state,
        };
        f.debug_struct("QueryStream")
            .field("columns", &self.columns)
            .field("state", &state)
            .finish()
    }
}

impl<'a> QueryStream<'a> {
    pub(crate) fn for_tx(
        rows: Vec<Row>,
        columns: Vec<String>,
        tx_handle: Arc<Mutex<TxInner>>,
    ) -> Self {
        Self {
            columns,
            inner: StreamInner::Buffered {
                rows: rows.into_iter(),
                state: StreamState::Active,
                tx_handle: Some(tx_handle),
            },
        }
    }

    pub(crate) fn auto_commit(
        rows: Vec<Row>,
        columns: Vec<String>,
        guard: AutoCommitGuard<'a>,
    ) -> Self {
        Self {
            columns,
            inner: StreamInner::AutoCommit {
                rows: rows.into_iter(),
                state: StreamState::Active,
                guard,
            },
        }
    }

    pub(crate) fn live(cursor: LiveCursor, columns: Vec<String>) -> Self {
        Self {
            columns,
            inner: StreamInner::Live {
                cursor,
                state: StreamState::Active,
                _phantom: std::marker::PhantomData,
            },
        }
    }

    /// Plan-derived column names. Populated even when the result is
    /// empty so callers can drive a row-arrays format off this list
    /// without first peeking at a materialized row.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Pull the next row. Returns `Ok(None)` once the cursor is
    /// exhausted, `Ok(Some(row))` for the next hydrated row, or an
    /// error if the underlying execution failed. Once an error has
    /// been observed, subsequent calls keep returning that terminal
    /// state — the cursor never tries to recover or re-execute.
    pub fn next_row(&mut self) -> Result<Option<Row>> {
        match &mut self.inner {
            StreamInner::Buffered { state, rows, .. } => match *state {
                StreamState::Errored => Err(anyhow!("query stream errored")),
                StreamState::Exhausted => Ok(None),
                StreamState::Active => match rows.next() {
                    Some(row) => Ok(Some(row)),
                    None => {
                        *state = StreamState::Exhausted;
                        Ok(None)
                    }
                },
            },
            StreamInner::Live { state, cursor, .. } => match *state {
                StreamState::Errored => Err(anyhow!("query stream errored")),
                StreamState::Exhausted => Ok(None),
                StreamState::Active => match cursor.next_row() {
                    Ok(Some(row)) => Ok(Some(row)),
                    Ok(None) => {
                        *state = StreamState::Exhausted;
                        Ok(None)
                    }
                    Err(e) => {
                        *state = StreamState::Errored;
                        Err(anyhow!(e))
                    }
                },
            },
            StreamInner::AutoCommit { state, rows, guard } => match *state {
                StreamState::Errored => Err(anyhow!("query stream errored")),
                StreamState::Exhausted => Ok(None),
                StreamState::Active => match rows.next() {
                    Some(row) => Ok(Some(row)),
                    None => {
                        // Last row already returned — commit the
                        // staged graph and replay the WAL buffer.
                        match guard.commit() {
                            Ok(()) => {
                                *state = StreamState::Exhausted;
                                Ok(None)
                            }
                            Err(e) => {
                                *state = StreamState::Errored;
                                Err(e)
                            }
                        }
                    }
                },
            },
        }
    }

    /// True once the stream has produced its last row.
    fn is_exhausted(&self) -> bool {
        match &self.inner {
            StreamInner::Buffered { state, .. }
            | StreamInner::AutoCommit { state, .. }
            | StreamInner::Live { state, .. } => matches!(state, StreamState::Exhausted),
        }
    }
}

impl<'a> Iterator for QueryStream<'a> {
    type Item = Row;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_row() {
            Ok(Some(row)) => Some(row),
            Ok(None) => None,
            Err(_) => None,
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            StreamInner::Buffered { rows, .. } | StreamInner::AutoCommit { rows, .. } => {
                rows.size_hint()
            }
            // Live cursors don't know their length until drained.
            StreamInner::Live { .. } => (0, None),
        }
    }
}

// Note: `ExactSizeIterator` intentionally not implemented. The
// `Live` variant produces rows lazily and can't report an exact
// remaining count.

impl<'a> Drop for QueryStream<'a> {
    fn drop(&mut self) {
        match &mut self.inner {
            StreamInner::Buffered { tx_handle, .. } => {
                if let Some(handle) = tx_handle.take() {
                    if let Ok(mut inner) = handle.lock() {
                        inner.cursor_active = false;
                        if self.is_exhausted() {
                            inner.pending_savepoint = None;
                            inner.cursor_dropped_dirty = false;
                        } else {
                            inner.cursor_dropped_dirty = true;
                        }
                    }
                }
            }
            StreamInner::Live { .. } => {
                // Drop releases the cursor, which releases the
                // mutex guard, which releases the live store
                // mutex. No additional cleanup needed — live
                // streams never mutate, so there is nothing to
                // commit or roll back.
            }
            StreamInner::AutoCommit { state, guard, .. } => {
                // Premature drop = rollback. Successful exhaustion
                // already finalized the guard via `commit()` in
                // `next_row`, so this path is a no-op for the
                // exhausted case.
                if !guard.finalized && !matches!(state, StreamState::Exhausted) {
                    guard.rollback();
                }
            }
        }
    }
}

impl<'a> AutoCommitGuard<'a> {
    /// Publish the staged graph as the live store. Delegates to
    /// [`Transaction::commit`] which owns the WAL replay + swap
    /// logic. Idempotent — subsequent calls are no-ops once
    /// finalized, regardless of whether the previous attempt
    /// succeeded or failed.
    fn commit(&mut self) -> Result<()> {
        if self.finalized {
            return Ok(());
        }
        // Mark finalized before consuming the tx so a commit
        // failure still prevents Drop from later trying to roll
        // back a tx that no longer exists.
        self.finalized = true;
        match self.tx.take() {
            Some(tx) => tx.commit(),
            None => Ok(()),
        }
    }

    /// Discard the staged graph. Delegates to
    /// [`Transaction::rollback`]; failures are swallowed because
    /// the rollback path runs from `Drop` and has nowhere to
    /// surface an error.
    fn rollback(&mut self) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        if let Some(tx) = self.tx.take() {
            let _ = tx.rollback();
        }
    }
}
