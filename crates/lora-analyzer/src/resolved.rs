use std::collections::BTreeSet;

use crate::{analyzer::FunctionId, symbols::*};
use lora_ast::{
    BinaryOp, Direction, ListPredicateKind, RangeLiteral, SortDirection, Span, UnaryOp,
};

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
    Foreach(ResolvedForeach),
    Return(ResolvedReturn),
    With(ResolvedWith),
    CallSubquery(ResolvedCallSubquery),
}

/// `CALL { ... }` subquery body. The inner clause list reads from
/// the outer scope (variables visible at the call site are still
/// visible inside the subquery), runs once per outer row, and
/// projects the variables named in its final RETURN back into the
/// outer scope.
#[derive(Debug, Clone)]
pub struct ResolvedCallSubquery {
    pub clauses: Vec<ResolvedClause>,
    /// VarIds produced by the inner final RETURN that become
    /// available in the outer scope after the CALL.
    pub return_vars: Vec<VarId>,
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

/// `FOREACH (var IN list | body...)`. The body is restricted to
/// updating clauses (Create / Merge / Delete / Set / Remove / nested
/// Foreach). The analyzer enforces that restriction; the planner /
/// executor treat each body item as a side-effect-only operation
/// applied row-by-row inside the iteration.
#[derive(Debug, Clone)]
pub struct ResolvedForeach {
    pub variable: VarId,
    pub list: ResolvedExpr,
    pub body: Vec<ResolvedClause>,
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
    /// Aggregate calls nested inside a larger item (`size(collect(x))`,
    /// `count(*) + 1`), each lifted into its own hidden column. Those items
    /// read the lifted columns, and grouping keys, by output id, so they
    /// are evaluated after aggregation.
    pub lifted_aggregates: Vec<ResolvedProjection>,
    pub include_existing: bool,
    pub order: Vec<ResolvedSortItem>,
    pub skip: Option<ResolvedExpr>,
    pub limit: Option<ResolvedExpr>,
}

#[derive(Debug, Clone)]
pub struct ResolvedWith {
    pub distinct: bool,
    pub items: Vec<ResolvedProjection>,
    /// Aggregate calls nested inside a larger item (`size(collect(x))`,
    /// `count(*) + 1`), each lifted into its own hidden column. Those items
    /// read the lifted columns, and grouping keys, by output id, so they
    /// are evaluated after aggregation.
    pub lifted_aggregates: Vec<ResolvedProjection>,
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
    /// Output column name. `Arc<str>` because executors stamp it onto
    /// every produced row; cloning it per row is a refcount bump rather
    /// than a heap allocation per cell.
    pub name: std::sync::Arc<str>,
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
        function: FunctionId,
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
        /// Every variable the pattern and WHERE use (see
        /// [`ResolvedExpr::collect_vars`]), sorted: the outer bindings the
        /// subquery can read, computed once so evaluation need not walk it.
        reads: Vec<VarId>,
    },
    PatternComprehension {
        pattern: ResolvedPattern,
        where_: Option<Box<ResolvedExpr>>,
        map_expr: Box<ResolvedExpr>,
        /// Every variable the pattern, WHERE and projection use, as for
        /// [`ResolvedExpr::ExistsSubquery`].
        reads: Vec<VarId>,
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
    TypeName(String),
    Bool(bool),
    Null,
}

impl ResolvedExpr {
    /// Every variable the expression reads. Patterns inside it (`EXISTS`,
    /// pattern comprehensions) contribute the outer variables they name and
    /// their own fresh ones: an over-approximation, which is safe for every
    /// caller (it only ever keeps a predicate in place, or a binding in a
    /// row).
    pub fn collect_vars(&self, out: &mut BTreeSet<VarId>) {
        let expr = self;
        match expr {
            ResolvedExpr::Variable(v) => {
                out.insert(*v);
            }
            ResolvedExpr::Property { expr, .. } => ResolvedExpr::collect_vars(expr, out),
            ResolvedExpr::Binary { lhs, rhs, .. } => {
                ResolvedExpr::collect_vars(lhs, out);
                ResolvedExpr::collect_vars(rhs, out);
            }
            ResolvedExpr::Unary { expr, .. } => ResolvedExpr::collect_vars(expr, out),
            ResolvedExpr::Function { args, .. } => {
                for arg in args {
                    ResolvedExpr::collect_vars(arg, out);
                }
            }
            ResolvedExpr::List(items) => {
                for item in items {
                    ResolvedExpr::collect_vars(item, out);
                }
            }
            ResolvedExpr::Map(items) => {
                for (_, v) in items {
                    ResolvedExpr::collect_vars(v, out);
                }
            }
            ResolvedExpr::Case {
                input,
                alternatives,
                else_expr,
            } => {
                if let Some(e) = input {
                    ResolvedExpr::collect_vars(e, out);
                }
                for (w, t) in alternatives {
                    ResolvedExpr::collect_vars(w, out);
                    ResolvedExpr::collect_vars(t, out);
                }
                if let Some(e) = else_expr {
                    ResolvedExpr::collect_vars(e, out);
                }
            }
            ResolvedExpr::ListPredicate {
                variable,
                list,
                predicate,
                ..
            } => {
                out.insert(*variable);
                ResolvedExpr::collect_vars(list, out);
                ResolvedExpr::collect_vars(predicate, out);
            }
            ResolvedExpr::ListComprehension {
                variable,
                list,
                filter,
                map_expr,
                ..
            } => {
                out.insert(*variable);
                ResolvedExpr::collect_vars(list, out);
                if let Some(f) = filter {
                    ResolvedExpr::collect_vars(f, out);
                }
                if let Some(m) = map_expr {
                    ResolvedExpr::collect_vars(m, out);
                }
            }
            ResolvedExpr::Reduce {
                accumulator,
                init,
                variable,
                list,
                expr,
                ..
            } => {
                out.insert(*accumulator);
                out.insert(*variable);
                ResolvedExpr::collect_vars(init, out);
                ResolvedExpr::collect_vars(list, out);
                ResolvedExpr::collect_vars(expr, out);
            }
            ResolvedExpr::Index { expr, index } => {
                ResolvedExpr::collect_vars(expr, out);
                ResolvedExpr::collect_vars(index, out);
            }
            ResolvedExpr::Slice { expr, from, to } => {
                ResolvedExpr::collect_vars(expr, out);
                if let Some(f) = from {
                    ResolvedExpr::collect_vars(f, out);
                }
                if let Some(t) = to {
                    ResolvedExpr::collect_vars(t, out);
                }
            }
            ResolvedExpr::MapProjection { base, selectors } => {
                ResolvedExpr::collect_vars(base, out);
                for sel in selectors {
                    if let ResolvedMapSelector::Literal(_, e) = sel {
                        ResolvedExpr::collect_vars(e, out);
                    }
                }
            }
            // A pattern reads the outer variables it names (`(a)<-[:T]-(x)`
            // reads `a`). Its own fresh variables are collected too: an
            // over-approximation, which only ever keeps a predicate in place.
            ResolvedExpr::ExistsSubquery {
                pattern, where_, ..
            } => {
                pattern.collect_vars(out);
                if let Some(w) = where_ {
                    ResolvedExpr::collect_vars(w, out);
                }
            }
            ResolvedExpr::PatternComprehension {
                pattern,
                where_,
                map_expr,
                ..
            } => {
                pattern.collect_vars(out);
                if let Some(w) = where_ {
                    ResolvedExpr::collect_vars(w, out);
                }
                ResolvedExpr::collect_vars(map_expr, out);
            }
            ResolvedExpr::Literal(_) | ResolvedExpr::Parameter(_) => {}
        }
    }
}

impl ResolvedPattern {
    /// Every variable the pattern binds or names, and those its property
    /// maps read.
    pub fn collect_vars(&self, out: &mut BTreeSet<VarId>) {
        let pattern = self;
        let node = |n: &ResolvedNode, out: &mut BTreeSet<VarId>| {
            out.extend(n.var);
            if let Some(p) = &n.properties {
                ResolvedExpr::collect_vars(p, out);
            }
        };
        for part in &pattern.parts {
            out.extend(part.binding);
            match &part.element {
                ResolvedPatternElement::Node {
                    var, properties, ..
                } => {
                    out.extend(*var);
                    if let Some(p) = properties {
                        ResolvedExpr::collect_vars(p, out);
                    }
                }
                ResolvedPatternElement::NodeChain { head, chain }
                | ResolvedPatternElement::ShortestPath { head, chain, .. } => {
                    node(head, out);
                    for link in chain {
                        out.extend(link.rel.var);
                        if let Some(p) = &link.rel.properties {
                            ResolvedExpr::collect_vars(p, out);
                        }
                        node(&link.node, out);
                    }
                }
            }
        }
    }
}
