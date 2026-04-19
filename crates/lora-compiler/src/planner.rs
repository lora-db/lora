use crate::pattern::PatternPlanner;
use crate::{
    Aggregation, Argument, Filter, Limit, LogicalOp, LogicalPlan, OptionalMatch, PlanNodeId,
    Projection, Sort, Unwind,
};
use lora_analyzer::symbols::VarId;
use lora_analyzer::{
    ResolvedClause, ResolvedCreate, ResolvedDelete, ResolvedExpr, ResolvedMatch, ResolvedMerge,
    ResolvedPattern, ResolvedPatternElement, ResolvedProjection, ResolvedQuery, ResolvedRemove,
    ResolvedReturn, ResolvedSet, ResolvedUnwind, ResolvedWith,
};

pub struct Planner {
    nodes: Vec<LogicalOp>,
}

impl Default for Planner {
    fn default() -> Self {
        Self::new()
    }
}

impl Planner {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub(crate) fn push(&mut self, op: LogicalOp) -> PlanNodeId {
        let id = self.nodes.len();
        self.nodes.push(op);
        id
    }

    pub fn plan(&mut self, query: &ResolvedQuery) -> LogicalPlan {
        let root = self.plan_query(query);

        LogicalPlan {
            root,
            nodes: std::mem::take(&mut self.nodes),
        }
    }

    fn plan_query(&mut self, query: &ResolvedQuery) -> PlanNodeId {
        let mut input = None;

        for clause in &query.clauses {
            input = Some(match clause {
                ResolvedClause::Match(m) => self.plan_match(input, m),

                ResolvedClause::Unwind(u) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_unwind(upstream, u)
                }

                ResolvedClause::Create(c) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_create(upstream, c)
                }

                ResolvedClause::Merge(m) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_merge(upstream, m)
                }

                ResolvedClause::Delete(d) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_delete(upstream, d)
                }

                ResolvedClause::Set(s) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_set(upstream, s)
                }

                ResolvedClause::Remove(rm) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_remove(upstream, rm)
                }

                ResolvedClause::With(w) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_with(upstream, w)
                }

                ResolvedClause::Return(r) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_return(upstream, r)
                }
            });
        }

        input.unwrap_or_else(|| self.plan_unit_input())
    }

    fn plan_match(&mut self, input: Option<PlanNodeId>, m: &ResolvedMatch) -> PlanNodeId {
        if let (true, Some(upstream)) = (m.optional, input) {
            // OPTIONAL MATCH: build the inner sub-plan that reads from Argument,
            // then wrap it in an OptionalMatch node that provides null-extension.

            // Collect variables introduced by this pattern (for null-extension).
            let new_vars = collect_pattern_vars(&m.pattern);

            // Build inner match plan WITHOUT the upstream input — the executor
            // will inject each upstream row individually.
            let mut pattern_planner = PatternPlanner::new(self);
            let mut inner = pattern_planner.plan_pattern(None, &m.pattern);

            if let Some(pred) = &m.where_ {
                inner = self.push(LogicalOp::Filter(Filter {
                    input: inner,
                    predicate: pred.clone(),
                }));
            }

            self.push(LogicalOp::OptionalMatch(OptionalMatch {
                input: upstream,
                inner,
                new_vars,
            }))
        } else {
            let mut pattern_planner = PatternPlanner::new(self);
            let mut node = pattern_planner.plan_pattern(input, &m.pattern);

            if let Some(pred) = &m.where_ {
                node = self.push(LogicalOp::Filter(Filter {
                    input: node,
                    predicate: pred.clone(),
                }));
            }

            node
        }
    }

    fn plan_unwind(&mut self, input: PlanNodeId, u: &ResolvedUnwind) -> PlanNodeId {
        self.push(LogicalOp::Unwind(Unwind {
            input,
            expr: u.expr.clone(),
            alias: u.alias,
        }))
    }

    fn plan_create(&mut self, input: PlanNodeId, c: &ResolvedCreate) -> PlanNodeId {
        self.push(LogicalOp::Create(crate::Create {
            input,
            pattern: c.pattern.clone(),
        }))
    }

    fn plan_merge(&mut self, input: PlanNodeId, m: &ResolvedMerge) -> PlanNodeId {
        self.push(LogicalOp::Merge(crate::Merge {
            input,
            pattern_part: m.pattern_part.clone(),
            actions: m.actions.clone(),
        }))
    }

    fn plan_delete(&mut self, input: PlanNodeId, d: &ResolvedDelete) -> PlanNodeId {
        self.push(LogicalOp::Delete(crate::Delete {
            input,
            detach: d.detach,
            expressions: d.expressions.clone(),
        }))
    }

    fn plan_set(&mut self, input: PlanNodeId, s: &ResolvedSet) -> PlanNodeId {
        self.push(LogicalOp::Set(crate::Set {
            input,
            items: s.items.clone(),
        }))
    }

    fn plan_remove(&mut self, input: PlanNodeId, r: &ResolvedRemove) -> PlanNodeId {
        self.push(LogicalOp::Remove(crate::Remove {
            input,
            items: r.items.clone(),
        }))
    }

    fn plan_with(&mut self, input: PlanNodeId, with: &ResolvedWith) -> PlanNodeId {
        let mut node = input;

        // Sort before projection so sort expressions can access original variables.
        if !with.order.is_empty() {
            node = self.push(LogicalOp::Sort(Sort {
                input: node,
                items: with.order.clone(),
            }));
        }

        if with.skip.is_some() || with.limit.is_some() {
            node = self.push(LogicalOp::Limit(Limit {
                input: node,
                skip: with.skip.clone(),
                limit: with.limit.clone(),
            }));
        }

        node = self.plan_projection_or_aggregation(
            node,
            &with.items,
            with.distinct,
            with.include_existing,
        );

        if let Some(pred) = &with.where_ {
            node = self.push(LogicalOp::Filter(Filter {
                input: node,
                predicate: pred.clone(),
            }));
        }

        node
    }

    fn plan_return(&mut self, input: PlanNodeId, ret: &ResolvedReturn) -> PlanNodeId {
        let mut node = input;

        // Sort must happen BEFORE projection so that the sort expressions
        // can access the original variables (e.g. n.name) which are not
        // available after projection replaces the row with output VarIds.
        if !ret.order.is_empty() {
            node = self.push(LogicalOp::Sort(Sort {
                input: node,
                items: ret.order.clone(),
            }));
        }

        if ret.skip.is_some() || ret.limit.is_some() {
            node = self.push(LogicalOp::Limit(Limit {
                input: node,
                skip: ret.skip.clone(),
                limit: ret.limit.clone(),
            }));
        }

        node = self.plan_projection_or_aggregation(
            node,
            &ret.items,
            ret.distinct,
            ret.include_existing,
        );

        node
    }

    /// If any projection item contains an aggregate function, emit an
    /// Aggregation node followed by a Projection. Otherwise emit a plain
    /// Projection.
    fn plan_projection_or_aggregation(
        &mut self,
        input: PlanNodeId,
        items: &[ResolvedProjection],
        distinct: bool,
        include_existing: bool,
    ) -> PlanNodeId {
        let has_aggregates = items.iter().any(|item| expr_contains_aggregate(&item.expr));

        if !has_aggregates {
            return self.push(LogicalOp::Projection(Projection {
                input,
                distinct,
                items: items.to_vec(),
                include_existing,
            }));
        }

        // Split items into group-by keys and aggregate expressions.
        let mut group_by = Vec::new();
        let mut aggregates = Vec::new();

        for item in items {
            if expr_contains_aggregate(&item.expr) {
                aggregates.push(item.clone());
            } else {
                group_by.push(item.clone());
            }
        }

        let node = self.push(LogicalOp::Aggregation(Aggregation {
            input,
            group_by: group_by.clone(),
            aggregates: aggregates.clone(),
        }));

        // After aggregation the row already contains the right VarIds and names,
        // but we still emit a Projection to handle DISTINCT and to ensure the
        // final column order matches the original item list. The projection uses
        // include_existing=true so it picks up the aggregation output, and each
        // item just reads its own output variable.
        //
        // However, since the aggregation node already produces correctly-named
        // rows, we can skip the extra projection when not needed.
        if distinct {
            // For DISTINCT we still need the dedup pass in exec_projection.
            let passthrough_items: Vec<ResolvedProjection> = items
                .iter()
                .map(|item| ResolvedProjection {
                    expr: ResolvedExpr::Variable(item.output),
                    output: item.output,
                    name: item.name.clone(),
                    explicit_alias: item.explicit_alias,
                    span: item.span,
                })
                .collect();
            self.push(LogicalOp::Projection(Projection {
                input: node,
                distinct: true,
                items: passthrough_items,
                include_existing: false,
            }))
        } else {
            node
        }
    }

    fn plan_unit_input(&mut self) -> PlanNodeId {
        self.push(LogicalOp::Argument(Argument))
    }
}

const AGGREGATE_FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "collect",
    "stdev",
    "stdevp",
    "percentilecont",
    "percentiledisc",
];

fn is_aggregate_function(name: &str) -> bool {
    AGGREGATE_FUNCTIONS
        .iter()
        .any(|&f| f.eq_ignore_ascii_case(name))
}

/// Collect all VarIds introduced by a pattern (node vars, relationship vars).
fn collect_pattern_vars(pattern: &ResolvedPattern) -> Vec<VarId> {
    let mut vars = Vec::new();
    for part in &pattern.parts {
        if let Some(v) = part.binding {
            vars.push(v);
        }
        match &part.element {
            ResolvedPatternElement::Node { var, .. } => {
                if let Some(v) = var {
                    vars.push(*v);
                }
            }
            ResolvedPatternElement::ShortestPath { head, chain, .. }
            | ResolvedPatternElement::NodeChain { head, chain } => {
                if let Some(v) = head.var {
                    vars.push(v);
                }
                for step in chain {
                    if let Some(v) = step.rel.var {
                        vars.push(v);
                    }
                    if let Some(v) = step.node.var {
                        vars.push(v);
                    }
                }
            }
        }
    }
    vars
}

fn expr_contains_aggregate(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Function { name, args, .. } => {
            if is_aggregate_function(name) {
                return true;
            }
            args.iter().any(expr_contains_aggregate)
        }
        ResolvedExpr::Property { expr, .. } => expr_contains_aggregate(expr),
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            expr_contains_aggregate(lhs) || expr_contains_aggregate(rhs)
        }
        ResolvedExpr::Unary { expr, .. } => expr_contains_aggregate(expr),
        ResolvedExpr::List(items) => items.iter().any(expr_contains_aggregate),
        ResolvedExpr::Map(items) => items.iter().any(|(_, v)| expr_contains_aggregate(v)),
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            input.as_ref().is_some_and(|e| expr_contains_aggregate(e))
                || alternatives
                    .iter()
                    .any(|(w, t)| expr_contains_aggregate(w) || expr_contains_aggregate(t))
                || else_expr
                    .as_ref()
                    .is_some_and(|e| expr_contains_aggregate(e))
        }
        _ => false,
    }
}
