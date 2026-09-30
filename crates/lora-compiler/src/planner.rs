use crate::pattern::PatternPlanner;
use crate::{
    Aggregation, Argument, Filter, Limit, LogicalOp, LogicalPlan, OptionalMatch, PlanNodeId,
    Projection, Sort, Unwind,
};
use lora_analyzer::symbols::VarId;
use lora_analyzer::{
    ResolvedCallSubquery, ResolvedClause, ResolvedCreate, ResolvedDelete, ResolvedExpr,
    ResolvedForeach, ResolvedMatch, ResolvedMerge, ResolvedPattern, ResolvedPatternElement,
    ResolvedProjection, ResolvedQuery, ResolvedRemove, ResolvedReturn, ResolvedSet,
    ResolvedSortItem, ResolvedUnwind, ResolvedWith,
};
use lora_store::GraphStats;
use std::collections::BTreeSet;

pub struct Planner {
    nodes: Vec<LogicalOp>,
    /// Cardinalities used to pick which end of a pattern to start from.
    stats: GraphStats,
    /// Variables bound by the clauses planned so far. Only a hint for
    /// choosing a pattern's starting point: a wrong entry costs speed,
    /// never correctness, because scans re-check bound variables.
    bound: BTreeSet<VarId>,
}

impl Default for Planner {
    fn default() -> Self {
        Self::new()
    }
}

impl Planner {
    pub fn new() -> Self {
        Self::with_stats(&GraphStats::default())
    }

    pub fn with_stats(stats: &GraphStats) -> Self {
        Self {
            nodes: Vec::new(),
            stats: stats.clone(),
            bound: BTreeSet::new(),
        }
    }

    pub(crate) fn push(&mut self, op: LogicalOp) -> PlanNodeId {
        let id = self.nodes.len();
        self.nodes.push(op);
        id
    }

    pub(crate) fn stats(&self) -> &GraphStats {
        &self.stats
    }

    pub(crate) fn is_bound(&self, var: VarId) -> bool {
        self.bound.contains(&var)
    }

    fn bind_projection(&mut self, items: &[ResolvedProjection], include_existing: bool) {
        if !include_existing {
            self.bound.clear();
        }
        self.bound.extend(items.iter().map(|item| item.output));
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
            input = Some(self.plan_clause(input, clause));
            self.track_bindings(clause);
        }

        input.unwrap_or_else(|| self.plan_unit_input())
    }

    /// Record the variables `clause` leaves bound for the clauses after it.
    fn track_bindings(&mut self, clause: &ResolvedClause) {
        match clause {
            ResolvedClause::Match(m) => self.bound.extend(pattern_binders(&m.pattern)),
            ResolvedClause::Create(c) => self.bound.extend(pattern_binders(&c.pattern)),
            ResolvedClause::Merge(m) => {
                let pattern = ResolvedPattern {
                    parts: vec![m.pattern_part.clone()],
                };
                self.bound.extend(pattern_binders(&pattern));
            }
            ResolvedClause::Unwind(u) => {
                self.bound.insert(u.alias);
            }
            ResolvedClause::With(w) => self.bind_projection(&w.items, w.include_existing),
            ResolvedClause::Return(r) => self.bind_projection(&r.items, r.include_existing),
            ResolvedClause::CallSubquery(c) => self.bound.extend(c.return_vars.iter().copied()),
            ResolvedClause::Delete(_)
            | ResolvedClause::Set(_)
            | ResolvedClause::Remove(_)
            | ResolvedClause::Foreach(_) => {}
        }
    }

    fn plan_clause(&mut self, input: Option<PlanNodeId>, clause: &ResolvedClause) -> PlanNodeId {
        {
            match clause {
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

                ResolvedClause::Foreach(f) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_foreach(upstream, f)
                }

                ResolvedClause::With(w) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_with(upstream, w)
                }

                ResolvedClause::Return(r) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_return(upstream, r)
                }

                ResolvedClause::CallSubquery(c) => {
                    let upstream = input.unwrap_or_else(|| self.plan_unit_input());
                    self.plan_call_subquery(upstream, c)
                }
            }
        }
    }

    /// Plan a `CALL { ... }` subquery: build the inner plan starting
    /// from a fresh `Argument`, then wrap it with `CallSubquery` to
    /// drive it per outer row.
    fn plan_call_subquery(&mut self, input: PlanNodeId, call: &ResolvedCallSubquery) -> PlanNodeId {
        let inner_query = ResolvedQuery {
            clauses: call.clauses.clone(),
            unions: Vec::new(),
        };
        // The inner query sees the outer row; its own WITH / RETURN must
        // not change what the outer query considers bound.
        let outer_bound = self.bound.clone();
        let inner = self.plan_query(&inner_query);
        self.bound = outer_bound;
        self.push(LogicalOp::CallSubquery(crate::logical::CallSubquery {
            input,
            inner,
            new_vars: call.return_vars.clone(),
        }))
    }

    fn plan_match(&mut self, input: Option<PlanNodeId>, m: &ResolvedMatch) -> PlanNodeId {
        if let (true, Some(upstream)) = (m.optional, input) {
            // OPTIONAL MATCH: build the inner sub-plan that reads from Argument,
            // then wrap it in an OptionalMatch node that provides null-extension.

            // Collect variables introduced by this pattern (for null-extension).
            let new_vars = pattern_binders(&m.pattern);

            // Build inner match plan WITHOUT the upstream input — the executor
            // will inject each upstream row individually. The WHERE belongs
            // to the OPTIONAL MATCH, so its conjuncts are only ever placed
            // inside this inner plan, never above the OptionalMatch.
            let mut pattern_planner = PatternPlanner::new(self);
            let (inner, residual) =
                pattern_planner.plan_pattern_with_where(None, &m.pattern, m.where_.as_ref());
            let inner = self.push_conjuncts(inner, residual);

            self.push(LogicalOp::OptionalMatch(OptionalMatch {
                input: upstream,
                inner,
                new_vars,
            }))
        } else {
            let mut pattern_planner = PatternPlanner::new(self);
            let (node, residual) =
                pattern_planner.plan_pattern_with_where(input, &m.pattern, m.where_.as_ref());
            self.push_conjuncts(node, residual)
        }
    }

    /// A `Filter` over `input` holding `conjuncts` ANDed in order, or
    /// `input` itself when there are none.
    fn push_conjuncts(&mut self, input: PlanNodeId, conjuncts: Vec<ResolvedExpr>) -> PlanNodeId {
        let predicate = conjuncts
            .into_iter()
            .reduce(|acc, next| ResolvedExpr::Binary {
                lhs: Box::new(acc),
                op: lora_ast::BinaryOp::And,
                rhs: Box::new(next),
            });
        match predicate {
            Some(predicate) => self.push(LogicalOp::Filter(Filter { input, predicate })),
            None => input,
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

    fn plan_foreach(&mut self, input: PlanNodeId, f: &ResolvedForeach) -> PlanNodeId {
        self.push(LogicalOp::Foreach(crate::Foreach {
            input,
            variable: f.variable,
            list: f.list.clone(),
            body: f.body.clone(),
        }))
    }

    fn plan_with(&mut self, input: PlanNodeId, with: &ResolvedWith) -> PlanNodeId {
        let mut node = self.plan_projection_sort_limit(
            input,
            &with.items,
            with.distinct,
            with.include_existing,
            &with.order,
            &with.skip,
            &with.limit,
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
        self.plan_projection_sort_limit(
            input,
            &ret.items,
            ret.distinct,
            ret.include_existing,
            &ret.order,
            &ret.skip,
            &ret.limit,
        )
    }

    /// Plan `items [ORDER BY ...] [SKIP ...] [LIMIT ...]` for WITH / RETURN.
    ///
    /// Cypher evaluates projection (with aggregation and DISTINCT) first,
    /// then ORDER BY, then SKIP / LIMIT. Sort keys may name projected
    /// aliases (`RETURN p.name AS name ORDER BY name`) and, unless the
    /// projection aggregates or deduplicates, pre-projection variables too
    /// (`RETURN p.name AS name ORDER BY p.age`).
    ///
    /// * Aggregation or DISTINCT changes the row set, so both must run
    ///   before sorting and limiting; otherwise `LIMIT 1` would cut the
    ///   input to one row before counting, and DISTINCT would dedupe an
    ///   already-truncated list. Sort keys that restate a projected
    ///   expression are pointed at that expression's output column.
    /// * A plain projection maps rows 1:1. It first projects while keeping
    ///   the input bindings, so keys can use both aliases and original
    ///   variables, then sorts and limits, then trims the row down to the
    ///   projected columns.
    #[allow(clippy::too_many_arguments)]
    fn plan_projection_sort_limit(
        &mut self,
        input: PlanNodeId,
        items: &[ResolvedProjection],
        distinct: bool,
        include_existing: bool,
        order: &[ResolvedSortItem],
        skip: &Option<ResolvedExpr>,
        limit: &Option<ResolvedExpr>,
    ) -> PlanNodeId {
        let aggregates = items.iter().any(|item| expr_contains_aggregate(&item.expr));
        let has_order = !order.is_empty();
        let has_limit = skip.is_some() || limit.is_some();

        if aggregates || distinct {
            let mut node =
                self.plan_projection_or_aggregation(input, items, distinct, include_existing);
            if has_order {
                node = self.push(LogicalOp::Sort(Sort {
                    input: node,
                    items: sort_keys_on_outputs(order, items),
                    top_k: None,
                }));
            }
            if has_limit {
                node = self.push(LogicalOp::Limit(Limit {
                    input: node,
                    skip: skip.clone(),
                    limit: limit.clone(),
                }));
            }
            return node;
        }

        if !has_order {
            // LIMIT on a 1:1 projection can run first and saves projecting
            // rows that would be dropped.
            let mut node = input;
            if has_limit {
                node = self.push(LogicalOp::Limit(Limit {
                    input: node,
                    skip: skip.clone(),
                    limit: limit.clone(),
                }));
            }
            return self.plan_projection_or_aggregation(node, items, false, include_existing);
        }

        let mut node = self.push(LogicalOp::Projection(Projection {
            input,
            distinct: false,
            items: items.to_vec(),
            include_existing: true,
        }));
        node = self.push(LogicalOp::Sort(Sort {
            input: node,
            items: order.to_vec(),
            top_k: None,
        }));
        if has_limit {
            node = self.push(LogicalOp::Limit(Limit {
                input: node,
                skip: skip.clone(),
                limit: limit.clone(),
            }));
        }
        if include_existing {
            // WITH * / RETURN * keep every binding anyway.
            return node;
        }
        self.push(LogicalOp::Projection(Projection {
            input: node,
            distinct: false,
            items: passthrough_items(items),
            include_existing: false,
        }))
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
            self.push(LogicalOp::Projection(Projection {
                input: node,
                distinct: true,
                items: passthrough_items(items),
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

/// Projection items that re-emit each item's own output column.
fn passthrough_items(items: &[ResolvedProjection]) -> Vec<ResolvedProjection> {
    items
        .iter()
        .map(|item| ResolvedProjection {
            expr: ResolvedExpr::Variable(item.output),
            output: item.output,
            name: item.name.clone(),
            explicit_alias: item.explicit_alias,
            span: item.span,
        })
        .collect()
}

/// Rewrite sort keys for a sort that runs after aggregation / DISTINCT,
/// where only the projected columns exist. A key that restates a
/// projected expression (`RETURN n.v AS v, count(*) AS c ORDER BY
/// count(*)`) is pointed at that expression's output column. Alias keys
/// already resolve to output columns in the analyzer.
///
/// Expressions are compared by their derived `Debug` form: structural,
/// deterministic, and only paid once at planning time.
fn sort_keys_on_outputs(
    order: &[ResolvedSortItem],
    items: &[ResolvedProjection],
) -> Vec<ResolvedSortItem> {
    let projected: Vec<(String, VarId)> = items
        .iter()
        .map(|item| (format!("{:?}", item.expr), item.output))
        .collect();
    order
        .iter()
        .map(|key| {
            let shape = format!("{:?}", key.expr);
            match projected.iter().find(|(expr, _)| *expr == shape) {
                Some((_, output)) => ResolvedSortItem {
                    expr: ResolvedExpr::Variable(*output),
                    direction: key.direction,
                },
                None => key.clone(),
            }
        })
        .collect()
}

/// The VarIds a pattern binds (path, node and relationship variables); unlike
/// `ResolvedPattern::collect_vars`, not the variables its expressions read.
fn pattern_binders(pattern: &ResolvedPattern) -> Vec<VarId> {
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
        ResolvedExpr::Function { function, args, .. } => {
            if function.is_aggregate() {
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
