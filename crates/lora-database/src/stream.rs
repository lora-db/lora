use std::collections::BTreeMap;
use std::mem::ManuallyDrop;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use lora_compiler::CompiledQuery;
use lora_executor::{ExecResult, LoraValue, PullExecutor, Row, RowSource};
use lora_store::InMemoryGraph;

use crate::live_store::LiveStore;
use crate::transaction::{Transaction, TxCursorLease, TxStreamOutcome};

/// Owning row stream returned by [`crate::Database::stream`] and transaction
/// streaming methods.
///
/// The cursor is fallible (`next_row()` surfaces execution errors)
/// and exposes plan-derived column names populated even for empty
/// results. The lifetime parameter `'a` is bound to the source the
/// cursor borrows from — typically the database for auto-commit
/// write streams that hold the live write guard until exhaustion or
/// drop. Read-only and transaction-bound streams need no live
/// borrow and use `'static` (the buffered variant).
pub struct QueryStream<'a> {
    columns: Vec<String>,
    inner: StreamInner<'a>,
}

enum StreamInner<'a> {
    /// Transaction-bound streaming cursor. The cursor borrows from
    /// the transaction's staged graph, which is kept alive by
    /// `lease`; finalization releases the cursor token and either
    /// clears or restores the pending statement savepoint.
    Tx {
        cursor: Option<Box<dyn RowSource + 'static>>,
        state: StreamState,
        /// Lease releases the transaction cursor token and either
        /// clears or restores the pending statement savepoint.
        lease: TxCursorLease,
    },
    /// True pull-based read-only stream. Holds a live store read
    /// lock through the cursor's lifetime and emits rows as the
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
    /// `guard.tx.inner.staged`; the live store write lock stays locked
    /// through the tx's `live` guard so no other writer races. On
    /// full exhaustion the staged graph is published and the WAL
    /// replays the buffered events; on premature drop or error the
    /// staged graph and buffer are discarded and the live store is
    /// untouched.
    ///
    /// `cursor` is a streaming `RowSource` that may apply mutations
    /// row-by-row (via `StreamingWriteCursor`) or yield from a
    /// pre-materialized buffer (via `BufferedRowSource`); see
    /// `Transaction::open_streaming_compiled_autocommit`. It is
    /// taken and dropped before the guard's commit/rollback so any
    /// borrows back into the staged graph are released first.
    AutoCommit {
        cursor: Option<Box<dyn RowSource + 'static>>,
        state: StreamState,
        guard: AutoCommitGuard<'a>,
    },
}

/// Self-referential cursor that pulls rows directly from a snapshot
/// of the live store. We hold an `Arc<InMemoryGraph>` (loaded once
/// from the database's `LiveStore` at open time) and the boxed
/// `RowSource` borrows from `&*snapshot`. Drop order — `cursor`
/// first, then `_snapshot` — guarantees the cursor never sees a
/// freed graph.
///
/// Snapshot isolation is automatic: even if a writer commits a new
/// version while this cursor is live, the cursor keeps observing the
/// graph it was opened against until it drops the Arc.
pub(crate) struct LiveCursor {
    /// SAFETY invariant: borrows from `&*_snapshot` and `&*_compiled`.
    /// Must drop before either.
    cursor: ManuallyDrop<Box<dyn RowSource + 'static>>,
    /// Pinned snapshot the cursor borrows from. Dropped after `cursor`.
    _snapshot: Arc<InMemoryGraph>,
    /// Pinned live-store owner so the database storage outlives the borrowed
    /// cursor shape, even though the cursor itself only reads from `_snapshot`.
    _store: Arc<LiveStore<InMemoryGraph>>,
    /// Keeps the compiled plan alive — operator sources hold
    /// references into it (e.g. predicate `ResolvedExpr`s). Boxed
    /// so the plan address is stable across the move into the
    /// struct.
    _compiled: Box<CompiledQuery>,
}

impl LiveCursor {
    /// Snapshot the live store and open a streaming cursor against
    /// the given compiled query. Internal helper for
    /// `Database::stream_with_params` — never expose the
    /// constructed `LiveCursor` to callers without the
    /// surrounding `QueryStream`, which makes the `'static`
    /// transmutes invisible.
    pub(crate) fn open(
        store: Arc<LiveStore<InMemoryGraph>>,
        compiled: CompiledQuery,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Self> {
        let compiled = Box::new(compiled);
        let snapshot = store.load_full();

        // SAFETY: We extend the lifetime of borrows into `&*snapshot`
        // and `&*compiled` to `'static`. This is sound because the
        // surrounding `LiveCursor` keeps:
        //   (a) the `Arc<InMemoryGraph>` alive while the cursor is
        //       alive — the graph behind it is never freed; and
        //   (b) the `Box<CompiledQuery>` alive while the cursor is
        //       alive.
        // The `Drop` impl below releases `cursor` before `_snapshot`,
        // so neither borrow can outlive its backing storage.
        let storage_ref: &'static InMemoryGraph =
            unsafe { std::mem::transmute::<&InMemoryGraph, _>(&*snapshot) };
        let compiled_ref: &'static CompiledQuery =
            unsafe { std::mem::transmute::<&CompiledQuery, _>(&*compiled) };

        let cursor = PullExecutor::new(storage_ref, params)
            .open_compiled(compiled_ref)
            .map_err(|e| anyhow!(e))?;

        Ok(Self {
            cursor: ManuallyDrop::new(cursor),
            _snapshot: snapshot,
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
        // (releases its borrow into `*_snapshot` / `*_compiled`),
        // then the rest drops via field-drop ordering. After this
        // call we never touch `cursor` again.
        unsafe {
            ManuallyDrop::drop(&mut self.cursor);
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
            StreamInner::Tx { state, .. }
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
    pub(crate) fn for_tx_cursor(
        cursor: Box<dyn RowSource + 'static>,
        columns: Vec<String>,
        lease: TxCursorLease,
    ) -> Self {
        Self {
            columns,
            inner: StreamInner::Tx {
                cursor: Some(cursor),
                state: StreamState::Active,
                lease,
            },
        }
    }

    pub(crate) fn auto_commit(
        cursor: Box<dyn RowSource + 'static>,
        columns: Vec<String>,
        guard: AutoCommitGuard<'a>,
    ) -> Self {
        Self {
            columns,
            inner: StreamInner::AutoCommit {
                cursor: Some(cursor),
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
            StreamInner::Tx {
                state,
                cursor,
                lease,
            } => match *state {
                StreamState::Errored => Err(anyhow!("query stream errored")),
                StreamState::Exhausted => Ok(None),
                StreamState::Active => {
                    let pull = match cursor.as_mut() {
                        Some(c) => c.next_row(),
                        None => {
                            *state = StreamState::Errored;
                            return Err(anyhow!("transaction cursor missing"));
                        }
                    };
                    match pull {
                        Ok(Some(row)) => Ok(Some(row)),
                        Ok(None) => {
                            cursor.take();
                            lease.finalize(TxStreamOutcome::Exhausted);
                            *state = StreamState::Exhausted;
                            Ok(None)
                        }
                        Err(e) => {
                            cursor.take();
                            lease.finalize(TxStreamOutcome::Interrupted);
                            *state = StreamState::Errored;
                            Err(anyhow!(e))
                        }
                    }
                }
            },
            StreamInner::AutoCommit {
                state,
                cursor,
                guard,
            } => match *state {
                StreamState::Errored => Err(anyhow!("query stream errored")),
                StreamState::Exhausted => Ok(None),
                StreamState::Active => {
                    let pull = match cursor.as_mut() {
                        Some(c) => c.next_row(),
                        None => {
                            *state = StreamState::Errored;
                            return Err(anyhow!("auto-commit cursor missing"));
                        }
                    };
                    match pull {
                        Ok(Some(row)) => Ok(Some(row)),
                        Ok(None) => {
                            // Drop the cursor first so its borrows
                            // into the staged graph release before
                            // commit moves staged out of inner.
                            cursor.take();
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
                        Err(e) => {
                            cursor.take();
                            guard.rollback();
                            *state = StreamState::Errored;
                            Err(anyhow!(e))
                        }
                    }
                }
            },
        }
    }

    /// True once the stream has produced its last row.
    fn is_exhausted(&self) -> bool {
        match &self.inner {
            StreamInner::Tx { state, .. }
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
            // Live and AutoCommit (now backed by a streaming cursor)
            // don't know their length until drained.
            StreamInner::Live { .. } | StreamInner::Tx { .. } | StreamInner::AutoCommit { .. } => {
                (0, None)
            }
        }
    }
}

// Note: `ExactSizeIterator` intentionally not implemented. The
// `Live` variant produces rows lazily and can't report an exact
// remaining count.

impl<'a> Drop for QueryStream<'a> {
    fn drop(&mut self) {
        let exhausted = self.is_exhausted();
        match &mut self.inner {
            StreamInner::Tx { cursor, lease, .. } => {
                cursor.take();
                let outcome = if exhausted {
                    TxStreamOutcome::Exhausted
                } else {
                    TxStreamOutcome::Interrupted
                };
                lease.finalize(outcome);
            }
            StreamInner::Live { .. } => {
                // Drop releases the cursor, then the read guard,
                // which releases the live store read lock. No
                // additional cleanup needed — live streams never
                // mutate, so there is nothing to commit or roll back.
            }
            StreamInner::AutoCommit {
                state,
                cursor,
                guard,
            } => {
                // Drop the cursor first so its borrows into the
                // staged graph release before the guard rolls back
                // (which moves staged to None).
                cursor.take();
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
            Some(tx) => {
                // The streaming auto-commit cursor sets
                // `cursor_active = true` at construction; it must
                // be cleared before `tx.commit` (which rejects on
                // an active cursor). The cursor itself was already
                // dropped by the caller in `next_row` — its
                // borrows back into staged are gone, so we can
                // safely flip the flag here. For the buffered
                // fallback path the flag was never set, so this
                // assignment is a no-op.
                tx.release_streaming_cursor();
                Ok(tx.commit()?)
            }
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
            // Clear the streaming-cursor flag before delegating to
            // tx.rollback so the rollback can finalize without
            // stumbling over a stale `cursor_active = true`.
            tx.release_streaming_cursor();
            let _ = tx.rollback();
        }
    }
}
