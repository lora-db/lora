use lora_analyzer::symbols::VarId;
use lora_analyzer::{
    ResolvedClause, ResolvedExpr, ResolvedMergeAction, ResolvedPattern, ResolvedPatternPart,
    ResolvedProjection, ResolvedRemoveItem, ResolvedSetItem, ResolvedSortItem,
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
    NodeByPropertyScan(NodeByPropertyScan),
    NodeByPropertyRangeScan(NodeByPropertyRangeScan),
    NodeByTextScan(NodeByTextScan),
    NodeByPointScan(NodeByPointScan),
    RelByPropertyRangeScan(RelByPropertyRangeScan),
    RelByTextScan(RelByTextScan),
    RelByPointScan(RelByPointScan),
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
    Foreach(Foreach),
    OptionalMatch(OptionalMatch),
    PathBuild(PathBuild),
    CallSubquery(CallSubquery),
}

/// `FOREACH (var IN list | body...)` — for each input row, evaluate
/// the list, then run each body clause once per element with `var`
/// bound to the element. The body is a flat list of resolved updating
/// clauses applied for side effects only; the outer row is emitted
/// unchanged after the loop.
#[derive(Debug, Clone)]
pub struct Foreach {
    pub input: PlanNodeId,
    pub variable: VarId,
    pub list: ResolvedExpr,
    pub body: Vec<ResolvedClause>,
}

/// `CALL { ... }` subquery: for each upstream row, runs the inner
/// sub-plan with the upstream row as its initial argument, then
/// emits the cartesian product of `(upstream row, inner row)` for
/// each inner row produced. `new_vars` are the VarIds the inner
/// RETURN exposes to the outer scope.
#[derive(Debug, Clone)]
pub struct CallSubquery {
    pub input: PlanNodeId,
    pub inner: PlanNodeId,
    pub new_vars: Vec<VarId>,
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
pub struct NodeByPropertyScan {
    pub input: Option<PlanNodeId>,
    pub var: VarId,
    /// Each inner Vec is a disjunctive group (OR). Outer Vec is conjunctive (AND).
    pub labels: Vec<Vec<String>>,
    pub key: String,
    pub value: ResolvedExpr,
}

/// Range-bounded property scan rewritten from `Filter(NodeScan, var.prop CMP value)`
/// patterns. `lo == None` means `-∞`, `hi == None` means `+∞`. Inclusivity flags
/// distinguish `>` from `>=` and `<` from `<=`. Both bounds combined cover
/// `BETWEEN`-style queries (`a < x AND x <= b`).
#[derive(Debug, Clone)]
pub struct NodeByPropertyRangeScan {
    pub input: Option<PlanNodeId>,
    pub var: VarId,
    pub labels: Vec<Vec<String>>,
    pub key: String,
    pub lo: Option<ResolvedExpr>,
    pub lo_inclusive: bool,
    pub hi: Option<ResolvedExpr>,
    pub hi_inclusive: bool,
}

/// Trigram-backed property scan rewritten from `Filter(NodeScan, var.prop OP "literal")`
/// where OP is `STARTS WITH`, `ENDS WITH`, or `CONTAINS`. The executor consults
/// the trigram registry for candidates and re-verifies the predicate.
#[derive(Debug, Clone)]
pub struct NodeByTextScan {
    pub input: Option<PlanNodeId>,
    pub var: VarId,
    pub labels: Vec<Vec<String>>,
    pub key: String,
    pub predicate: TextPredicate,
    pub query: ResolvedExpr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextPredicate {
    StartsWith,
    EndsWith,
    Contains,
}

/// Spatial-index scan rewritten from `Filter(NodeScan, predicate)`
/// where the predicate is `point.withinBBox(n.prop, ll, ur)` or
/// `point.distance(n.prop, c) OP d`. Index probe is conservative;
/// the executor refilters with the precise predicate (including the
/// inclusivity of distance comparisons and the z-axis when the point
/// is 3D).
#[derive(Debug, Clone)]
pub struct NodeByPointScan {
    pub input: Option<PlanNodeId>,
    pub var: VarId,
    pub labels: Vec<Vec<String>>,
    pub key: String,
    pub predicate: PointPredicate,
}

#[derive(Debug, Clone)]
pub enum PointPredicate {
    WithinBBox {
        lower_left: ResolvedExpr,
        upper_right: ResolvedExpr,
    },
    WithinDistance {
        center: ResolvedExpr,
        max_distance: ResolvedExpr,
        inclusive: bool,
    },
}

/// Range-bounded relationship scan, the rel-side mirror of
/// [`NodeByPropertyRangeScan`]. Produces one row per indexed
/// relationship of `types`, binding `src`, `rel`, `dst` to the stored
/// endpoints. The optimizer only emits this operator for patterns
/// with anonymous endpoints (no upstream label/property constraints
/// on src/dst), since the operator does not refilter endpoints.
#[derive(Debug, Clone)]
pub struct RelByPropertyRangeScan {
    pub input: Option<PlanNodeId>,
    pub src: VarId,
    pub rel: VarId,
    pub dst: VarId,
    pub types: Vec<String>,
    pub direction: Direction,
    pub key: String,
    pub lo: Option<ResolvedExpr>,
    pub lo_inclusive: bool,
    pub hi: Option<ResolvedExpr>,
    pub hi_inclusive: bool,
}

/// Trigram-backed relationship scan. Mirror of [`NodeByTextScan`].
#[derive(Debug, Clone)]
pub struct RelByTextScan {
    pub input: Option<PlanNodeId>,
    pub src: VarId,
    pub rel: VarId,
    pub dst: VarId,
    pub types: Vec<String>,
    pub direction: Direction,
    pub key: String,
    pub predicate: TextPredicate,
    pub query: ResolvedExpr,
}

/// Spatial-index relationship scan. Mirror of [`NodeByPointScan`].
#[derive(Debug, Clone)]
pub struct RelByPointScan {
    pub input: Option<PlanNodeId>,
    pub src: VarId,
    pub rel: VarId,
    pub dst: VarId,
    pub types: Vec<String>,
    pub direction: Direction,
    pub key: String,
    pub predicate: PointPredicate,
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
    /// Optional upper bound for rows the sort must retain because a parent
    /// LIMIT will discard everything after this many sorted rows.
    pub top_k: Option<usize>,
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
