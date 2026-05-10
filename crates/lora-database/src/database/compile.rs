//! Parse/analyze/compile helpers for [`Database`].
//!
//! Keeping these methods separate from the execution routing code makes the
//! query path easier to scan: this module owns turning query text or an AST
//! into a cached [`CompiledQuery`], while `execute` decides how to run it.

use std::any::Any;
use std::sync::Arc;

use anyhow::Result;
use lora_analyzer::{Analyzer, ResolvedQuery};
use lora_compiler::{CompiledQuery, Compiler};
use lora_parser::parse_query;
use lora_store::{GraphStats, GraphStorage, GraphStorageMut};

use crate::database::Database;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    fn compile_resolved_with_stats(resolved: &ResolvedQuery, stats: &GraphStats) -> CompiledQuery {
        Compiler::compile(resolved, stats)
    }

    /// Return a cached compiled plan for `query`, or compile + cache one
    /// against the supplied store. Cache hits still read a cheap stats
    /// fingerprint so catalog/cardinality changes get fresh cost-based
    /// operator choices.
    pub(crate) fn compile_query_cached(
        &self,
        query: &str,
        store: &S,
    ) -> Result<Arc<CompiledQuery>> {
        let stats = store.graph_stats();
        let stats_fingerprint = stats.fingerprint();
        if let Some(plan) = self.plan_cache.get(query, stats_fingerprint) {
            return Ok(plan);
        }
        let document = parse_query(query)?;
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(&document)?
        };
        let plan = Arc::new(Self::compile_resolved_with_stats(&resolved, &stats));
        self.plan_cache
            .insert(query, stats_fingerprint, plan.clone());
        Ok(plan)
    }
}
