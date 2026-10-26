//! Parse/analyze/compile helpers for [`Database`].
//!
//! Keeping these methods separate from the execution routing code makes the
//! query path easier to scan: this module owns turning query text or an AST
//! into a cached [`CompiledQuery`], while `execute` decides how to run it.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use lora_analyzer::{Analyzer, ResolvedQuery};
use lora_compiler::{CompiledQuery, Compiler};
use lora_executor::LoraValue;
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
    params: &BTreeMap<String, LoraValue>,
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
    fn compile_resolved_with_stats(
        resolved: &ResolvedQuery,
        stats: &GraphStats,
        params: &BTreeMap<String, LoraValue>,
    ) -> CompiledQuery {
        Compiler::compile_folding(
            resolved,
            stats,
            &|name| param_nullness(params, name),
            &lora_executor::fold_call,
        )
    }

    /// Return a cached compiled plan for `query`, or compile + cache one
    /// against the supplied store.
    ///
    /// The cache key is `(query, plan epoch)`. `store_epoch` (the live
    /// store's write counter for `store`) maps to a plan epoch that moves
    /// only when the store's plan signature changes: index and catalog
    /// changes, labels / types / keys appearing or going, counts crossing
    /// a power of two (see [`crate::plan_cache`]). Writes that change none
    /// of those keep reusing the plan. The signature is computed once per
    /// store epoch, by the first query that sees it.
    ///
    /// A plan that folded `$p IS NULL` tests for `params` (see
    /// [`Compiler::compile_for`]) fits only parameters with the same
    /// nullness; parameters that differ get their own plan, cached under
    /// the query text plus that nullness.
    pub(crate) fn compile_query_cached(
        &self,
        query: &str,
        store: &S,
        store_epoch: u64,
        params: &BTreeMap<String, LoraValue>,
    ) -> Result<Arc<CompiledQuery>> {
        let nullness = |name: &str| param_nullness(params, name);
        let (epoch, hit) = match self.plan_cache.lookup(query, store_epoch) {
            (Some(epoch), hit) => (Some(epoch), hit),
            (None, _) => {
                let epoch = self.plan_cache.resolve(store_epoch, store.plan_signature());
                (
                    epoch,
                    epoch.and_then(|epoch| self.plan_cache.get(query, epoch)),
                )
            }
        };
        let variant = match (epoch, hit) {
            (_, Some(plan)) if plan.fits(&nullness) => return Ok(plan),
            (Some(epoch), Some(plan)) => {
                let key = variant_key(query, &plan, &nullness);
                if let Some(variant) = self.plan_cache.get(&key, epoch) {
                    if variant.fits(&nullness) {
                        return Ok(variant);
                    }
                }
                Some(key)
            }
            _ => None,
        };
        // A new plan epoch (or a variant) misses the plan, but the parse
        // depends only on the text: reuse it and redo analysis and planning.
        let document = match self.plan_cache.document(query) {
            Some(document) => document,
            None => Arc::new(parse_query(query)?),
        };
        let resolved = {
            let mut analyzer = Analyzer::new(store);
            analyzer.analyze(&document)?
        };
        let stats = store.graph_stats();
        let plan = Arc::new(Self::compile_resolved_with_stats(&resolved, &stats, params));
        // `None`: a snapshot older than the newest resolved one with
        // different stats; its plan is not worth caching.
        let Some(epoch) = epoch else {
            return Ok(plan);
        };
        match variant {
            Some(key) => self.plan_cache.insert(&key, epoch, plan.clone()),
            None => {
                self.plan_cache
                    .insert_with_document(query, epoch, Some(document), plan.clone())
            }
        }
        Ok(plan)
    }
}

/// Whether parameter `name` is null in `params` (`None` when absent).
pub(crate) fn param_nullness(params: &BTreeMap<String, LoraValue>, name: &str) -> Option<bool> {
    params.get(name).map(|v| matches!(v, LoraValue::Null))
}

/// The cache key of the plan for `query` under the parameter nullness
/// `nullness`, for the parameters `plan` tests.
fn variant_key(
    query: &str,
    plan: &CompiledQuery,
    nullness: lora_compiler::NullnessOf<'_>,
) -> String {
    let mut key = String::with_capacity(query.len() + 16);
    key.push_str(query);
    key.push('\0');
    for name in plan.null_params.keys() {
        key.push_str(match nullness(name) {
            Some(true) => "n",
            Some(false) => "v",
            None => "?",
        });
        key.push_str(name);
        key.push('\0');
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_store::InMemoryGraph;

    fn run(db: &Database<InMemoryGraph>, query: &str) {
        db.execute(query, None)
            .unwrap_or_else(|e| panic!("{query}: {e}"));
    }

    /// The plan `execute` would use for `query` right now.
    fn plan(db: &Database<InMemoryGraph>, query: &str) -> Arc<CompiledQuery> {
        let (store, epoch) = db.read_store_with_epoch_deadline(None).unwrap();
        db.compile_query_cached(query, &store, epoch, &BTreeMap::new())
            .unwrap()
    }

    fn seed(db: &Database<InMemoryGraph>) {
        run(
            db,
            "UNWIND range(1, 100) AS i CREATE (:Person {id: i, name: 'p' + toString(i)})",
        );
    }

    #[test]
    fn plan_is_reused_across_writes() {
        let db = Database::in_memory();
        seed(&db);
        let write = "MATCH (p:Person {id: 7}) SET p.name = 'x'";
        let read = "MATCH (p:Person) WHERE p.id = 7 RETURN p.name";
        let (write_plan, read_plan) = (plan(&db, write), plan(&db, read));
        for _ in 0..5 {
            run(&db, write);
            // Counts and distinct values stay within their buckets.
            run(&db, "CREATE (:Person {id: 5}), (:Person {id: 6})");
            run(&db, "MATCH (p:Person {id: 6}) WITH p LIMIT 1 DELETE p");
        }
        assert!(Arc::ptr_eq(&write_plan, &plan(&db, write)));
        assert!(Arc::ptr_eq(&read_plan, &plan(&db, read)));
    }

    #[test]
    fn index_changes_replan() {
        let db = Database::in_memory();
        seed(&db);
        let query = "MATCH (p:Person) WHERE p.id > 95 RETURN p.id";
        let before = plan(&db, query);
        run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
        let indexed = plan(&db, query);
        assert!(!Arc::ptr_eq(&before, &indexed));
        let ops = |plan: &CompiledQuery| format!("{:?}", plan.physical.nodes);
        assert!(!ops(&before).contains("NodeByPropertyRangeScan"));
        assert!(ops(&indexed).contains("NodeByPropertyRangeScan"));
        run(&db, "DROP INDEX person_id");
        let dropped = plan(&db, query);
        assert!(!Arc::ptr_eq(&indexed, &dropped));
        assert!(!ops(&dropped).contains("NodeByPropertyRangeScan"));
    }

    #[test]
    fn constraint_changes_replan() {
        // A uniqueness constraint is backed by an index the planner sees.
        let db = Database::in_memory();
        seed(&db);
        let query = "MATCH (p:Person) WHERE p.name > 'p5' RETURN p.id";
        let before = plan(&db, query);
        run(
            &db,
            "CREATE CONSTRAINT person_name FOR (p:Person) REQUIRE p.name IS UNIQUE",
        );
        let constrained = plan(&db, query);
        assert!(!Arc::ptr_eq(&before, &constrained));
        run(&db, "DROP CONSTRAINT person_name");
        assert!(!Arc::ptr_eq(&constrained, &plan(&db, query)));
    }

    #[test]
    fn new_label_type_or_key_replans() {
        let db = Database::in_memory();
        seed(&db);
        let query = "MATCH (a:Person)-[:KNOWS]->(b:Later) RETURN b";
        let mut last = plan(&db, query);
        for write in [
            "CREATE (:Later)",
            "MATCH (a:Person {id: 1}), (b:Later) CREATE (a)-[:KNOWS]->(b)",
            "MATCH (b:Later) SET b.since = 1",
        ] {
            run(&db, write);
            let next = plan(&db, query);
            assert!(!Arc::ptr_eq(&last, &next), "{write} kept the plan");
            last = next;
        }
    }

    #[test]
    fn cardinality_crossing_a_power_of_two_replans() {
        let db = Database::in_memory();
        // 100 Person nodes: the 128 bucket ends at 127.
        seed(&db);
        let query = "MATCH (p:Person) RETURN count(p)";
        let first = plan(&db, query);
        // Repeated ids keep the distinct-value estimate where it was.
        run(&db, "UNWIND range(1, 27) AS i CREATE (:Person {id: 1})");
        assert!(Arc::ptr_eq(&first, &plan(&db, query)), "127 nodes");
        run(&db, "CREATE (:Person {id: 1})");
        assert!(!Arc::ptr_eq(&first, &plan(&db, query)), "128 nodes");
    }

    #[test]
    fn stale_snapshot_with_other_stats_is_not_cached() {
        let db = Database::in_memory();
        seed(&db);
        let query = "MATCH (p:Person) RETURN p.id";
        let (old_store, old_epoch) = db.read_store_with_epoch_deadline(None).unwrap();
        run(&db, "CREATE INDEX person_id FOR (p:Person) ON (p.id)");
        let current = plan(&db, query);
        let params = BTreeMap::new();
        let old = db
            .compile_query_cached(query, &old_store, old_epoch, &params)
            .unwrap();
        assert!(!Arc::ptr_eq(&current, &old));
        assert!(Arc::ptr_eq(&current, &plan(&db, query)));
    }

    fn sort_keys(plan: &CompiledQuery) -> Vec<lora_analyzer::ResolvedExpr> {
        plan.physical
            .nodes
            .iter()
            .find_map(|op| match op {
                lora_compiler::PhysicalOp::Sort(sort) => {
                    Some(sort.items.iter().map(|item| item.expr.clone()).collect())
                }
                _ => None,
            })
            .expect("plan sorts")
    }

    #[test]
    fn sort_key_restating_a_projected_property_reads_the_column() {
        use lora_analyzer::ResolvedExpr;
        let db = Database::in_memory();
        seed(&db);
        let keys = sort_keys(&plan(
            &db,
            "MATCH (p:Person) RETURN p.name AS name ORDER BY p.name DESC, p.id LIMIT 3",
        ));
        assert!(matches!(keys[0], ResolvedExpr::Variable(_)), "{keys:?}");
        // Not projected: still read from the node.
        assert!(matches!(keys[1], ResolvedExpr::Property { .. }), "{keys:?}");
        // An arbitrary expression is evaluated afresh, as Cypher requires.
        let keys = sort_keys(&plan(&db, "RETURN rand() AS r ORDER BY rand()"));
        assert!(matches!(keys[0], ResolvedExpr::Function { .. }), "{keys:?}");
    }
}
