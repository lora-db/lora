use lora_analyzer::symbols::VarId;
use lora_analyzer::{
    ResolvedExpr, ResolvedMergeAction, ResolvedPattern, ResolvedPatternPart, ResolvedProjection,
    ResolvedRemoveItem, ResolvedSetItem, ResolvedSortItem,
};
use lora_ast::{Direction, RangeLiteral};

pub type PlanNodeId = usize;

#[derive(Debug, Clone)]
pub struct LogicalPlan {
    pub root: PlanNodeId,
    pub nodes: Vec<LogicalOp>,
}

#[derive(Debug, Clone)]
pub enum LogicalOp {
    Argument(Argument),
    NodeScan(NodeScan),
    Expand(Expand),
    Filter(Filter),
    Projection(Projection),
    Unwind(Unwind),
    Aggregation(Aggregation),
    Sort(Sort),
    Limit(Limit),
    Merge(Merge),
    Delete(Delete),
    Set(Set),
    Remove(Remove),
    Create(Create),
    OptionalMatch(OptionalMatch),
    PathBuild(PathBuild),
}

/// Assembles a path value from matched node and relationship VarIds.
#[derive(Debug, Clone)]
pub struct PathBuild {
    pub input: PlanNodeId,
    /// VarId to store the assembled path.
    pub output: VarId,
    /// Node VarIds in order: head, chain[0].node, chain[1].node, ...
    pub node_vars: Vec<VarId>,
    /// Relationship VarIds in order: chain[0].rel, chain[1].rel, ...
    pub rel_vars: Vec<VarId>,
    /// `None` = normal path, `Some(false)` = shortestPath, `Some(true)` = allShortestPaths
    pub shortest_path_all: Option<bool>,
}

/// Left-outer-join style node: runs the inner sub-plan for each input row.
/// If no rows are produced, emits one row with nulls for the new variables.
#[derive(Debug, Clone)]
pub struct OptionalMatch {
    /// Upstream rows that feed the optional match.
    pub input: PlanNodeId,
    /// The root of the inner sub-plan that implements the pattern + filter.
    pub inner: PlanNodeId,
    /// Variables introduced by the optional match (need null-extension).
    pub new_vars: Vec<VarId>,
}

#[derive(Debug, Clone)]
pub struct Argument;

#[derive(Debug, Clone)]
pub struct NodeScan {
    pub input: Option<PlanNodeId>,
    pub var: VarId,
    /// Each inner Vec is a disjunctive group (OR). Outer Vec is conjunctive (AND).
    pub labels: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Expand {
    pub input: PlanNodeId,
    pub src: VarId,
    pub rel: Option<VarId>,
    pub dst: VarId,
    pub types: Vec<String>,
    pub direction: Direction,
    pub rel_properties: Option<ResolvedExpr>,
    pub range: Option<RangeLiteral>,
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub input: PlanNodeId,
    pub predicate: ResolvedExpr,
}

#[derive(Debug, Clone)]
pub struct Projection {
    pub input: PlanNodeId,
    pub distinct: bool,
    pub items: Vec<ResolvedProjection>,
    pub include_existing: bool,
}

#[derive(Debug, Clone)]
pub struct Unwind {
    pub input: PlanNodeId,
    pub expr: ResolvedExpr,
    pub alias: VarId,
}

#[derive(Debug, Clone)]
pub struct Aggregation {
    pub input: PlanNodeId,
    pub group_by: Vec<ResolvedProjection>,
    pub aggregates: Vec<ResolvedProjection>,
}

#[derive(Debug, Clone)]
pub struct Sort {
    pub input: PlanNodeId,
    pub items: Vec<ResolvedSortItem>,
}

#[derive(Debug, Clone)]
pub struct Limit {
    pub input: PlanNodeId,
    pub skip: Option<ResolvedExpr>,
    pub limit: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct Create {
    pub input: PlanNodeId,
    pub pattern: ResolvedPattern,
}

#[derive(Debug, Clone)]
pub struct Merge {
    pub input: PlanNodeId,
    pub pattern_part: ResolvedPatternPart,
    pub actions: Vec<ResolvedMergeAction>,
}

#[derive(Debug, Clone)]
pub struct Delete {
    pub input: PlanNodeId,
    pub detach: bool,
    pub expressions: Vec<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct Set {
    pub input: PlanNodeId,
    pub items: Vec<ResolvedSetItem>,
}

#[derive(Debug, Clone)]
pub struct Remove {
    pub input: PlanNodeId,
    pub items: Vec<ResolvedRemoveItem>,
}
