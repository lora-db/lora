pub mod logical;
pub mod optimizer;
pub mod physical;
pub mod plan_tree;

mod param_fold;
mod pattern;
mod planner;

pub use plan_tree::{plan_tree_from_compiled, PlanTree, PlanTreeNode};

pub use logical::{
    Aggregation, Argument, Create, Delete, Expand, ExtraRangeBound, Filter, Foreach, Limit,
    LogicalOp, LogicalPlan, Merge, NodeByIdSeek, NodeByPointScan, NodeByPropertyRangeScan,
    NodeByPropertyScan, NodeByTextScan, NodeScan, OptionalMatch, PathBuild, PlanNodeId,
    PointPredicate, Projection, RelByIdSeek, RelByPointScan, RelByPropertyRangeScan, RelByTextScan,
    Remove, Set, Sort, SortLimit, TextPredicate, Unwind,
};
pub use optimizer::Optimizer;
pub use param_fold::NullnessOf;
pub use physical::{
    ArgumentExec, CreateExec, DeleteExec, ExpandExec, FilterExec, ForeachExec, HashAggregationExec,
    LimitExec, MergeExec, NodeByIdSeekExec, NodeByLabelScanExec, NodeByPointScanExec,
    NodeByPropertyRangeScanExec, NodeByPropertyScanExec, NodeByTextScanExec, NodeScanExec,
    OptionalMatchExec, PathBuildExec, PhysicalNodeId, PhysicalOp, PhysicalPlan, ProjectionExec,
    RelByIdSeekExec, RelByPointScanExec, RelByPropertyRangeScanExec, RelByTextScanExec, RemoveExec,
    SetExec, SortExec, UnwindExec,
};
pub use planner::Planner;

use lora_analyzer::resolved::ResolvedQuery;
use lora_store::GraphStats;

#[derive(Debug, Clone)]
pub struct CompiledQuery {
    pub physical: PhysicalPlan,
    /// Every `$name` the query reads ([`ResolvedQuery::parameters`]).
    pub parameters: std::collections::BTreeSet<String>,
    /// Additional UNION branches, each compiled independently.
    pub unions: Vec<CompiledUnionBranch>,
    /// Parameters a filter tests with `IS NULL` / `IS NOT NULL`, with the
    /// nullness the plan folded the test for (`Some(true)`: null; `None`:
    /// unknown, not folded). The plan is valid only for parameters with
    /// the same nullness; see [`CompiledQuery::fits`].
    pub null_params: std::collections::BTreeMap<String, Option<bool>>,
}

impl CompiledQuery {
    /// Whether this plan was built for parameters with the nullness
    /// `nullness` reports.
    pub fn fits(&self, nullness: NullnessOf<'_>) -> bool {
        self.null_params
            .iter()
            .all(|(name, assumed)| nullness(name) == *assumed)
    }
}

#[derive(Debug, Clone)]
pub struct CompiledUnionBranch {
    /// If true, UNION ALL (keep duplicates). If false, plain UNION (deduplicate).
    pub all: bool,
    pub physical: PhysicalPlan,
}

pub struct Compiler;

impl Compiler {
    /// Compile a resolved query into an executable plan, using `stats`
    /// for cost-based rewrite selection. Pass [`GraphStats::default()`]
    /// when no cardinality information is available — the optimizer
    /// then falls back to the conservative "commit any matching
    /// rewrite" behaviour that the runtime executor handles safely.
    pub fn compile(query: &ResolvedQuery, stats: &GraphStats) -> CompiledQuery {
        Self::compile_for(query, stats, &|_| None)
    }

    /// [`Self::compile`] for parameters whose nullness `nullness` knows:
    /// `$p IS NULL` and `$p IS NOT NULL` in a filter fold to constants, so
    /// `$p IS NULL OR n.k IN $p` plans as the `IN` alone (or as no filter).
    /// The result records what it assumed in
    /// [`CompiledQuery::null_params`].
    pub fn compile_for(
        query: &ResolvedQuery,
        stats: &GraphStats,
        nullness: NullnessOf<'_>,
    ) -> CompiledQuery {
        let mut null_params = std::collections::BTreeMap::new();
        let physical = compile_physical(query, stats, nullness, &mut null_params);
        let unions = query
            .unions
            .iter()
            .map(|union_part| {
                let branch_query = ResolvedQuery {
                    clauses: union_part.clauses.clone(),
                    unions: Vec::new(),
                    parameters: Default::default(),
                };
                CompiledUnionBranch {
                    all: union_part.all,
                    physical: compile_physical(&branch_query, stats, nullness, &mut null_params),
                }
            })
            .collect();

        CompiledQuery {
            physical,
            unions,
            parameters: query.parameters.clone(),
            null_params,
        }
    }
}

fn compile_physical(
    query: &ResolvedQuery,
    stats: &GraphStats,
    nullness: NullnessOf<'_>,
    null_params: &mut std::collections::BTreeMap<String, Option<bool>>,
) -> PhysicalPlan {
    let mut planner = Planner::with_stats(stats);
    let mut logical = planner.plan(query);
    param_fold::fold_parameter_null_tests(&mut logical, nullness, null_params);

    let mut optimizer = Optimizer::new();
    let optimized = optimizer.optimize(logical, stats);

    // Lower by moving the logical plan; it is not needed after lowering.
    optimizer.lower_to_physical(optimized)
}
