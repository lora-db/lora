//! `Database::profile` — execute a query and report runtime metrics.
//!
//! Unlike `explain`, `profile` runs the query against the live database.
//! Mutating queries (CREATE / MERGE / SET / DELETE / REMOVE) are
//! persisted exactly as they would be from `execute()`. Callers who
//! want to inspect a mutating plan without running it should use
//! `explain` instead.
//!
//! v1 surfaces coarse metrics (total elapsed time, total rows produced,
//! whether mutations occurred). Per-operator instrumentation is
//! reserved for a future phase and lives in
//! [`crate::explain::ProfileMetrics::per_operator`] as an empty map.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use web_time::Instant;

use lora_compiler::plan_tree_from_compiled;
use lora_executor::{
    classify_stream, collect_compiled, plan_result_columns, CollectorGuard, LoraValue,
    MetricsCollector, StreamShape,
};
use lora_store::{GraphStorage, GraphStorageMut};

use crate::database::Database;
use crate::error::LoraError;
use crate::explain::{OperatorMetrics, PlanShape, ProfileMetrics, QueryPlan, QueryProfile};

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Execute `query` and return the plan plus runtime metrics.
    ///
    /// **PROFILE executes the query for real.** Mutating queries
    /// (CREATE / MERGE / SET / DELETE / REMOVE) produce exactly the
    /// same side effects as `execute()` — the WAL is written, snapshots
    /// observe the commit, and the live store advances. Use
    /// [`Database::explain`] to inspect a plan without running it.
    pub fn profile(
        &self,
        query: &str,
        params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryProfile, LoraError> {
        let params = params.unwrap_or_default();

        // Compile up front so we can attach the plan to the profile
        // result whether execution succeeds or not. (Errors during
        // compile flow through the same `LoraError` path as
        // `execute()`, so the caller sees consistent error codes.)
        let (store, store_epoch) = self.read_store_with_epoch_deadline(None)?;
        let compiled = self
            .compile_query_cached(query, &*store, store_epoch)
            .map_err(LoraError::from_anyhow)?;
        let tree = plan_tree_from_compiled(&compiled);
        let shape: PlanShape = classify_stream(&compiled).into();
        let result_columns = plan_result_columns(&compiled.physical);
        drop(store);

        let plan = QueryPlan {
            query: query.to_string(),
            tree,
            shape,
            result_columns,
        };

        let collector = Arc::new(MetricsCollector::new());
        let _guard = CollectorGuard::install(collector.clone());

        // For read-only queries route through the streaming pull
        // executor so the metrics collector sees every operator's
        // `next_row` call. The `execute()` fast path uses a buffered
        // executor when there's no early LIMIT; that path skips
        // `build_streaming`, so we'd get only top-level totals back.
        // Profile accepts the small perf cost in exchange for per-op
        // timing.
        let started = Instant::now();
        let rows = if matches!(classify_stream(&compiled), StreamShape::ReadOnly) {
            let snapshot = self.read_store();
            let res = collect_compiled(&*snapshot, params, &compiled)
                .map_err(|e| LoraError::from_anyhow(e.into()));
            drop(snapshot);
            res?
        } else {
            self.execute_rows_with_params(query, params)
                .map_err(|e| LoraError::from_anyhow(e.into()))?
        };
        let total_elapsed_ns = started.elapsed().as_nanos() as u64;

        // Drop the guard before reading the snapshot so the
        // thread-local is cleared even if `snapshot` panics. (`Arc`
        // ensures the collector outlives the guard.)
        drop(_guard);
        let per_operator = collector
            .snapshot()
            .into_iter()
            .map(|(id, op)| {
                (
                    id,
                    OperatorMetrics {
                        rows: op.rows,
                        elapsed_ns: op.elapsed_ns,
                        next_calls: op.next_calls,
                        // db_hits is reserved for a future phase.
                        db_hits: 0,
                    },
                )
            })
            .collect();

        let metrics = ProfileMetrics {
            total_elapsed_ns,
            total_rows: rows.len() as u64,
            mutated: shape.is_mutating(),
            per_operator,
        };

        Ok(QueryProfile { plan, metrics })
    }
}
