//! Query-execute family on [`Database<S>`].
//!
//! These methods cover the `execute_*` and `execute_rows_*` entry points
//! plus the cooperative-deadline variants and the plan-cache-backed
//! compile path. The actual write routing (optimistic OCC vs. pessimistic
//! `with_logged_write_guard`) is dispatched from
//! [`Database::execute_rows_with_params_deadline`]; the OCC body lives
//! in [`super::occ`] and the WAL-bracketed write closure lives in
//! [`super::write_guard`].

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use lora_analyzer::Analyzer;
use lora_ast::Document;
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::{
    classify_stream, collect_compiled, project_rows, ExecuteOptions, LoraValue,
    MutableExecutionContext, MutableExecutor, QueryResult, Row, StreamShape,
};
use lora_parser::parse_query;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph};

use crate::database::{Database, QUERY_FAILURE_POISON};
use crate::wal::write_scope::{ensure_wal_query_can_start, WalAbortPolicy};

use super::pull_mode::should_collect_read_via_pull;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    pub(super) fn compile_document_against(
        &self,
        document: &Document,
        store: &S,
    ) -> Result<CompiledQuery> {
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(document)?
        };

        Ok(Compiler::compile(&resolved))
    }

    /// Return a cached compiled plan for `query`, or compile + cache one
    /// against the supplied store. The store is only touched on cache
    /// miss, so a steady-state hot query never reaches the analyzer or
    /// the compiler.
    pub(crate) fn compile_query_cached(
        &self,
        query: &str,
        store: &S,
    ) -> Result<Arc<CompiledQuery>> {
        if let Some(plan) = self.plan_cache.get(query) {
            return Ok(plan);
        }
        let document = parse_query(query)?;
        let plan = Arc::new(self.compile_document_against(&document, store)?);
        self.plan_cache.insert(query, plan.clone());
        Ok(plan)
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
    ///    `MutationRecorder::record`, which buffers events in memory.
    /// 3. On Ok, `recorder.commit()` writes `TxBegin`, one batched
    ///    mutation record, and `TxCommit` only when mutations occurred;
    ///    the surrounding `recorder.flush()` runs only in that case so
    ///    a read-only query never pays an `fsync`.
    /// 4. On Err, `recorder.abort()` clears the pending batch. The
    ///    engine has no rollback, so the in-memory state may already
    ///    be partially mutated; the live handle is quarantined while
    ///    durable recovery stays atomic because no committed batch was
    ///    written.
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

    pub(crate) fn execute_rows_with_params_deadline(
        &self,
        query: &str,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
    ) -> Result<Vec<Row>> {
        // Compile (or fetch from the plan cache) under the read lock. The
        // read lock is also what the read-only fast path runs under, so we
        // can reuse it without a release/reacquire when the plan turns out
        // to be a pure read.
        let store = self.read_store_deadline(deadline)?;
        let compiled = self.compile_query_cached(query, &*store)?;
        let shape = classify_stream(&compiled);

        if matches!(shape, StreamShape::ReadOnly) {
            if let Some(rec) = &self.wal {
                ensure_wal_query_can_start(rec)?;
            }
            if deadline.is_none() && should_collect_read_via_pull(&compiled) {
                return collect_compiled(&*store, params, &compiled).map_err(anyhow::Error::from);
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
                .map_err(anyhow::Error::from);
        }

        // Mutating path. Drop the snapshot we used for compilation and
        // route through the optimistic auto-commit path: build the
        // working copy + mutate without holding any lock, then take
        // the writer Mutex only briefly to do a CAS publish (replays
        // buffered mutation events to the durable WAL inside the
        // critical section). Multiple auto-commit writers can run
        // their prep work in parallel; they only serialize at the
        // commit point. On conflict (another writer published since
        // we took our snapshot), retry from a fresh snapshot.
        drop(store);
        debug_assert!(shape.is_mutating());

        if std::any::TypeId::of::<S>() == std::any::TypeId::of::<InMemoryGraph>() {
            return self.execute_mutating_optimistic(params, deadline, &compiled);
        }

        // Backends that aren't `InMemoryGraph` don't support the
        // recorder install hook, so fall back to the pessimistic path.
        let store = self.write_store_deadline(deadline)?;
        self.with_logged_write_guard(
            store,
            WalAbortPolicy::PoisonIfMutated(QUERY_FAILURE_POISON),
            |store| {
                let mut executor = MutableExecutor::with_deadline(
                    MutableExecutionContext {
                        storage: store,
                        params,
                    },
                    deadline,
                );
                Ok(executor.execute_compiled_rows(&compiled)?)
            },
        )
    }
}
