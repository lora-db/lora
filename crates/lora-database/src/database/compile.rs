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
use crate::error::DatabaseOperationError;

/// Refuse to run `compiled` when it reads a `$name` that `params` doesn't
/// supply: a missing parameter is a caller bug, and reading it as null
/// would silently match nothing or write a null. Pass `null` explicitly
/// for a parameter that has no value.
pub(crate) fn ensure_parameters(
    compiled: &CompiledQuery,
    params: &std::collections::BTreeMap<String, lora_executor::LoraValue>,
) -> Result<()> {
    let missing: Vec<String> = compiled
        .parameters
        .iter()
        .filter(|name| !params.contains_key(*name))
        .map(|name| format!("${name}"))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(DatabaseOperationError::invalid_params(format!(
        "expected parameter{}: {}",
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", ")
    ))
    .into())
}

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
        // A write bumps the epoch, so a repeated mutating query misses the
        // plan every time. Its parse is still valid (it depends only on the
        // text), so reuse it and redo only analysis and planning.
        let document = match self.plan_cache.document(query) {
            Some(document) => document,
            None => Arc::new(parse_query(query)?),
        };
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(&document)?
        };
        let stats = store.graph_stats();
        let plan = Arc::new(Self::compile_resolved_with_stats(&resolved, &stats));
        self.plan_cache
            .insert_with_document(query, store_epoch, Some(document), plan.clone());
        Ok(plan)
    }
}
