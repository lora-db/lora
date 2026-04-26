use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{anyhow, Result};
use lora_analyzer::Analyzer;
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    classify_stream, compiled_result_columns, project_rows, ExecuteOptions, ExecutionContext,
    Executor, LoraValue, MutableExecutionContext, MutableExecutor, QueryResult, Row,
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

/// Captures the staged graph and tx-local mutation buffer at the point
/// a statement is opened, so a failed/dropped statement can be rolled
/// back to that point without affecting earlier work in the same
/// transaction.
pub(crate) struct Savepoint {
    staged: InMemoryGraph,
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
}

/// Explicit transaction over the in-memory graph.
///
/// The implementation is conservative: it holds the database mutex
/// for the lifetime of the transaction, lazily creates a cloned
/// staging graph on the first mutating statement, and either swaps
/// that graph into place on commit (ReadWrite) or drops it on
/// rollback. Explicit mutating statements capture a graph +
/// WAL-buffer savepoint so a failed or dropped streaming statement
/// only rolls back its own effects, not the transaction as a whole.
///
/// When a WAL is attached, mutation events fire into a tx-local
/// buffer rather than the durable log. The buffer is replayed into
/// the WAL exactly once at commit, so recovery never observes
/// partial / aborted / dropped statements.
pub struct Transaction<'db> {
    pub(crate) live: Option<MutexGuard<'db, InMemoryGraph>>,
    pub(crate) inner: Arc<Mutex<TxInner>>,
    pub(crate) wal: Option<Arc<WalRecorder>>,
}

impl<'db> Transaction<'db> {
    /// Build a fresh transaction. Used by `Database::begin_transaction`.
    pub(crate) fn new(
        live: MutexGuard<'db, InMemoryGraph>,
        wal: Option<Arc<WalRecorder>>,
        mode: TransactionMode,
    ) -> Self {
        let inner = TxInner {
            staged: None,
            buffer: Arc::new(Mutex::new(Vec::new())),
            pending_savepoint: None,
            cursor_active: false,
            cursor_dropped_dirty: false,
            closed: false,
            mode,
        };
        Self {
            live: Some(live),
            inner: Arc::new(Mutex::new(inner)),
            wal,
        }
    }

    /// Transaction mode chosen at begin time.
    pub fn mode(&self) -> TransactionMode {
        self.lock_inner_unchecked().mode
    }

    /// Execute a query inside the transaction and return a materialized
    /// `QueryResult`.
    pub fn execute(&mut self, query: &str, options: Option<ExecuteOptions>) -> Result<QueryResult> {
        self.execute_with_params(query, options, BTreeMap::new())
    }

    /// Execute a parameterised query inside the transaction.
    pub fn execute_with_params(
        &mut self,
        query: &str,
        options: Option<ExecuteOptions>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryResult> {
        let rows = self.execute_rows_with_params(query, params)?;
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
        let compiled = self.compile_in_tx(query)?;
        self.execute_rows_compiled(&compiled, params)
    }

    /// Execute a pre-compiled query inside the transaction. Used by
    /// `Database::stream_with_params`'s auto-commit branch to avoid
    /// re-compiling a plan it already has.
    pub(crate) fn execute_rows_compiled(
        &mut self,
        compiled: &CompiledQuery,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        // ReadOnly tx: never clones, runs straight against live.
        if self.is_read_only_unchecked() {
            self.precheck_open_no_savepoint()?;
            let live = self
                .live
                .as_ref()
                .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
            let storage: &InMemoryGraph = live;
            let executor = Executor::new(ExecutionContext { storage, params });
            return executor
                .execute_compiled_rows(compiled)
                .map_err(|e| anyhow!(e));
        }

        // ReadWrite tx, lazy-clone aware.
        let mut inner = self.begin_statement()?;
        let is_mutating = classify_stream(compiled).is_mutating();

        if !is_mutating {
            // Read-only statement in a ReadWrite tx. Run against
            // staged if it has been materialized (so the read
            // sees prior in-tx writes), otherwise straight off
            // the live graph — which equals staged-as-it-would-be
            // because no writes have happened yet.
            return match inner.staged.as_ref() {
                Some(staged) => {
                    let executor = Executor::new(ExecutionContext {
                        storage: staged,
                        params,
                    });
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
                    let storage: &InMemoryGraph = live;
                    let executor = Executor::new(ExecutionContext { storage, params });
                    executor
                        .execute_compiled_rows(compiled)
                        .map_err(|e| anyhow!(e))
                }
            };
        }

        // Mutating statement: lazy-clone the live graph if this
        // is the first write in the tx, then capture a savepoint
        // and run the mutable executor.
        self.ensure_staged_locked(&mut inner)?;
        let savepoint = take_savepoint(&inner);

        let exec_result: ExecResultRows = {
            let staged = inner.staged_mut()?;
            let mut executor = MutableExecutor::new(MutableExecutionContext {
                storage: staged,
                params,
            });
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

    /// Execute a compiled query for a hidden auto-commit write stream.
    ///
    /// Auto-commit streams are single-statement transactions: if execution
    /// fails, or the returned stream is dropped before exhaustion, the whole
    /// hidden transaction is discarded. That means we do not need the
    /// per-statement savepoint clone used by explicit multi-statement
    /// transactions.
    pub(crate) fn execute_rows_compiled_autocommit(
        &mut self,
        compiled: &CompiledQuery,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<Vec<Row>> {
        if self.is_read_only_unchecked() || !classify_stream(compiled).is_mutating() {
            return self.execute_rows_compiled(compiled, params);
        }

        let mut inner = self.begin_statement()?;
        self.ensure_staged_locked(&mut inner)?;

        let exec_result: ExecResultRows = {
            let staged = inner.staged_mut()?;
            let mut executor = MutableExecutor::new(MutableExecutionContext {
                storage: staged,
                params,
            });
            executor
                .execute_compiled_rows(compiled)
                .map_err(|e| anyhow!(e))
        };

        match exec_result {
            Ok(rows) => Ok(rows),
            Err(err) => {
                // No savepoint was taken. The only correct recovery is to
                // abandon the entire hidden auto-commit transaction.
                discard_transaction_state(&mut inner);
                drop(inner);
                self.live.take();
                Err(err)
            }
        }
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
                let mut analyzer = Analyzer::new(&**live);
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
        let mut staged: InMemoryGraph = (**live).clone();
        staged.set_mutation_recorder(Some(
            Arc::new(BufferingRecorder::new(inner.buffer.clone())) as Arc<dyn MutationRecorder>
        ));
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
        let compiled = self.compile_in_tx(query)?;
        let columns = compiled_result_columns(&compiled);
        self.stream_compiled(&compiled, columns, params)
    }

    /// Open a tx-bound stream for an already-compiled plan. Lets
    /// `Database::stream_with_params` reuse the plan it built for
    /// classification.
    pub(crate) fn stream_compiled(
        &mut self,
        compiled: &CompiledQuery,
        columns: Vec<String>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'static>> {
        if self.is_read_only_unchecked() {
            return self.stream_read_only_compiled(compiled, columns, params);
        }

        let mut inner = self.begin_statement()?;
        let is_mutating = classify_stream(compiled).is_mutating();

        // Read-only statement in a ReadWrite tx: no savepoint, no
        // staged-graph requirement. Run against staged if it
        // exists (sees in-tx writes), otherwise live.
        if !is_mutating {
            let exec_result: ExecResultRows = match inner.staged.as_ref() {
                Some(staged) => {
                    let executor = Executor::new(ExecutionContext {
                        storage: staged,
                        params,
                    });
                    executor
                        .execute_compiled_rows(compiled)
                        .map_err(|e| anyhow!(e))
                }
                None => {
                    let live = self
                        .live
                        .as_ref()
                        .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
                    let storage: &InMemoryGraph = live;
                    let executor = Executor::new(ExecutionContext { storage, params });
                    executor
                        .execute_compiled_rows(compiled)
                        .map_err(|e| anyhow!(e))
                }
            };
            let rows = exec_result?;
            inner.cursor_active = true;
            drop(inner);
            return Ok(QueryStream::for_tx(rows, columns, self.inner.clone()));
        }

        // Mutating statement: lazy-clone, capture savepoint, run.
        self.ensure_staged_locked(&mut inner)?;
        let savepoint = take_savepoint(&inner);

        let exec_result: ExecResultRows = {
            let staged = inner.staged_mut()?;
            let mut executor = MutableExecutor::new(MutableExecutionContext {
                storage: staged,
                params,
            });
            executor
                .execute_compiled_rows(compiled)
                .map_err(|e| anyhow!(e))
        };

        let rows = match exec_result {
            Ok(rows) => rows,
            Err(err) => {
                restore_savepoint(&mut inner, savepoint);
                return Err(err);
            }
        };

        // Park the savepoint on the tx so the cursor's Drop can
        // signal "rollback this statement" without owning the
        // savepoint itself.
        inner.pending_savepoint = savepoint;
        inner.cursor_active = true;
        drop(inner);

        Ok(QueryStream::for_tx(rows, columns, self.inner.clone()))
    }

    /// ReadOnly fast path for `stream_compiled`. Materializes
    /// rows against live and returns a tx-bound `QueryStream`.
    fn stream_read_only_compiled(
        &mut self,
        compiled: &CompiledQuery,
        columns: Vec<String>,
        params: BTreeMap<String, LoraValue>,
    ) -> Result<QueryStream<'static>> {
        // Take the cursor token first so a concurrent commit
        // attempt can't slip in between the precheck and the
        // QueryStream construction.
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.closed {
                return Err(anyhow!("transaction is already closed"));
            }
            if inner.cursor_active {
                return Err(anyhow!(
                    "cannot start a new statement while a streaming cursor is still active"
                ));
            }
            inner.cursor_active = true;
        }

        let result: Result<Vec<Row>> = (|| {
            let live = self
                .live
                .as_ref()
                .ok_or_else(|| anyhow!("transaction has no live graph guard"))?;
            let storage: &InMemoryGraph = live;
            let executor = Executor::new(ExecutionContext { storage, params });
            executor
                .execute_compiled_rows(compiled)
                .map_err(|e| anyhow!(e))
        })();

        match result {
            Ok(rows) => Ok(QueryStream::for_tx(rows, columns, self.inner.clone())),
            Err(err) => {
                if let Ok(mut inner) = self.inner.lock() {
                    inner.cursor_active = false;
                }
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
            **live = staged;
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
    /// pay one mutex acquisition.
    fn is_read_only_unchecked(&self) -> bool {
        matches!(self.lock_inner_unchecked().mode, TransactionMode::ReadOnly)
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

fn take_savepoint(inner: &TxInner) -> Option<Savepoint> {
    let staged = inner.staged.as_ref()?;
    let buffer_len = inner.buffer.lock().ok().map(|b| b.len()).unwrap_or(0);
    Some(Savepoint {
        staged: staged.clone(),
        buffer_len,
    })
}

fn restore_savepoint(inner: &mut TxInner, savepoint: Option<Savepoint>) {
    if let Some(sp) = savepoint {
        apply_savepoint(inner, sp);
    }
}

fn apply_savepoint(inner: &mut TxInner, sp: Savepoint) {
    // Rebuild the staged graph from the snapshot and re-install the
    // buffering recorder. `InMemoryGraph::clone` deliberately drops
    // recorders, so the snapshot has none until we put it back.
    let mut graph = sp.staged;
    if matches!(inner.mode, TransactionMode::ReadWrite) {
        graph.set_mutation_recorder(Some(
            Arc::new(BufferingRecorder::new(inner.buffer.clone())) as Arc<dyn MutationRecorder>
        ));
    }
    inner.staged = Some(graph);
    if let Ok(mut buf) = inner.buffer.lock() {
        buf.truncate(sp.buffer_len);
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        // If the user never called commit/rollback, treat it as a
        // rollback: drop staged changes and the buffered mutations.
        // The live MutexGuard is released as part of dropping
        // `self.live`.
        if let Ok(mut inner) = self.inner.lock() {
            if !inner.closed {
                discard_transaction_state(&mut inner);
            }
        }
    }
}
