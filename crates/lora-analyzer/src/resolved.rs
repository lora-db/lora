use crate::symbols::*;
use lora_ast::{BinaryOp, Direction, ListPredicateKind, RangeLiteral, SortDirection, Span, UnaryOp};

#[derive(Debug, Clone)]
pub struct ResolvedQuery {
    pub clauses: Vec<ResolvedClause>,
    /// Additional UNION branches. Each branch is a separate resolved query
    /// that produces rows to be combined with the head query's results.
    pub unions: Vec<ResolvedUnionPart>,
}

#[derive(Debug, Clone)]
pub struct ResolvedUnionPart {
    /// If true, this is UNION ALL (no deduplication). If false, plain UNION (deduplicate).
    pub all: bool,
    /// The resolved clauses for this branch.
    pub clauses: Vec<ResolvedClause>,
}

#[derive(Debug, Clone)]
pub enum ResolvedClause {
    Match(ResolvedMatch),
    Unwind(ResolvedUnwind),
    Create(ResolvedCreate),
    Merge(ResolvedMerge),
    Delete(ResolvedDelete),
    Set(ResolvedSet),
    Remove(ResolvedRemove),
    Return(ResolvedReturn),
    With(ResolvedWith),
}

#[derive(Debug, Clone)]
pub struct ResolvedMatch {
    pub optional: bool,
    pub pattern: ResolvedPattern,
    pub where_: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct ResolvedUnwind {
    pub expr: ResolvedExpr,
    pub alias: VarId,
}

#[derive(Debug, Clone)]
pub struct ResolvedCreate {
    pub pattern: ResolvedPattern,
}

#[derive(Debug, Clone)]
pub struct ResolvedMerge {
    pub pattern_part: ResolvedPatternPart,
    pub actions: Vec<ResolvedMergeAction>,
}

#[derive(Debug, Clone)]
pub struct ResolvedMergeAction {
    pub on_match: bool,
    pub set: ResolvedSet,
}

#[derive(Debug, Clone)]
pub struct ResolvedDelete {
    pub detach: bool,
    pub expressions: Vec<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct ResolvedSet {
    pub items: Vec<ResolvedSetItem>,
}

#[derive(Debug, Clone)]
pub enum ResolvedSetItem {
    SetProperty {
        target: ResolvedExpr,
        value: ResolvedExpr,
    },
    SetVariable {
        variable: VarId,
        value: ResolvedExpr,
    },
    MutateVariable {
        variable: VarId,
        value: ResolvedExpr,
    },
    SetLabels {
        variable: VarId,
        labels: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ResolvedRemove {
    pub items: Vec<ResolvedRemoveItem>,
}

#[derive(Debug, Clone)]
pub enum ResolvedRemoveItem {
    Labels {
        variable: VarId,
        labels: Vec<String>,
    },
    Property {
        expr: ResolvedExpr,
    },
}

#[derive(Debug, Clone)]
pub struct ResolvedReturn {
    pub distinct: bool,
    pub items: Vec<ResolvedProjection>,
    pub include_existing: bool,
    pub order: Vec<ResolvedSortItem>,
    pub skip: Option<ResolvedExpr>,
    pub limit: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct ResolvedWith {
    pub distinct: bool,
    pub items: Vec<ResolvedProjection>,
    pub include_existing: bool,
    pub order: Vec<ResolvedSortItem>,
    pub skip: Option<ResolvedExpr>,
    pub limit: Option<ResolvedExpr>,
    pub where_: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct ResolvedProjection {
    pub expr: ResolvedExpr,
    pub output: VarId,
    pub name: String,
    /// True when the name came from an explicit `AS` alias.
    pub explicit_alias: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ResolvedSortItem {
    pub expr: ResolvedExpr,
    pub direction: SortDirection,
}

#[derive(Debug, Clone)]
pub struct ResolvedPattern {
    pub parts: Vec<ResolvedPatternPart>,
}

#[derive(Debug, Clone)]
pub struct ResolvedPatternPart {
    pub binding: Option<VarId>,
    pub element: ResolvedPatternElement,
}

#[derive(Debug, Clone)]
pub enum ResolvedPatternElement {
    Node {
        var: Option<VarId>,
        /// Each inner Vec is a disjunctive group (OR). Outer Vec is conjunctive (AND).
        labels: Vec<Vec<String>>,
        properties: Option<ResolvedExpr>,
    },
    NodeChain {
        head: ResolvedNode,
        chain: Vec<ResolvedChain>,
    },
    ShortestPath {
        all: bool,
        head: ResolvedNode,
        chain: Vec<ResolvedChain>,
    },
}

#[derive(Debug, Clone)]
pub struct ResolvedNode {
    pub var: Option<VarId>,
    /// Each inner Vec is a disjunctive group (OR). Outer Vec is conjunctive (AND).
    pub labels: Vec<Vec<String>>,
    pub properties: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct ResolvedChain {
    pub rel: ResolvedRel,
    pub node: ResolvedNode,
}

#[derive(Debug, Clone)]
pub struct ResolvedRel {
    pub var: Option<VarId>,
    pub types: Vec<String>,
    pub direction: Direction,
    pub range: Option<RangeLiteral>,
    pub properties: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub enum ResolvedExpr {
    Variable(VarId),
    Literal(LiteralValue),
    Property {
        expr: Box<ResolvedExpr>,
        property: String,
    },
    Binary {
        lhs: Box<ResolvedExpr>,
        op: BinaryOp,
        rhs: Box<ResolvedExpr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<ResolvedExpr>,
    },
    Function {
        name: String,
        distinct: bool,
        args: Vec<ResolvedExpr>,
    },
    List(Vec<ResolvedExpr>),
    Map(Vec<(String, ResolvedExpr)>),
    Case {
        input: Option<Box<ResolvedExpr>>,
        alternatives: Vec<(ResolvedExpr, ResolvedExpr)>,
        else_expr: Option<Box<ResolvedExpr>>,
    },
    Parameter(String),
    ListPredicate {
        kind: ListPredicateKind,
        variable: VarId,
        list: Box<ResolvedExpr>,
        predicate: Box<ResolvedExpr>,
    },
    ListComprehension {
        variable: VarId,
        list: Box<ResolvedExpr>,
        filter: Option<Box<ResolvedExpr>>,
        map_expr: Option<Box<ResolvedExpr>>,
    },
    Reduce {
        accumulator: VarId,
        init: Box<ResolvedExpr>,
        variable: VarId,
        list: Box<ResolvedExpr>,
        expr: Box<ResolvedExpr>,
    },
    MapProjection {
        base: Box<ResolvedExpr>,
        selectors: Vec<ResolvedMapSelector>,
    },
    Index {
        expr: Box<ResolvedExpr>,
        index: Box<ResolvedExpr>,
    },
    Slice {
        expr: Box<ResolvedExpr>,
        from: Option<Box<ResolvedExpr>>,
        to: Option<Box<ResolvedExpr>>,
    },
    ExistsSubquery {
        pattern: ResolvedPattern,
        where_: Option<Box<ResolvedExpr>>,
    },
    PatternComprehension {
        pattern: ResolvedPattern,
        where_: Option<Box<ResolvedExpr>>,
        map_expr: Box<ResolvedExpr>,
    },
}

#[derive(Debug, Clone)]
pub enum ResolvedMapSelector {
    Property(String),
    AllProperties,
    Literal(String, ResolvedExpr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    Integer(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Null,
}