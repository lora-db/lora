//! Streaming entry points on [`Database<InMemoryGraph>`].
//!
//! The actual [`QueryStream`] type and its three cursor variants live
//! in [`crate::stream`]; this module owns the *opening* of those
//! streams from a `Database` — including the read/mutating shape
//! split, the hidden auto-commit transaction wrapping for mutating
//! queries, and the `unsafe` `'static`-lifetime escape hatch used by
//! language bindings.
//!
//! [`Database::begin_transaction`] also lives here because the
//! mutating-stream path uses it as the hidden auto-commit transaction
//! origin.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use lora_executor::{classify_stream, compiled_result_columns, LoraValue, StreamShape};
use lora_store::InMemoryGraph;

use crate::database::Database;
use crate::error::LoraError;
use crate::stream::{AutoCommitGuard, LiveCursor, QueryStream};
use crate::transaction::{LiveStoreGuard, Transaction, TransactionMode, WriteLease};

impl Database<InMemoryGraph> {
    /// Start an explicit transaction.
    ///
    /// Read-only transactions hold a shared read lock for their
    /// lifetime; read-write transactions hold the write lock. The
    /// staging clone is **lazy** — it only happens when a
    /// [`TransactionMode::ReadWrite`] transaction sees its first
    /// mutating statement. Materialized read-only statements run
    /// straight against the live graph; tx-bound streams may still
    /// clone so their cursors can own a stable view. ReadWrite
    /// transactions that perform only materialized reads (or commit
    /// empty) pay nothing for staging.
    pub fn begin_transaction(&self, mode: TransactionMode) -> Result<Transaction<'_>, LoraError> {
        let live = match mode {
            TransactionMode::ReadOnly => LiveStoreGuard::Read(self.store.load_full()),
            TransactionMode::ReadWrite => {
                // Acquire the writer lock — writers serialize, but we
                // do NOT clone the graph yet. Staging is lazy: the
                // working copy is built only when the first mutating
                // statement runs. This keeps a `begin_transaction →
                // commit` round trip with no mutations cheap (matches
                // the previous RwLock-based behavior).
                let lock = self
                    .writer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let snapshot = self.store.load_full();
                LiveStoreGuard::Write(WriteLease {
                    _writer_lock: lock,
                    store: self.store.clone(),
                    snapshot,
                })
            }
        };
        Ok(Transaction::new(
            live,
            self.wal.clone(),
            self.snapshots.clone(),
            mode,
        ))
    }

    /// Execute a query and return an owning row stream.
    pub fn stream(&self, query: &str) -> Result<QueryStream<'_>, LoraError> {
        self.stream_with_params(query, BTreeMap::new())
    }

    /// Execute a parameterised query and return an owning row stream.
    ///
    /// The compiled plan is classified at open time. Read-only
    /// queries run directly off the live store and yield a
    /// buffered cursor with plan-derived columns. Mutating queries
    /// are routed through a hidden read-write [`Transaction`]:
    /// full cursor exhaustion calls `tx.commit` (publishing staged
    /// changes and replaying the tx-local WAL buffer); a premature
    /// drop or any error from `next_row` calls `tx.rollback` so
    /// the live store and the WAL stay untouched.
    pub fn stream_with_params(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'_>, LoraError> {
        // Classify by fetching (or compiling once into) the plan cache. The
        // mutating branch hands the same `Arc<CompiledQuery>` straight to
        // the hidden transaction, so we no longer recompile against the
        // staged graph — and the read-only branch reuses the cached plan
        // for every subsequent stream.
        let (store_guard, store_epoch) = self.read_store_with_epoch_deadline(None)?;
        let compiled_arc = self.compile_query_cached(query, &*store_guard, store_epoch)?;
        let columns = compiled_result_columns(&compiled_arc);
        let shape = classify_stream(&compiled_arc);
        // Release the analyzer's lock before either branch
        // re-acquires (read-only path keeps it; mutating path
        // delegates to begin_transaction which takes its own).
        drop(store_guard);

        match shape {
            StreamShape::ReadOnly => {
                // True pull-shaped streaming. `LiveCursor` holds
                // the live store lock and the cursor that
                // borrows from it; its `Drop` releases them in
                // the right order so the caller observes pure
                // pull semantics with no intermediate
                // materialization.
                let compiled = (*compiled_arc).clone();
                let live = LiveCursor::open(self.store.clone(), compiled, params)?;
                Ok(QueryStream::live(live, columns))
            }
            StreamShape::Mutating => {
                // Hidden auto-commit transaction. The transaction
                // owns staging, the buffering recorder, savepoint
                // management, and the WAL replay-on-commit logic;
                // we just pick commit-on-exhaustion vs
                // rollback-on-drop based on cursor state.
                //
                // The cursor returned by `open_streaming_compiled_autocommit`
                // may be a real per-row `StreamingWriteCursor`,
                // a mutable UNION cursor, or a buffered leaf for
                // operators that still need full materialization.
                // Either way the AutoCommit guard's
                // drop/exhaustion semantics are identical. The
                // compiled plan is already wrapped in an `Arc` so
                // the cursor's `'static` borrows into it remain
                // valid for the cursor's lifetime.
                let mut tx = self.begin_transaction(TransactionMode::ReadWrite)?;
                let cursor =
                    match tx.open_streaming_compiled_autocommit(compiled_arc.clone(), params) {
                        Ok(c) => c,
                        Err(err) => {
                            // Tx rolls back implicitly on drop here.
                            return Err(err.into());
                        }
                    };
                let guard = AutoCommitGuard {
                    tx: Some(tx),
                    finalized: false,
                };
                Ok(QueryStream::auto_commit(cursor, columns, guard))
            }
        }
    }

    /// Open a stream whose lifetime can be carried by an outer owner that
    /// also retains an `Arc<Database>`.
    ///
    /// # Safety
    ///
    /// The returned stream may contain lock guards that borrow from the
    /// database's internal `RwLock`. The caller must keep this exact `Arc`
    /// alive until the stream is dropped. This is intended for language
    /// bindings that store both the `Arc<Database>` and the `QueryStream` in
    /// the same opaque stream handle.
    pub unsafe fn stream_with_params_owned(
        self: &Arc<Self>,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'static>, LoraError> {
        let stream = self.stream_with_params(query, params)?;
        Ok(std::mem::transmute::<QueryStream<'_>, QueryStream<'static>>(stream))
    }
}
