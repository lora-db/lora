use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lora_analyzer::Analyzer;
use lora_ast::Document;
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    classify_stream, compiled_result_columns, project_rows, ExecuteOptions, LoraValue,
    MutableExecutionContext, MutableExecutor, QueryResult, Row, StreamShape,
};
use lora_parser::parse_query;
use lora_store::{
    GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent, MutationRecorder, SnapshotMeta,
    Snapshotable,
};
use lora_wal::{replay_dir, Lsn, Wal, WalConfig, WalRecorder, WroteCommit};

use crate::stream::{AutoCommitGuard, LiveCursor, QueryStream};
use crate::transaction::{LiveStoreGuard, Transaction, TransactionMode};

/// Minimal abstraction any transport can depend on to run Lora queries.
pub trait QueryRunner: Send + Sync + 'static {
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult>;
}

/// Owns the graph store and orchestrates parse → analyze → compile → execute.
///
/// Optionally drives a write-ahead log: when constructed via
/// [`Database::open_with_wal`] or [`Database::recover`] the database
/// holds an [`Arc<WalRecorder>`] that brackets every query with
/// `begin → mutations → commit/abort → flush` while the store write
/// lock is held, so the WAL order is exactly the in-memory commit order.
/// When constructed via [`Database::in_memory`] / [`Database::from_graph`]
/// the WAL handle is `None` and the engine pays only the existing
/// `MutationRecorder::record` null-pointer check per mutation.
pub struct Database<S> {
    pub(crate) store: Arc<RwLock<S>>,
    pub(crate) wal: Option<Arc<WalRecorder>>,
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: a fresh, empty in-memory graph database.
    pub fn in_memory() -> Self {
        Self::from_graph(InMemoryGraph::new())
    }

    /// Open or create a WAL-enabled in-memory database from a fresh
    /// graph.
    ///
    /// `WalConfig::Disabled` falls back to [`Database::in_memory`].
    /// Otherwise, opens the WAL directory, replays any committed
    /// events into a fresh graph, installs a [`WalRecorder`] on the
    /// graph, and returns a database ready to serve queries.
    ///
    /// To restore from a snapshot in addition to the WAL, use
    /// [`Database::recover`] instead.
    pub fn open_with_wal(wal_config: WalConfig) -> Result<Self> {
        match wal_config {
            WalConfig::Disabled => Ok(Self::in_memory()),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                let mut graph = InMemoryGraph::new();
                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, Lsn::ZERO)?;
                replay_into(&mut graph, events)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
                Ok(Self {
                    store: Arc::new(RwLock::new(graph)),
                    wal: Some(recorder),
                })
            }
        }
    }

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
    pub fn begin_transaction(&self, mode: TransactionMode) -> Result<Transaction<'_>> {
        let live = match mode {
            TransactionMode::ReadOnly => LiveStoreGuard::Read(self.read_store()),
            TransactionMode::ReadWrite => LiveStoreGuard::Write(self.write_store()),
        };
        Ok(Transaction::new(live, self.wal.clone(), mode))
    }

    /// Restore from a snapshot file then replay any WAL records past
    /// it.
    ///
    /// The snapshot's `wal_lsn` (when set) becomes the replay fence —
    /// events at or below that LSN are already represented in the
    /// loaded snapshot and are skipped. A missing snapshot file is
    /// treated as "fresh start" so operators can pass the same path
    /// on every boot.
    ///
    /// If the WAL contains a checkpoint marker newer than the
    /// snapshot's `wal_lsn`, a one-line warning is printed to stderr
    /// — the snapshot is stale relative to a more recent checkpoint
    /// the operator is presumably aware of. Recovery still proceeds
    /// from the snapshot's fence (replay re-applies every record
    /// above it, which is conservative-correct); a tighter contract
    /// is deferred to v2 because verifying that the marker's
    /// snapshot file actually exists and is loadable is a separate
    /// observability concern.
    pub fn recover(snapshot_path: impl AsRef<Path>, wal_config: WalConfig) -> Result<Self> {
        let snapshot_path = snapshot_path.as_ref();
        let mut graph = InMemoryGraph::new();
        let snapshot_lsn = match File::open(snapshot_path) {
            Ok(f) => {
                let reader = BufReader::new(f);
                let meta = graph.load_snapshot(reader)?;
                meta.wal_lsn.map(Lsn::new).unwrap_or(Lsn::ZERO)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lsn::ZERO,
            Err(e) => return Err(e.into()),
        };

        match wal_config {
            WalConfig::Disabled => Ok(Self::from_graph(graph)),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                // Diagnostic peek at the WAL's newest checkpoint
                // marker so we can warn the operator about a stale
                // snapshot before we start replaying. Treat any error
                // as "no marker" — the subsequent `Wal::open` will
                // surface the real failure if there is one.
                if dir.exists() {
                    if let Ok(outcome) = replay_dir(&dir, Lsn::ZERO) {
                        if let Some(marker) = outcome.checkpoint_lsn_observed {
                            if marker > snapshot_lsn {
                                eprintln!(
                                    "lora-wal: snapshot at LSN {} is older than the newest \
                                     checkpoint marker on disk (LSN {}). Replaying every WAL \
                                     record above LSN {}; consider passing the more recent \
                                     snapshot to --restore-from.",
                                    snapshot_lsn.raw(),
                                    marker.raw(),
                                    snapshot_lsn.raw()
                                );
                            }
                        }
                    }
                }

                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, snapshot_lsn)?;
                replay_into(&mut graph, events)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
                Ok(Self {
                    store: Arc::new(RwLock::new(graph)),
                    wal: Some(recorder),
                })
            }
        }
    }

    /// Execute a query and return an owning row stream.
    pub fn stream(&self, query: &str) -> Result<QueryStream<'_>> {
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
    ) -> Result<QueryStream<'_>> {
        // Classify by compiling once against the live store. The
        // mutating branch then re-compiles inside the hidden
        // transaction (against a staged graph that's identical to
        // live at clone time, so the second plan matches the
        // first); the cost is one extra parse+analyze+compile per
        // mutating stream, paid in exchange for a tiny
        // classify-stream surface.
        let document = parse_query(query)?;
        let store_guard = self.read_store();
        let resolved = {
            let mut analyzer = Analyzer::new(&*store_guard);
            analyzer.analyze(&document)?
        };
        let compiled = Compiler::compile(&resolved);
        let columns = compiled_result_columns(&compiled);
        let shape = classify_stream(&compiled);
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
                // compiled plan is wrapped in an `Arc` so the
                // cursor's `'static` borrows into it remain valid
                // for the cursor's lifetime.
                let mut tx = self.begin_transaction(TransactionMode::ReadWrite)?;
                let compiled_arc = Arc::new(compiled);
                let cursor =
                    match tx.open_streaming_compiled_autocommit(compiled_arc.clone(), params) {
                        Ok(c) => c,
                        Err(err) => {
                            // Tx rolls back implicitly on drop here.
                            return Err(err);
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
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut,
{
    /// Build a database from a pre-wrapped, shared store.
    pub fn new(store: Arc<RwLock<S>>) -> Self {
        Self { store, wal: None }
    }

    /// Build a database by taking ownership of a bare graph store.
    pub fn from_graph(graph: S) -> Self {
        Self::new(Arc::new(RwLock::new(graph)))
    }

    /// Handle to the installed WAL recorder, if any. Exposed for
    /// admin paths (checkpoint, truncate, observability) that need
    /// to drive the WAL outside the standard query lifecycle.
    pub fn wal(&self) -> Option<&Arc<WalRecorder>> {
        self.wal.as_ref()
    }

    /// Handle to the underlying shared store — useful for callers that need
    /// to snapshot or share the graph across multiple databases.
    pub fn store(&self) -> &Arc<RwLock<S>> {
        &self.store
    }

    /// Parse a query string into an AST without executing it.
    pub fn parse(&self, query: &str) -> Result<Document> {
        Ok(parse_query(query)?)
    }

    pub(crate) fn read_store(&self) -> RwLockReadGuard<'_, S> {
        self.store
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn write_store(&self) -> RwLockWriteGuard<'_, S> {
        self.store
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn read_store_deadline(&self, deadline: Option<Instant>) -> Result<RwLockReadGuard<'_, S>> {
        let Some(deadline) = deadline else {
            return Ok(self.read_store());
        };

        loop {
            match self.store.try_read() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(anyhow!("query deadline exceeded"));
                }
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn write_store_deadline(&self, deadline: Option<Instant>) -> Result<RwLockWriteGuard<'_, S>> {
        let Some(deadline) = deadline else {
            return Ok(self.write_store());
        };

        loop {
            match self.store.try_write() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(anyhow!("query deadline exceeded"));
                }
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    fn compile_document_against(&self, document: &Document, store: &S) -> Result<CompiledQuery> {
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(document)?
        };

        Ok(Compiler::compile(&resolved))
    }

    /// Execute a query and return its result.
    pub fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        self.execute_with_params(query, options, BTreeMap::new())
    }

    /// Execute a query with a cooperative deadline. The timeout is checked at
    /// executor operator boundaries and hot scan loops; if it fires, the query
    /// returns an error and any WAL-backed mutating query is aborted through
    /// the existing failure path.
    pub fn execute_with_timeout(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        timeout: Duration,
    ) -> Result<QueryResult> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let rows =
            self.execute_rows_with_params_deadline(query, BTreeMap::new(), Some(deadline))?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a query with bound parameters.
    ///
    /// When a WAL is attached the call is bracketed by a transaction:
    ///
    /// 1. `recorder.arm()` after analyze + compile (so a parse /
    ///    semantic / compile error never opens a tx that has to be
    ///    immediately aborted). Arming is *cheap*: no record is
    ///    appended to the WAL yet, so a pure read query that
    ///    completes here pays nothing for the WAL hot path.
    /// 2. The executor runs; every primitive mutation fires
    ///    `MutationRecorder::record`, which on its first call
    ///    lazily issues `Wal::begin` and from then on forwards
    ///    every event to `Wal::append`.
    /// 3. On Ok, `recorder.commit()` writes a `TxCommit` only when a
    ///    `TxBegin` was actually allocated; the surrounding
    ///    `recorder.flush()` runs only in that case so a read-only
    ///    query never pays an `fsync`.
    /// 4. On Err, `recorder.abort()` marks the (lazily-issued) tx
    ///    for replay-time discard; if no `TxBegin` was issued,
    ///    abort is a no-op on the WAL. The engine has no rollback,
    ///    so the in-memory state may already be partially mutated;
    ///    the abort marker is what gives the *durable* layer
    ///    per-query atomicity.
    /// 5. The recorder's poisoned flag is polled once (it also
    ///    surfaces background-flusher fsync failures from
    ///    `SyncMode::Group`). If set, the query fails loudly with the
    ///    durability error so the caller can act on it; the WAL
    ///    refuses further appends until the operator restarts the
    ///    database, which recovers from the last consistent
    ///    snapshot + WAL.
    pub fn execute_with_params(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult> {
        let rows = self.execute_rows_with_params_deadline(query, params, None)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a parameterised query with a cooperative deadline.
    pub fn execute_with_params_timeout(
        &self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
        timeout: Duration,
    ) -> Result<QueryResult> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        let rows = self.execute_rows_with_params_deadline(query, params, Some(deadline))?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a query and return hydrated rows before final result-format
    /// projection.
    pub fn execute_rows(&self, query: &str) -> Result<Vec<Row>> {
        self.execute_rows_with_params(query, BTreeMap::new())
    }

    /// Execute a query with parameters and return hydrated rows before final
    /// result-format projection.
    pub fn execute_rows_with_params(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        self.execute_rows_with_params_deadline(query, params, None)
    }

    fn execute_rows_with_params_deadline(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        let document = self.parse(query)?;
        let shape = {
            let store = self.read_store_deadline(deadline)?;
            let compiled = self.compile_document_against(&document, &*store)?;

            if matches!(classify_stream(&compiled), StreamShape::ReadOnly) {
                if let Some(rec) = &self.wal {
                    if let Some(reason) = rec.poisoned() {
                        return Err(anyhow!("WAL arm failed: WAL poisoned: {reason}"));
                    }
                }
                let executor = lora_executor::Executor::with_deadline(
                    lora_executor::ExecutionContext {
                        storage: &*store,
                        params,
                    },
                    deadline,
                );
                return executor
                    .execute_compiled_rows(&compiled)
                    .map_err(|e| anyhow!(e));
            }

            classify_stream(&compiled)
        };

        debug_assert!(shape.is_mutating());

        let mut store = self.write_store_deadline(deadline)?;
        let compiled = self.compile_document_against(&document, &*store)?;

        if let Some(rec) = &self.wal {
            rec.arm().map_err(|e| anyhow!("WAL arm failed: {e}"))?;
        }

        let exec_result: Result<Vec<Row>> = (|| {
            let mut executor = MutableExecutor::with_deadline(
                MutableExecutionContext {
                    storage: &mut *store,
                    params,
                },
                deadline,
            );
            Ok(executor.execute_compiled_rows(&compiled)?)
        })();

        if let Some(rec) = &self.wal {
            match &exec_result {
                Ok(_) => match rec.commit() {
                    Ok(WroteCommit::Yes) => {
                        rec.flush().map_err(|e| anyhow!("WAL flush failed: {e}"))?;
                    }
                    Ok(WroteCommit::No) => {
                        // Read-only query: no records were written
                        // and there is nothing to fsync. Skip flush
                        // entirely so PerCommit pays zero fsyncs on
                        // pure reads.
                    }
                    Err(e) => return Err(anyhow!("WAL commit failed: {e}")),
                },
                Err(_) => {
                    // Best-effort abort. If the WAL saw mutations, durable
                    // recovery will discard them but the live in-memory store
                    // may already be ahead of durable state. Quarantine this
                    // handle so callers restart instead of serving from a
                    // potentially divergent graph.
                    if matches!(rec.abort(), Ok(true)) {
                        rec.poison(
                            "query mutated the live graph before failing; restart from snapshot + WAL required",
                        );
                    }
                }
            }
            if let Some(reason) = rec.poisoned() {
                return Err(anyhow!("WAL poisoned: {reason}"));
            }
        }

        exec_result
    }

    // ---------- Storage-agnostic utility helpers ----------
    //
    // Bindings previously reached into the shared store lock to answer
    // stat / admin calls; these helpers let them depend on `Database<S>`
    // instead, so swapping in a new backend only requires changing one type
    // parameter.

    /// Drop every node and relationship.
    ///
    /// When a WAL is attached, `clear()` is wrapped in `arm`/`commit`
    /// so the `MutationEvent::Clear` fired by the store reaches the
    /// log inside a transaction (without arming, the recorder would
    /// poison itself on the first event). WAL failures here are
    /// best-effort: the in-memory state is still cleared so the
    /// caller's contract holds, but the recorder's poisoned flag
    /// will surface to the next query.
    pub fn clear(&self) {
        let mut guard = self.write_store();
        match &self.wal {
            None => guard.clear(),
            Some(rec) => {
                let armed = rec.arm();
                guard.clear();
                if armed.is_ok() {
                    // `clear()` always emits a `MutationEvent::Clear`,
                    // so commit returns `WroteCommit::Yes` and we
                    // flush. If that order ever changes, the worst
                    // case is one redundant flush call.
                    let _ = rec.commit();
                    let _ = rec.flush();
                }
            }
        }
    }

    /// Number of nodes currently in the graph.
    pub fn node_count(&self) -> usize {
        let guard = self.read_store();
        guard.node_count()
    }

    /// Number of relationships currently in the graph.
    pub fn relationship_count(&self) -> usize {
        let guard = self.read_store();
        guard.relationship_count()
    }

    /// Run a closure with a shared borrow of the underlying store. Used by
    /// bindings to answer ad-hoc queries without locking the RwLock themselves.
    pub fn with_store<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        let guard = self.read_store();
        f(&*guard)
    }

    /// Run a closure with an exclusive borrow of the underlying store. Reserved
    /// for admin paths (restore, bulk load); regular mutation goes through
    /// `execute_with_params`.
    pub fn with_store_mut<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut guard = self.write_store();
        f(&mut *guard)
    }
}

// ---------------------------------------------------------------------------
// Snapshot helpers
//
// A second impl block so the `Snapshotable` bound only constrains backends
// that actually need it. `Database<InMemoryGraph>` picks these up
// automatically; hypothetical backends that don't implement `Snapshotable`
// still get the core query API above.
// ---------------------------------------------------------------------------

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Snapshotable,
{
    /// Serialize the current graph state to the given path. Writes are
    /// atomic: the payload goes to `<path>.tmp`, is `fsync`'d, and then
    /// renamed over the target; a torn write can never leave a half-written
    /// file at `path`. If any step before the rename fails, the stale
    /// `<path>.tmp` is removed so a crashed save never leaks scratch files.
    ///
    /// Holds a store read lock for the duration of the save so concurrent
    /// readers can proceed and writers wait behind a consistent snapshot.
    pub fn save_snapshot_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);

        // Acquire the lock once so the snapshot is point-in-time consistent.
        let guard = self.read_store();

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        // Arm cleanup immediately after `open` succeeds: every early return
        // below must either surface an error *and* unlink the tmp, or commit
        // the guard once the rename takes effect.
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);

        let meta = guard.save_snapshot(&mut writer)?;

        // Flush the BufWriter before fsync; otherwise we fsync an empty
        // underlying file.
        use std::io::Write;
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp, path)?;
        // The tmp path no longer has a file behind it — disarm the guard so
        // it doesn't try to remove the just-renamed target by name race.
        tmp_guard.commit();

        // Best-effort parent-dir fsync so the rename itself is durable on
        // power loss. Non-fatal if the parent can't be opened.
        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        Ok(meta)
    }

    /// Replace the current graph state with a snapshot loaded from `path`.
    /// Holds the store write lock for the duration of the load; concurrent
    /// queries block until restore completes.
    pub fn load_snapshot_from(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let file = File::open(path.as_ref())?;
        let reader = BufReader::new(file);

        let mut guard = self.write_store();
        Ok(guard.load_snapshot(reader)?)
    }
}

impl Database<InMemoryGraph> {
    /// Convenience constructor: open (or create) an empty in-memory database
    /// and immediately restore it from `path`. Errors if the file cannot be
    /// opened or the snapshot is malformed.
    pub fn in_memory_from_snapshot(path: impl AsRef<Path>) -> Result<Self> {
        let db = Self::in_memory();
        db.load_snapshot_from(path)?;
        Ok(db)
    }

    /// Take a checkpoint: snapshot the current state with the WAL's
    /// `durable_lsn` stamped into the header, append a `Checkpoint`
    /// marker to the WAL, then drop sealed segments at or below the
    /// fence.
    ///
    /// Errors with "checkpoint requires WAL enabled" when called on a
    /// database constructed without a WAL — operators that just want
    /// a fence-less dump should use [`save_snapshot_to`] instead.
    ///
    /// The write-lock-held window covers snapshot serialization plus the
    /// checkpoint marker append. Truncation runs after the rename
    /// but still under the write lock; making it concurrent with queries
    /// is a v2 concern (see `docs/decisions/0004-wal.md`).
    pub fn checkpoint_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("checkpoint requires WAL enabled"))?;
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);

        let guard = self.write_store();

        // Make every record appended so far durable, then capture
        // the LSN that becomes the snapshot fence.
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync before checkpoint failed: {e}"))?;
        let snapshot_lsn = recorder.wal().durable_lsn();

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);
        let meta = guard.save_checkpoint(&mut writer, snapshot_lsn.raw())?;

        use std::io::Write;
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        std::fs::rename(&tmp, path)?;
        tmp_guard.commit();

        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        // Append the checkpoint marker AFTER the rename succeeds —
        // this preserves the invariant that a `Checkpoint` record
        // in the WAL implies the snapshot it points at exists.
        recorder
            .checkpoint_marker(snapshot_lsn)
            .map_err(|e| anyhow!("WAL checkpoint marker failed: {e}"))?;
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync after checkpoint marker failed: {e}"))?;

        // Best-effort segment truncation. Failure here doesn't undo
        // the checkpoint — the next call will retry.
        let _ = recorder.truncate_up_to(snapshot_lsn);

        Ok(meta)
    }
}

fn snapshot_tmp_path(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// RAII handle that deletes its path on drop unless [`commit`] is called.
///
/// The snapshot save path creates `<target>.tmp` before the payload is
/// written; if any step between then and the final rename fails (or the
/// thread unwinds), the guard's `Drop` removes the scratch file so a crashed
/// save never leaves leftovers on disk.
///
/// [`commit`]: Self::commit
struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Disarm the guard. Call this once the tmp file's contents have been
    /// handed off (e.g. renamed to their final destination) so the `Drop`
    /// impl does not try to remove them.
    fn commit(mut self) {
        self.path.take();
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            // Best-effort: cleanup failure is not worth surfacing — the
            // worst case is a leaked scratch file that the next save
            // overwrites via `OpenOptions::truncate(true)`.
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Storage-agnostic admin surface for HTTP / binding callers that want to
/// drive snapshot operations without naming the backend type parameter.
///
/// `Database<S>` picks up a blanket impl when `S: Snapshotable + 'static`.
/// Transports (e.g. `lora-server`) type-erase on `Arc<dyn SnapshotAdmin>`.
pub trait SnapshotAdmin: Send + Sync + 'static {
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta>;
    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta>;
}

impl<S> SnapshotAdmin for Database<S>
where
    S: GraphStorage + GraphStorageMut + Snapshotable + Send + Sync + 'static,
{
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta> {
        self.save_snapshot_to(path)
    }

    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta> {
        self.load_snapshot_from(path)
    }
}

/// Storage-agnostic admin surface for the WAL.
///
/// `Database<InMemoryGraph>` picks up the blanket impl below when a
/// WAL is attached. Transports (e.g. `lora-server`) type-erase on
/// `Arc<dyn WalAdmin>` so they don't need to name the backend type
/// parameter.
///
/// All LSNs cross the trait boundary as raw `u64` so callers don't
/// need a dependency on `lora-wal`.
pub trait WalAdmin: Send + Sync + 'static {
    /// Take a checkpoint at `path`. The snapshot's header is stamped
    /// with the WAL's `durable_lsn`; older sealed segments are then
    /// dropped.
    fn checkpoint(&self, path: &Path) -> Result<SnapshotMeta>;

    /// Snapshot of the WAL's current state — durable / next LSN,
    /// active / oldest segment id. Cheap; a single WAL mutex acquisition.
    fn wal_status(&self) -> Result<WalStatus>;

    /// Drop sealed segments at or below `fence_lsn`. Idempotent.
    fn wal_truncate(&self, fence_lsn: u64) -> Result<()>;
}

/// Snapshot of WAL state returned by [`WalAdmin::wal_status`].
///
/// `bg_failure` is the latched fsync error from the background flusher
/// (only meaningful under `SyncMode::Group`). When `Some`, the WAL is
/// poisoned and every subsequent commit will fail loudly until the
/// operator restarts from the last consistent snapshot + WAL.
#[derive(Debug, Clone)]
pub struct WalStatus {
    pub durable_lsn: u64,
    pub next_lsn: u64,
    pub active_segment_id: u64,
    pub oldest_segment_id: u64,
    pub bg_failure: Option<String>,
}

impl WalAdmin for Database<InMemoryGraph> {
    fn checkpoint(&self, path: &Path) -> Result<SnapshotMeta> {
        self.checkpoint_to(path)
    }

    fn wal_status(&self) -> Result<WalStatus> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("WAL not enabled"))?;
        let wal = recorder.wal();
        Ok(WalStatus {
            durable_lsn: wal.durable_lsn().raw(),
            next_lsn: wal.next_lsn().raw(),
            active_segment_id: wal.active_segment_id(),
            oldest_segment_id: wal.oldest_segment_id(),
            bg_failure: wal.bg_failure(),
        })
    }

    fn wal_truncate(&self, fence_lsn: u64) -> Result<()> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("WAL not enabled"))?;
        recorder.truncate_up_to(Lsn::new(fence_lsn))?;
        Ok(())
    }
}

impl<S> QueryRunner for Database<S>
where
    S: GraphStorage + GraphStorageMut + Send + Sync + 'static,
{
    fn execute(&self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        Database::execute(self, query, options)
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// Apply a `MutationEvent` stream to an in-memory graph by dispatching
/// each variant to the matching store operation.
///
/// Creation events are replayed through id-preserving paths, not the
/// normal allocator-backed mutation methods. That matters after aborted
/// transactions: an aborted create can consume id `N` in the original
/// process, be dropped by replay, and leave the next committed create at
/// id `N + 1`. Reusing the regular allocator would shift ids downward.
///
/// Replay must be invoked **before** the `WalRecorder` is installed
/// on the graph. Otherwise the replay's own mutations would fire the
/// recorder and re-write the same events to the WAL, doubling them on
/// the next recovery.
fn replay_into(graph: &mut InMemoryGraph, events: Vec<MutationEvent>) -> Result<()> {
    for (idx, event) in events.into_iter().enumerate() {
        match event {
            MutationEvent::CreateNode {
                id,
                labels,
                properties,
            } => {
                graph
                    .replay_create_node(id, labels, properties)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::CreateRelationship {
                id,
                src,
                dst,
                rel_type,
                properties,
            } => {
                graph
                    .replay_create_relationship(id, src, dst, &rel_type, properties)
                    .map_err(|e| anyhow!("WAL replay failed at event {idx}: {e}"))?;
            }
            MutationEvent::SetNodeProperty {
                node_id,
                key,
                value,
            } => {
                if !graph.set_node_property(node_id, key, value) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for property set"
                    ));
                }
            }
            MutationEvent::RemoveNodeProperty { node_id, key } => {
                if !graph.remove_node_property(node_id, &key) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for property removal"
                    ));
                }
            }
            MutationEvent::AddNodeLabel { node_id, label } => {
                if !graph.add_node_label(node_id, &label) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for label add"
                    ));
                }
            }
            MutationEvent::RemoveNodeLabel { node_id, label } => {
                if !graph.remove_node_label(node_id, &label) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing node {node_id} for label removal"
                    ));
                }
            }
            MutationEvent::SetRelationshipProperty { rel_id, key, value } => {
                if !graph.set_relationship_property(rel_id, key, value) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for property set"
                    ));
                }
            }
            MutationEvent::RemoveRelationshipProperty { rel_id, key } => {
                if !graph.remove_relationship_property(rel_id, &key) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for property removal"
                    ));
                }
            }
            MutationEvent::DeleteRelationship { rel_id } => {
                if !graph.delete_relationship(rel_id) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing relationship {rel_id} for delete"
                    ));
                }
            }
            MutationEvent::DeleteNode { node_id } => {
                if !graph.delete_node(node_id) {
                    return Err(anyhow!(
                        "WAL replay failed at event {idx}: missing or attached node {node_id} for delete"
                    ));
                }
            }
            MutationEvent::DetachDeleteNode { node_id } => {
                // After the cascading DeleteRelationship +
                // DeleteNode events have already replayed, the node
                // is gone and this becomes a no-op. Calling it
                // anyway is harmless.
                graph.detach_delete_node(node_id);
            }
            MutationEvent::Clear => {
                graph.clear();
            }
        }
    }
    Ok(())
}
