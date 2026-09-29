//! `Database::explain` — plan-only query inspection.
//!
//! `explain` parses, analyzes, and compiles a query, then returns the
//! resulting [`QueryPlan`]. The executor is *never* invoked, so calling
//! `explain` on a mutating query (CREATE / MERGE / SET / DELETE / REMOVE)
//! reports the plan without producing any side effects.
//!
//! `params` is accepted for API symmetry with `execute()` and reserved
//! for a future cost model — v1 does not consult parameter values when
//! producing the plan tree.

use std::any::Any;
use std::collections::BTreeMap;

use lora_compiler::{plan_tree_from_compiled, PlanTree, PlanTreeNode};
use lora_executor::{classify_stream, plan_result_columns, LoraValue};
use lora_store::{GraphStats, GraphStorage, GraphStorageMut};

use crate::database::Database;
use crate::error::LoraError;
use crate::explain::{PlanShape, QueryPlan};

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Compile `query` and return the plan that *would* run.
    ///
    /// The executor is not invoked: `explain` is a pure planning call
    /// and never produces side effects. Mutating queries return their
    /// plan without touching the graph.
    ///
    /// `params` is accepted for symmetry with `execute()`. The returned
    /// plan is identical regardless of the parameter values today;
    /// future cost-model work may use parameter values for selectivity
    /// estimation, so callers should pass real values when they have
    /// them.
    pub fn explain(
        &self,
        query: &str,
        _params: Option<BTreeMap<String, LoraValue>>,
    ) -> Result<QueryPlan, LoraError> {
        let (store, store_epoch) = self.read_store_with_epoch_deadline(None)?;
        let compiled = self
            .compile_query_cached(query, &*store, store_epoch)
            .map_err(LoraError::from_anyhow)?;
        let mut tree = plan_tree_from_compiled(&compiled);
        let stats = store.graph_stats();
        annotate_estimated_rows(&mut tree, &stats);
        let shape: PlanShape = classify_stream(&compiled).into();
        let result_columns = plan_result_columns(&compiled.physical);
        Ok(QueryPlan {
            query: query.to_string(),
            tree,
            shape,
            result_columns,
        })
    }
}

/// Walk a `PlanTree` and fill `estimated_rows` for the operators whose
/// cardinality is derivable from the cheap [`GraphStats`] snapshot.
/// Currently:
/// * `NodeScan` → `node_count`
/// * `NodeByLabelScan` → sum of per-label counts (handles `:A|B`)
/// * `NodeByPropertyScan` → uniform-distribution heuristic from
///   distinct-value count, when both label and property are recorded
/// * `NodeByPropertyRangeScan`, `NodeByTextScan`, `NodeByPointScan` → a
///   fraction of the label count, as the optimizer scores them
///
/// Operators that aren't covered keep the existing `None`. The
/// optimizer doesn't *use* these numbers yet — it's `EXPLAIN`-only —
/// but populating them now is what unlocks the cost model in a
/// follow-up.
pub(crate) fn annotate_estimated_rows(tree: &mut PlanTree, stats: &GraphStats) {
    annotate_node(&mut tree.root, stats);
}

fn annotate_node(node: &mut PlanTreeNode, stats: &GraphStats) {
    node.estimated_rows = match node.operator.as_str() {
        "NodeScan" => Some(stats.node_count as u64),
        "NodeByLabelScan" => labels_estimate(node, stats),
        "NodeByPropertyScan" => property_equality_estimate(node, stats),
        // The same selectivity guesses the optimizer scores seeks with
        // (`score_logical_op`), so EXPLAIN shows what the planner assumed.
        "NodeByPropertyRangeScan" => {
            let two_sided = node.details.contains_key("lo") && node.details.contains_key("hi");
            labels_estimate(node, stats).map(|n| n.div_ceil(if two_sided { 4 } else { 3 }))
        }
        "NodeByTextScan" => {
            let denom = match node.details.get("predicate").map(String::as_str) {
                Some("CONTAINS") => 2,
                _ => 4,
            };
            labels_estimate(node, stats).map(|n| n.div_ceil(denom))
        }
        "NodeByPointScan" => labels_estimate(node, stats).map(|n| n.div_ceil(5)),
        _ => None,
    };
    for child in &mut node.children {
        annotate_node(child, stats);
    }
}

fn labels_estimate(node: &PlanTreeNode, stats: &GraphStats) -> Option<u64> {
    let labels = node.details.get("labels")?;
    // `labels` is a humanised string from `label_groups_str`; we only
    // attempt the simple `:Foo` form for v1 cost.
    let trimmed = labels.trim_start_matches(':');
    let bare = trimmed.split('|').next()?.trim();
    stats.label_count(bare)
}

fn property_equality_estimate(node: &PlanTreeNode, stats: &GraphStats) -> Option<u64> {
    let property = node.details.get("key")?;
    let labels = node.details.get("labels")?;
    let trimmed = labels.trim_start_matches(':');
    let bare = trimmed.split('|').next()?.trim();
    stats.estimate_node_property_equality(bare, property)
}
