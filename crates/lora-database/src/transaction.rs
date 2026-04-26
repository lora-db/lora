use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lora_analyzer::Analyzer;
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    classify_stream, compiled_result_columns, project_rows, ExecuteOptions, ExecutionContext,
    Executor, LoraValue, MutableExecutionContext, MutableExecutor, MutablePullExecutor,
    PullExecutor, QueryResult, Row, RowSource,
};
use lora_parser::parse_query;
use lora_store::{InMemoryGraph, MutationEvent, MutationRecorder};
use lora_wal::{WalRecorder, WroteCommit};

use crate::stream::QueryStream;

/// Transaction execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionMode {
    /// Use the read-only executor. Write operators return read-only errors.
    ReadOnly,
    /// Execute reads and writes against a staged graph, then publish on commit.
    ReadWrite,
}

pub(crate) enum LiveStoreGuard<'db> {
    Read(RwLockReadGuard<'db, InMemoryGraph>),
    Write(RwLockWriteGuard<'db, InMemoryGraph>),
}

impl LiveStoreGuard<'_> {
    fn as_graph(&self) -> &InMemoryGraph {
        match self {
            Self::Read(guard) => guard,
            Self::Write(guard) => guard,
        }
    }

    fn as_graph_mut(&mut self) -> Option<&mut InMemoryGraph> {
        match self {
            Self::Read(_) => None,
            Self::Write(guard) => Some(guard),
        }
    }
}

/// Captures the staged graph and tx-local mutation buffer at the point
/// a statement is opened, so a failed/dropped statement can be rolled
/// back to that point without affecting earlier work in the same
/// transaction.
pub(crate) struct Savepoint {
    staged: Option<InMemoryGraph>,
    buffer_len: usize,
}

/// Buffers `MutationEvent`s emitted by the staged graph while a
/// transaction is in progress. The buffer replaces direct WAL writes
/// during the transaction body; on commit the host replays the
/// buffer into the real `WalRecorder` as a single durable
/// transaction. Statement rollback truncates the buffer back to its
/// pre-statement length; transaction rollback drops it entirely.
struct BufferingRecorder {
    buffer: Arc<Mutex<Vec<MutationEvent>>>,
}

impl BufferingRecorder {
    fn new(buffer: Arc<Mutex<Vec<MutationEvent>>>) -> Self {
        Self { buffer }
    }
}

impl MutationRecorder for BufferingRecorder {
    fn record(&self, event: &MutationEvent) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push(event.clone());
        }
    }
}

/// Shared transaction state. Wrapped in `Arc<Mutex<>>` so a
/// `QueryStream` opened against the transaction can release its
/// cursor token and signal savepoint-rollback intent on drop without
/// borrowing the [`Transaction`] handle.
pub(crate) struct TxInner {
    /// The cloned staging graph. Mutated by write statements through
    /// the [`MutableExecutor`]; read by read-only statements through
    /// [`PullExecutor`]. `None` once the transaction has been closed.
    pub(crate) staged: Option<InMemoryGraph>,
    /// Tx-local mutation log, populated by the [`BufferingRecorder`]
    /// installed on `staged`. Replayed into the real WAL exactly once
    /// at commit time.
    pub(crate) buffer: Arc<Mutex<Vec<MutationEvent>>>,
    /// Per-statement savepoint snapshot. Set when a statement opens,
    /// cleared on successful completion, restored on
    /// failure/premature drop.
    pub(crate) pending_savepoint: Option<Savepoint>,
    /// True while a `QueryStream` opened against this transaction is
    /// alive. Blocks new statements and prevents commit until the
    /// cursor is released.
    pub(crate) cursor_active: bool,
    /// Set by the cursor's `Drop` impl when the cursor was released
    /// without exhausting all rows. The next transaction operation
    /// applies the pending savepoint before doing anything else.
    pub(crate) cursor_dropped_dirty: bool,
    /// True after `commit` or `rollback` has run, regardless of
    /// outcome. Subsequent operations fail loudly instead of silently
    /// running on stale state.
    pub(crate) closed: bool,
    /// Transaction execution mode chosen at `begin_transaction` time.
    pub(crate) mode: TransactionMode,
    /// Whether this transaction needs a mutation buffer for durable WAL
    /// replay. Databases without a WAL can skip recorder installation and
    /// avoid cloning mutation payloads into an unused buffer.
    pub(crate) buffer_mutations: bool,
}

/// Explicit transaction over the in-memory graph.
///
/// The implementation is conservative: read-only transactions hold a
/// database read lock, and read-write transactions hold the database
/// write lock. Read-write transactions lazily create a cloned staging
/// graph on the first mutating statement, then either swap that graph
/// into place on commit or drop it on rollback. Explicit mutating
/// statements capture a graph +
/// WAL-buffer savepoint so a failed or dropped streaming statement
/// only rolls back its own effects, not the transaction as a whole.
///
/// When a WAL is attached, mutation events fire into a tx-local
/// buffer rather than the durable log. The buffer is replayed into
/// the WAL exactly once at commit, so recovery never observes
/// partial / aborted / dropped statements.
pub struct Transaction<'db> {
    pub(crate) live: Option<LiveStoreGuard<'db>>,
    pub(crate) inner: Arc<Mutex<TxInner>>,
    pub(crate) wal: Option<Arc<WalRecorder>>,
    mode: TransactionMode,
}

impl<'db> Transaction<'db> {
    /// Build a fresh transaction. Used by `Database::begin_transaction`.
    pub(crate) fn new(
        live: LiveStoreGuard<'db>,
        wal: Option<Arc<WalRecorder>>,
        mode: TransactionMode,
    ) -> Self {
        let buffer_mutations = wal.is_some();
        let inner = TxInner {
            staged: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            pending_savepoint: None,
            cursor_active: false,
            cursor_dropped_dirty: false,
            closed: false,
            mode,
            buffer_mutations,
        };
        Self {
            live: Some(live),
            inner: Arc::new(Mutex::new(inner)),
            wal,
            mode,
        }
    }

    /// Transaction mode chosen at begin time.
    pub fn mode(&self) -> TransactionMode {
        self.mode
    }

    /// Execute a query inside the transaction and return a materialized
    /// `QueryResult`.
    pub fn execute(&mut self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        self.execute_with_params(query, options, BTreeMap::new())
    }

    /// Execute a query inside the transaction with a cooperative deadline.
    pub fn execute_with_timeout(
        &mut self,
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

    /// Execute a parameterised query inside the transaction.
    pub fn execute_with_params(
        &mut self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult> {
        let rows = self.execute_rows_with_params_deadline(query, params, None)?;
        Ok(project_rows(rows, options.unwrap_or_default()))
    }

    /// Execute a parameterised query inside the transaction with a cooperative
    /// deadline.
    pub fn execute_with_params_timeout(
        &mut self,
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

    /// Execute a query inside the transaction and return hydrated rows before
    /// final result-format projection.
    pub fn execute_rows(&mut self, query: &str) -> Result<Vec<Row>> {
        self.execute_rows_with_params(query, BTreeMap::new())
    }

    /// Execute a parameterised query inside the transaction and return hydrated
    /// rows before final result-format projection.
    pub fn execute_rows_with_params(
        &mut self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        self.execute_rows_with_params_deadline(query, params, None)
    }

    fn execute_rows_with_params_deadline(
        &mut self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        let compiled = self.compile_in_tx(query)?;
        self.execute_rows_compiled_deadline(&compiled, params, deadline)
    }

    fn execute_rows_compiled_deadline(
        &mut self,
        compiled: &CompiledQuery,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        // ReadOnly tx: never clones, runs straight against live.
        if self.is_read_only_unchecked() {
            self.precheck_open_no_savepoint()?;
            let live = self
                .live
                .as_ref()
                .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
            let storage = live.as_graph();
            let executor = Executor::with_deadline(ExecutionContext { storage, params }, deadline);
            return executor
                .execute_compiled_rows(compiled)
                .map_err(|e| anyhow!(e));
        }

        // ReadWrite tx, lazy-clone aware.
        let mut inner = self.begin_statement()?;
        let is_mutating = classify_stream(&compiled).is_mutating();

        if !is_mutating {
            // Read-only statement in a ReadWrite tx. Run against
            // staged if it has been materialized (so the read
            // sees prior in-tx writes), otherwise straight off
            // the live graph — which equals staged-as-it-would-be
            // because no writes have happened yet.
            return match inner.staged.as_ref() {
                Some(staged) => {
                    let executor = Executor::with_deadline(
                        ExecutionContext {
                            storage: staged,
                            params,
                        },
                        deadline,
                    );
                    executor
                        .execute_compiled_rows(compiled)
                        .map_err(|e| anyhow!(e))
                }
                None => {
                    drop(inner);
                    let live = self
                        .live
                        .as_ref()
                        .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
                    let storage = live.as_graph();
                    let executor =
                        Executor::with_deadline(ExecutionContext { storage, params }, deadline);
                    executor
                        .execute_compiled_rows(compiled)
                        .map_err(|e| anyhow!(e))
                }
            };
        }

        // Mutating statement: lazy-clone the live graph if this
        // is the first write in the tx, then capture a savepoint
        // and run the mutable executor.
        let clone_savepoint_graph = inner.staged.is_some();
        self.ensure_staged_locked(&mut inner)?;
        let savepoint = Some(take_savepoint(&inner, clone_savepoint_graph));

        let exec_result: ExecResultRows = {
            let staged = inner.staged_mut()?;
            let mut executor = MutableExecutor::with_deadline(
                MutableExecutionContext {
                    storage: staged,
                    params,
                },
                deadline,
            );
            executor
                .execute_compiled_rows(compiled)
                .map_err(|e| anyhow!(e))
        };

        match exec_result {
            Ok(rows) => Ok(rows),
            Err(err) => {
                restore_savepoint(&mut inner, savepoint);
                Err(err)
            }
        }
    }

    /// Open a streaming write cursor over the staged graph for a
    /// pre-compiled mutating plan, used by the hidden auto-commit
    /// stream path in `Database::stream_with_params`.
    ///
    /// The returned `Box<dyn RowSource + 'static>` may be either a
    /// real per-row [`StreamingWriteCursor`][lora_executor::StreamingWriteCursor],
    /// a mutable UNION cursor, or a [`BufferedRowSource`][lora_executor::BufferedRowSource]
    /// for the remaining materialized leaves. Either way:
    ///
    /// * The cursor mutates the *staged* graph, never the live store.
    /// * Mutations fire the [`BufferingRecorder`] installed on staged
    ///   by [`Self::ensure_staged_locked`], which accumulates into
    ///   `inner.buffer` and is replayed into the WAL on commit.
    /// * `cursor_active` is set to `true` here. The caller MUST clear
    ///   it before invoking [`Self::commit`] or [`Self::rollback`] —
    ///   the cursor itself does not.
    ///
    /// # Safety
    ///
    /// The cursor is `'static` because it owns its compiled query (via
    /// the supplied `Arc`) and aliases the staged graph through a raw
    /// pointer. Soundness depends on the invariant that
    /// `inner.staged` remains `Some(_)` at a stable address for the
    /// cursor's lifetime. That invariant holds while
    /// `cursor_active = true` blocks every other path that could
    /// move or drop staged: explicit statements (`begin_statement`
    /// rejects), `commit` and `rollback` (rejected until the caller
    /// clears `cursor_active`).
    pub(crate) fn open_streaming_compiled_autocommit(
        &mut self,
        compiled: Arc<CompiledQuery>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Box<dyn RowSource + 'static>> {
        if self.is_read_only_unchecked() {
            return Err(anyhow!(
                "streaming write cursor requires a ReadWrite transaction"
            ));
        }

        let mut inner = self.begin_statement()?;
        self.ensure_staged_locked(&mut inner)?;
        inner.cursor_active = true;

        // SAFETY: `inner.staged` is `Some` after `ensure_staged_locked`,
        // and stays at the same address while `cursor_active = true`
        // (see method-level safety note).
        let staged_ptr: *mut InMemoryGraph = inner
            .staged
            .as_mut()
            .expect("ensure_staged_locked guarantees Some")
            as *mut _;
        drop(inner);

        // SAFETY: `compiled` (Arc held by the caller / AutoCommit guard)
        // keeps the plan alive; `staged_ptr` is valid for the cursor's
        // lifetime per the invariant above. We extend both lifetimes
        // to `'static` so the resulting cursor can sit inside the
        // `'static`-shaped AutoCommit variant of `QueryStream`.
        let storage_static: &'static mut InMemoryGraph = unsafe { &mut *staged_ptr };
        let compiled_static: &'static CompiledQuery =
            unsafe { std::mem::transmute::<&CompiledQuery, _>(compiled.as_ref()) };

        // `MutablePullExecutor::open_compiled` picks the narrowest
        // cursor shape it can: per-row write cursor, branch-wise
        // mutable UNION cursor, or a buffered materialized leaf.
        let cursor = MutablePullExecutor::new(storage_static, params)
            .open_compiled(compiled_static)
            .map_err(|e| {
                // Roll back: the cursor build never happened, so the
                // tx is in a clean-but-poisoned state. Discard
                // everything and let the caller bubble the error.
                if let Ok(mut inner) = self.inner.lock() {
                    discard_transaction_state(&mut inner);
                }
                self.live.take();
                anyhow!(e)
            })?;

        // The Arc<CompiledQuery> is the safety anchor for the
        // `'static` plan reference. Keep it alive for the cursor's
        // lifetime by leaking a clone into the cursor's owned data.
        // We can't store it on the cursor itself (it's a Box<dyn>),
        // so we wrap the cursor in a guard that owns the Arc.
        Ok(Box::new(StreamingCursorWithArc {
            cursor,
            _compiled: compiled,
        }))
    }

    /// Compile a query in this transaction's view of the world:
    /// against `staged` if it has been materialized, otherwise
    /// straight against `live`. The two are equivalent before the
    /// first mutating statement, so the resulting plan is valid
    /// either way.
    fn compile_in_tx(&self, query: &str) -> Result<CompiledQuery> {
        let document = parse_query(query)?;
        let resolved = {
            let inner = self.lock_inner_unchecked();
            if let Some(staged) = &inner.staged {
                let mut analyzer = Analyzer::new(staged);
                analyzer.analyze(&document)?
            } else {
                drop(inner);
                let live = self
                    .live
                    .as_ref()
                    .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
                let mut analyzer = Analyzer::new(live.as_graph());
                analyzer.analyze(&document)?
            }
        };
        Ok(Compiler::compile(&resolved))
    }

    /// Materialize `inner.staged` if it doesn't exist yet —
    /// ReadWrite transactions defer this clone until the first
    /// mutating statement.
    fn ensure_staged_locked(&self, inner: &mut MutexGuard<'_, TxInner>) -> Result<()> {
        if inner.staged.is_some() {
            return Ok(());
        }
        let live = self
            .live
            .as_ref()
            .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
        let mut staged: InMemoryGraph = live.as_graph().clone();
        if matches!(inner.mode, TransactionMode::ReadWrite) && inner.buffer_mutations {
            staged.set_mutation_recorder(Some(
                Arc::new(BufferingRecorder::new(inner.buffer.clone())) as Arc<dyn MutationRecorder>,
            ));
        }
        inner.staged = Some(staged);
        Ok(())
    }

    /// Execute a query inside the transaction and return an owning row stream.
    pub fn stream(&mut self, query: &str) -> Result<QueryStream<'static>> {
        self.stream_with_params(query, BTreeMap::new())
    }

    /// Execute a parameterised query inside the transaction and return an
    /// owning row stream.
    pub fn stream_with_params(
        &mut self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'static>> {
        let compiled = Arc::new(self.compile_in_tx(query)?);
        let columns = compiled_result_columns(&compiled);
        self.stream_compiled(compiled, columns, params)
    }

    /// Open a tx-bound stream for an already-compiled plan. Lets
    /// `Database::stream_with_params` reuse the plan it built for
    /// classification.
    pub(crate) fn stream_compiled(
        &mut self,
        compiled: Arc<CompiledQuery>,
        columns: Vec<String>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'static>> {
        let mut inner = self.begin_statement()?;
        let is_mutating = classify_stream(&compiled).is_mutating();
        if matches!(inner.mode, TransactionMode::ReadOnly) && is_mutating {
            return Err(anyhow!(
                "cannot execute mutating query in read-only transaction"
            ));
        }

        // Transaction streams borrow from the staged graph. Even
        // read-only streams materialize staging when needed so the
        // cursor can outlive the `&mut Transaction` borrow without
        // borrowing from the transaction-owned live write guard.
        let clone_savepoint_graph = inner.staged.is_some();
        self.ensure_staged_locked(&mut inner)?;
        inner.cursor_active = true;

        let rollback_on_drop = is_mutating;
        if rollback_on_drop {
            inner.pending_savepoint = Some(take_savepoint(&inner, clone_savepoint_graph));
        } else {
            inner.pending_savepoint = None;
        }

        let staged_ptr: *mut InMemoryGraph = inner
            .staged
            .as_mut()
            .expect("ensure_staged_locked guarantees Some")
            as *mut _;
        drop(inner);

        let compiled_static: &'static CompiledQuery =
            unsafe { std::mem::transmute::<&CompiledQuery, _>(compiled.as_ref()) };
        let cursor: Result<Box<dyn RowSource + 'static>> = if is_mutating {
            let storage_static: &'static mut InMemoryGraph = unsafe { &mut *staged_ptr };
            MutablePullExecutor::new(storage_static, params)
                .open_compiled(compiled_static)
                .map(|cursor| {
                    Box::new(StreamingCursorWithArc {
                        cursor,
                        _compiled: compiled.clone(),
                    }) as Box<dyn RowSource + 'static>
                })
                .map_err(|e| anyhow!(e))
        } else {
            let storage_static: &'static InMemoryGraph = unsafe { &*staged_ptr };
            PullExecutor::new(storage_static, params)
                .open_compiled(compiled_static)
                .map(|cursor| {
                    Box::new(StreamingCursorWithArc {
                        cursor,
                        _compiled: compiled.clone(),
                    }) as Box<dyn RowSource + 'static>
                })
                .map_err(|e| anyhow!(e))
        };

        match cursor {
            Ok(cursor) => Ok(QueryStream::for_tx_cursor(
                cursor,
                columns,
                self.inner.clone(),
                rollback_on_drop,
            )),
            Err(err) => {
                finalize_tx_stream(&self.inner, false, rollback_on_drop);
                Err(err)
            }
        }
    }

    /// Commit the transaction and publish staged changes.
    ///
    /// When WAL is attached the buffered tx-local mutation log is
    /// replayed into the durable WAL as a single committed
    /// transaction; recovery therefore observes either every write
    /// in this transaction or none.
    pub fn commit(mut self) -> Result<()> {
        // Apply any pending statement rollback first (cursor was
        // dropped pre-exhaustion in a previous step). After that we
        // hold the staged graph and buffer in their final shape.
        let (staged, buffer_events, mode) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.cursor_active {
                return Err(anyhow!(
                    "cannot commit transaction while a streaming cursor is still active"
                ));
            }
            if inner.cursor_dropped_dirty {
                if let Some(sp) = inner.pending_savepoint.take() {
                    apply_savepoint(&mut inner, sp);
                }
                inner.cursor_dropped_dirty = false;
            }
            if inner.closed {
                return Err(anyhow!("transaction is already closed"));
            }
            let mode = inner.mode;
            // Both modes can have `staged = None`: ReadOnly never
            // clones, and ReadWrite tx that performed no writes
            // (or where every write was rolled back via a
            // savepoint) leaves it unmaterialized too.
            let staged = inner.staged.take();
            let buffer_events = std::mem::take(&mut *inner.buffer.lock().unwrap());
            inner.closed = true;
            (staged, buffer_events, mode)
        };

        // Replay the tx-local mutation buffer into the real WAL as
        // one committed transaction. Read-only transactions never
        // touch the WAL — `arm` is only called when there is durable
        // work to commit.
        if let Some(rec) = &self.wal {
            if matches!(mode, TransactionMode::ReadWrite) && !buffer_events.is_empty() {
                rec.arm().map_err(|e| anyhow!("WAL arm failed: {e}"))?;
                for event in &buffer_events {
                    rec.record(event);
                    if let Some(reason) = rec.poisoned() {
                        return Err(anyhow!("WAL poisoned during commit replay: {reason}"));
                    }
                }
                match rec.commit() {
                    Ok(WroteCommit::Yes) => {
                        rec.flush().map_err(|e| anyhow!("WAL flush failed: {e}"))?;
                    }
                    Ok(WroteCommit::No) => {}
                    Err(e) => return Err(anyhow!("WAL commit failed: {e}")),
                }
                if let Some(reason) = rec.poisoned() {
                    return Err(anyhow!("WAL poisoned: {reason}"));
                }
            }
        }

        if matches!(mode, TransactionMode::ReadWrite) {
            if let Some(mut staged) = staged {
                // Strip the buffering recorder from the staged graph
                // before publishing it as the live store; the live store
                // either has the durable WAL recorder reinstalled below
                // or no recorder at all (for non-WAL databases).
                staged.set_mutation_recorder(None);
                if let Some(rec) = &self.wal {
                    staged.set_mutation_recorder(Some(rec.clone() as Arc<dyn MutationRecorder>));
                }
                let live = self
                    .live
                    .as_mut()
                    .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
                let live = live
                    .as_graph_mut()
                    .ok_or_else(|| anyhow!("read-only transaction cannot publish staged graph"))?;
                *live = staged;
            }
        }

        self.live.take();
        Ok(())
    }

    /// Roll back the transaction. Staged graph changes and buffered
    /// mutations are discarded; the WAL is never armed.
    pub fn rollback(mut self) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(anyhow!("transaction is already closed"));
        }
        discard_transaction_state(&mut inner);
        drop(inner);
        self.live.take();
        Ok(())
    }

    /// Acquire the inner state for a new statement. Validates that
    /// the transaction is still open and no cursor is active, and
    /// applies any pending savepoint left behind by a dropped
    /// cursor. The staged graph is *not* required: ReadWrite
    /// transactions defer the staging clone until the first
    /// mutating statement (see [`Transaction::ensure_staged_locked`]).
    fn begin_statement(&self) -> Result<MutexGuard<'_, TxInner>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(anyhow!("transaction is already closed"));
        }
        if inner.cursor_active {
            return Err(anyhow!(
                "cannot start a new statement while a streaming cursor is still active"
            ));
        }
        if inner.cursor_dropped_dirty {
            if let Some(sp) = inner.pending_savepoint.take() {
                apply_savepoint(&mut inner, sp);
            }
            inner.cursor_dropped_dirty = false;
        }
        Ok(inner)
    }

    /// Cheap state check for the ReadOnly fast path: closed +
    /// cursor_active. No staged-graph check — ReadOnly tx has no
    /// staged graph by construction.
    fn precheck_open_no_savepoint(&self) -> Result<()> {
        let inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(anyhow!("transaction is already closed"));
        }
        if inner.cursor_active {
            return Err(anyhow!(
                "cannot start a new statement while a streaming cursor is still active"
            ));
        }
        Ok(())
    }

    /// True if the transaction was begun in `ReadOnly` mode. Cheap
    /// — `mode` doesn't change after `begin_transaction`, so we
    /// pay one small state-lock acquisition.
    fn is_read_only_unchecked(&self) -> bool {
        matches!(self.mode, TransactionMode::ReadOnly)
    }

    fn lock_inner_unchecked(&self) -> MutexGuard<'_, TxInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

type ExecResultRows = Result<Vec<Row>>;

impl TxInner {
    fn staged_mut(&mut self) -> Result<&mut InMemoryGraph> {
        self.staged
            .as_mut()
            .ok_or_else(|| anyhow!("transaction has no staged graph"))
    }
}

/// `RowSource` adapter that owns an `Arc<CompiledQuery>` so the
/// inner cursor's `'static` borrows into the plan stay valid for the
/// life of the wrapper. The inner cursor is stored first so it drops
/// before the Arc, releasing any borrows back into the plan.
struct StreamingCursorWithArc {
    cursor: Box<dyn RowSource + 'static>,
    _compiled: Arc<CompiledQuery>,
}

impl RowSource for StreamingCursorWithArc {
    fn next_row(&mut self) -> lora_executor::ExecResult<Option<Row>> {
        self.cursor.next_row()
    }
}

pub(crate) fn finalize_tx_stream(
    handle: &Arc<Mutex<TxInner>>,
    exhausted: bool,
    rollback_on_drop: bool,
) {
    if let Ok(mut inner) = handle.lock() {
        inner.cursor_active = false;

        if inner.closed {
            discard_transaction_state(&mut inner);
            return;
        }

        if exhausted || !rollback_on_drop {
            inner.pending_savepoint = None;
            inner.cursor_dropped_dirty = false;
            return;
        }

        if let Some(sp) = inner.pending_savepoint.take() {
            apply_savepoint(&mut inner, sp);
        }
        inner.cursor_dropped_dirty = false;
    }
}

fn discard_transaction_state(inner: &mut TxInner) {
    // A full transaction rollback supersedes any pending cursor savepoint.
    inner.pending_savepoint = None;
    inner.cursor_dropped_dirty = false;
    inner.cursor_active = false;
    inner.staged = None;
    if let Ok(mut buf) = inner.buffer.lock() {
        buf.clear();
    }
    inner.closed = true;
}

fn take_savepoint(inner: &TxInner, clone_staged: bool) -> Savepoint {
    let buffer_len = inner.buffer.lock().ok().map(|b| b.len()).unwrap_or(0);
    Savepoint {
        staged: if clone_staged {
            inner.staged.as_ref().cloned()
        } else {
            None
        },
        buffer_len,
    }
}

fn restore_savepoint(inner: &mut TxInner, savepoint: Option<Savepoint>) {
    if let Some(sp) = savepoint {
        apply_savepoint(inner, sp);
    }
}

fn apply_savepoint(inner: &mut TxInner, sp: Savepoint) {
    if let Ok(mut buf) = inner.buffer.lock() {
        buf.truncate(sp.buffer_len);
    }

    let Some(mut graph) = sp.staged else {
        inner.staged = None;
        return;
    };

    // Rebuild the staged graph from the snapshot and re-install the
    // buffering recorder. `InMemoryGraph::clone` deliberately drops
    // recorders, so the snapshot has none until we put it back.
    if matches!(inner.mode, TransactionMode::ReadWrite) && inner.buffer_mutations {
        graph.set_mutation_recorder(Some(
            Arc::new(BufferingRecorder::new(inner.buffer.clone())) as Arc<dyn MutationRecorder>
        ));
    }
    inner.staged = Some(graph);
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // If the user never called commit/rollback, treat it as a
        // rollback: drop staged changes and the buffered mutations.
        // The live RwLock guard is released as part of dropping `self.live`.
        if let Ok(mut inner) = self.inner.lock() {
            if !inner.closed {
                if inner.cursor_active {
                    // A tx-bound stream may still be borrowing the
                    // staged graph through `inner`. Leave that graph
                    // in place until the stream drops, but mark the
                    // transaction closed so finalization discards it
                    // instead of making it commit-eligible.
                    inner.closed = true;
                } else {
                    discard_transaction_state(&mut inner);
                }
            }
        }
    }
}
