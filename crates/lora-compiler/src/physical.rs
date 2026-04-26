use lora_analyzer::symbols::VarId;
use lora_analyzer::{
    ResolvedExpr, ResolvedMergeAction, ResolvedPattern, ResolvedPatternPart, ResolvedProjection,
    ResolvedRemoveItem, ResolvedSetItem, ResolvedSortItem,
};
use lora_ast::{Direction, RangeLiteral};

pub type PhysicalNodeId = usize;

#[derive(Debug, Clone)]
pub struct PhysicalPlan {
    pub root: PhysicalNodeId,
    pub nodes: Vec<PhysicalOp>,
}

#[derive(Debug, Clone)]
pub enum PhysicalOp {
    Argument(ArgumentExec),
    NodeScan(NodeScanExec),
    NodeByLabelScan(NodeByLabelScanExec),
    NodeByPropertyScan(NodeByPropertyScanExec),
    Expand(ExpandExec),
    Filter(FilterExec),
    Projection(ProjectionExec),
    Unwind(UnwindExec),
    HashAggregation(HashAggregationExec),
    Sort(SortExec),
    Limit(LimitExec),
    Create(CreateExec),
    Merge(MergeExec),
    Delete(DeleteExec),
    Set(SetExec),
    Remove(RemoveExec),
    OptionalMatch(OptionalMatchExec),
    PathBuild(PathBuildExec),
}

#[derive(Debug, Clone)]
pub struct PathBuildExec {
    pub input: PhysicalNodeId,
    pub output: VarId,
    pub node_vars: Vec<VarId>,
    pub rel_vars: Vec<VarId>,
    pub shortest_path_all: Option<bool>,
}

/// Left-outer-join: for each input row, runs inner sub-plan.
/// If inner produces nothing, emits one row with nulls for new_vars.
#[derive(Debug, Clone)]
pub struct OptionalMatchExec {
    pub input: PhysicalNodeId,
    pub inner: PhysicalNodeId,
    pub new_vars: Vec<VarId>,
}

#[derive(Debug, Clone)]
pub struct ArgumentExec;

#[derive(Debug, Clone)]
pub struct NodeScanExec {
    pub input: Option<PhysicalNodeId>,
    pub var: VarId,
}

#[derive(Debug, Clone)]
pub struct NodeByLabelScanExec {
    pub input: Option<PhysicalNodeId>,
    pub var: VarId,
    /// Each inner Vec is a disjunctive group (OR). Outer Vec is conjunctive (AND).
    pub labels: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct NodeByPropertyScanExec {
    pub input: Option<PhysicalNodeId>,
    pub var: VarId,
    /// Each inner Vec is a disjunctive group (OR). Outer Vec is conjunctive (AND).
    pub labels: Vec<Vec<String>>,
    pub key: String,
    pub value: ResolvedExpr,
}

#[derive(Debug, Clone)]
pub struct ExpandExec {
    pub input: PhysicalNodeId,
    pub src: VarId,
    pub rel: Option<VarId>,
    pub dst: VarId,
    pub types: Vec<String>,
    pub direction: Direction,
    pub rel_properties: Option<ResolvedExpr>,
    pub range: Option<RangeLiteral>,
}

#[derive(Debug, Clone)]
pub struct FilterExec {
    pub input: PhysicalNodeId,
    pub predicate: ResolvedExpr,
}

#[derive(Debug, Clone)]
pub struct ProjectionExec {
    pub input: PhysicalNodeId,
    pub distinct: bool,
    pub items: Vec<ResolvedProjection>,
    pub include_existing: bool,
}

#[derive(Debug, Clone)]
pub struct UnwindExec {
    pub input: PhysicalNodeId,
    pub expr: ResolvedExpr,
    pub alias: VarId,
}

#[derive(Debug, Clone)]
pub struct HashAggregationExec {
    pub input: PhysicalNodeId,
    pub group_by: Vec<ResolvedProjection>,
    pub aggregates: Vec<ResolvedProjection>,
}

#[derive(Debug, Clone)]
pub struct SortExec {
    pub input: PhysicalNodeId,
    pub items: Vec<ResolvedSortItem>,
}

#[derive(Debug, Clone)]
pub struct LimitExec {
    pub input: PhysicalNodeId,
    pub skip: Option<ResolvedExpr>,
    pub limit: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct CreateExec {
    pub input: PhysicalNodeId,
    pub pattern: ResolvedPattern,
}

#[derive(Debug, Clone)]
pub struct MergeExec {
    pub input: PhysicalNodeId,
    pub pattern_part: ResolvedPatternPart,
    pub actions: Vec<ResolvedMergeAction>,
}

#[derive(Debug, Clone)]
pub struct DeleteExec {
    pub input: PhysicalNodeId,
    pub detach: bool,
    pub expressions: Vec<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct SetExec {
    pub input: PhysicalNodeId,
    pub items: Vec<ResolvedSetItem>,
}

#[derive(Debug, Clone)]
pub struct RemoveExec {
    pub input: PhysicalNodeId,
    pub items: Vec<ResolvedRemoveItem>,
}
