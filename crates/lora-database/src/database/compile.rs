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
    /// against the supplied store.
    ///
    /// The cache key is `(query, write_epoch)`. The write epoch is a
    /// cheap atomic counter bumped by every publish/in-place write on
    /// the live store, so cache hits avoid the O(labels + types +
    /// scoped properties + indexes) `GraphStats` rebuild + BTreeMap
    /// hash that the old "compute fingerprint on every execute" path
    /// paid. On miss we build full stats once and hand them to the
    /// optimizer.
    pub(crate) fn compile_query_cached(
        &self,
        query: &str,
        store: &S,
        store_epoch: u64,
    ) -> Result<Arc<CompiledQuery>> {
        if let Some(plan) = self.plan_cache.get(query, store_epoch) {
            return Ok(plan);
        }
        let document = parse_query(query)?;
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(&document)?
        };
        let stats = store.graph_stats();
        let plan = Arc::new(Self::compile_resolved_with_stats(&resolved, &stats));
        self.plan_cache.insert(query, store_epoch, plan.clone());
        Ok(plan)
    }
}
