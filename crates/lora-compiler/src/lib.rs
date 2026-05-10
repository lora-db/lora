pub mod logical;
pub mod optimizer;
pub mod physical;
pub mod plan_tree;

mod pattern;
mod planner;

pub use plan_tree::{plan_tree_from_compiled, PlanTree, PlanTreeNode};

pub use logical::{
    Aggregation, Argument, Create, Delete, Expand, Filter, Limit, LogicalOp, LogicalPlan, Merge,
    NodeByPointScan, NodeByPropertyRangeScan, NodeByPropertyScan, NodeByTextScan, NodeScan,
    OptionalMatch, PathBuild, PlanNodeId, PointPredicate, Projection, RelByPointScan,
    RelByPropertyRangeScan, RelByTextScan, Remove, Set, Sort, TextPredicate, Unwind,
};
pub use optimizer::Optimizer;
pub use physical::{
    ArgumentExec, CreateExec, DeleteExec, ExpandExec, FilterExec, HashAggregationExec, LimitExec,
    MergeExec, NodeByLabelScanExec, NodeByPointScanExec, NodeByPropertyRangeScanExec,
    NodeByPropertyScanExec, NodeByTextScanExec, NodeScanExec, OptionalMatchExec, PathBuildExec,
    PhysicalNodeId, PhysicalOp, PhysicalPlan, ProjectionExec, RelByPointScanExec,
    RelByPropertyRangeScanExec, RelByTextScanExec, RemoveExec, SetExec, SortExec, UnwindExec,
};
pub use planner::Planner;

use lora_analyzer::resolved::ResolvedQuery;
use lora_store::GraphStats;

#[derive(Debug, Clone)]
pub struct CompiledQuery {
    pub physical: PhysicalPlan,
    /// Additional UNION branches, each compiled independently.
    pub unions: Vec<CompiledUnionBranch>,
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
        let physical = compile_physical(query, stats);
        let unions = query
            .unions
            .iter()
            .map(|union_part| {
                let branch_query = ResolvedQuery {
                    clauses: union_part.clauses.clone(),
                    unions: Vec::new(),
                };
                CompiledUnionBranch {
                    all: union_part.all,
                    physical: compile_physical(&branch_query, stats),
                }
            })
            .collect();

        CompiledQuery { physical, unions }
    }
}

fn compile_physical(query: &ResolvedQuery, stats: &GraphStats) -> PhysicalPlan {
    let mut planner = Planner::new();
    let logical = planner.plan(query);

    let mut optimizer = Optimizer::new();
    let optimized = optimizer.optimize(logical, stats);

    // Lower by moving the logical plan; it is not needed after lowering.
    optimizer.lower_to_physical(optimized)
}
