use std::collections::BTreeSet;

use crate::logical::*;
use crate::planner::Planner;
use lora_analyzer::{
    symbols::VarId, FunctionId, LiteralValue, ResolvedChain, ResolvedExpr, ResolvedMapSelector,
    ResolvedNode, ResolvedPattern, ResolvedPatternElement, ResolvedPatternPart, ResolvedRel,
};
use lora_ast::{BinaryOp, Direction};

/// A WHERE conjunct waiting for the pattern variables it reads.
///
/// `needs` holds only variables introduced by the MATCH pattern itself;
/// everything else a conjunct reads (parameters, variables bound by
/// earlier clauses) is available from the first operator on. The
/// conjunct is placed in a `Filter` right after the operator that binds
/// the last variable of `needs`, so a key test on a node lands directly
/// on that node's scan, where the optimizer turns it into an index seek.
struct PendingConjunct {
    /// Position in the WHERE, so conjuncts keep their written order.
    order: usize,
    expr: ResolvedExpr,
    needs: BTreeSet<VarId>,
}

pub struct PatternPlanner<'a> {
    planner: &'a mut Planner,
    pending: Vec<PendingConjunct>,
    /// Pattern variables bound by the operators planned so far.
    available: BTreeSet<VarId>,
}

impl<'a> PatternPlanner<'a> {
    pub fn new(planner: &'a mut Planner) -> Self {
        Self {
            planner,
            pending: Vec::new(),
            available: BTreeSet::new(),
        }
    }

    /// Plan `pattern` with the WHERE split into conjuncts. Conjuncts that
    /// only read pattern variables, parameters and variables bound
    /// upstream are placed as early as their variables allow. The rest,
    /// together with anything never placed, is returned in WHERE order
    /// for the caller to apply above the whole pattern.
    pub fn plan_pattern_with_where(
        &mut self,
        input: Option<PlanNodeId>,
        pattern: &ResolvedPattern,
        where_: Option<&ResolvedExpr>,
    ) -> (PlanNodeId, Vec<ResolvedExpr>) {
        let pattern_vars: BTreeSet<VarId> = collect_pattern_var_set(pattern);
        let mut residual: Vec<(usize, ResolvedExpr)> = Vec::new();

        if let Some(where_) = where_ {
            for (order, conjunct) in split_conjuncts(where_).into_iter().enumerate() {
                let needs: BTreeSet<VarId> = match conjunct_vars(&conjunct) {
                    Some(vars) => vars.intersection(&pattern_vars).copied().collect(),
                    None => BTreeSet::new(),
                };
                // Conjuncts that read no pattern variable (or that hide
                // variables inside a subquery we do not inspect) stay
                // above the pattern, where the WHERE was written.
                if needs.is_empty() {
                    residual.push((order, conjunct));
                } else {
                    self.pending.push(PendingConjunct {
                        order,
                        expr: conjunct,
                        needs,
                    });
                }
            }
        }

        let node = self.plan_pattern(input, pattern);

        residual.extend(
            std::mem::take(&mut self.pending)
                .into_iter()
                .map(|p| (p.order, p.expr)),
        );
        residual.sort_by_key(|(order, _)| *order);
        (node, residual.into_iter().map(|(_, e)| e).collect())
    }

    pub fn plan_pattern(
        &mut self,
        input: Option<PlanNodeId>,
        pattern: &ResolvedPattern,
    ) -> PlanNodeId {
        let mut last = input;

        for part in &pattern.parts {
            last = Some(self.plan_part(last, part));
        }

        last.unwrap_or_else(|| {
            input.unwrap_or_else(|| self.planner.push(LogicalOp::Argument(Argument)))
        })
    }

    fn plan_part(&mut self, input: Option<PlanNodeId>, part: &ResolvedPatternPart) -> PlanNodeId {
        let shortest_path_all = match &part.element {
            ResolvedPatternElement::ShortestPath { all, .. } => Some(*all),
            _ => None,
        };

        let node = self.plan_element(input, part);

        // If the pattern part has a path binding, add a PathBuild operator.
        if let Some(path_var) = part.binding {
            let (node_vars, rel_vars) = collect_chain_vars(&part.element);
            if !node_vars.is_empty() {
                let node = self.planner.push(LogicalOp::PathBuild(PathBuild {
                    input: node,
                    output: path_var,
                    node_vars,
                    rel_vars,
                    shortest_path_all,
                }));
                self.available.insert(path_var);
                return self.attach_ready(node, Vec::new());
            }
        }

        node
    }

    fn plan_element(
        &mut self,
        input: Option<PlanNodeId>,
        part: &ResolvedPatternPart,
    ) -> PlanNodeId {
        match &part.element {
            ResolvedPatternElement::Node {
                var,
                labels,
                properties,
            } => self.plan_node(input, *var, labels, properties.as_ref()),

            ResolvedPatternElement::ShortestPath { head, chain, .. } => {
                self.plan_node_chain(input, head, chain)
            }

            ResolvedPatternElement::NodeChain { head, chain } => {
                // A path binding records the nodes in written order, and a
                // variable-length relationship binds its list in traversal
                // order, so only plain chains may be walked backwards.
                let reversible = part.binding.is_none()
                    && !chain.is_empty()
                    && chain.iter().all(|step| step.rel.range.is_none());
                if reversible && self.should_start_from_tail(head, chain) {
                    let (head, chain) = reverse_chain(head, chain);
                    self.plan_node_chain(input, &head, &chain)
                } else {
                    self.plan_node_chain(input, head, chain)
                }
            }
        }
    }

    fn plan_node_chain(
        &mut self,
        input: Option<PlanNodeId>,
        head: &ResolvedNode,
        chain: &[ResolvedChain],
    ) -> PlanNodeId {
        let mut node = self.plan_node(input, head.var, &head.labels, head.properties.as_ref());
        let mut current_src = assigned_node_var(head.var);

        for step in chain {
            let dst = assigned_node_var(step.node.var);
            node = self.plan_expand(node, current_src, dst, step);
            if let Some(rel) = step.rel.var {
                self.available.insert(rel);
            }
            self.available.insert(dst);

            // The expand only follows relationship types, so the step
            // node's own labels and inline properties are tested here.
            let mut checks = Vec::new();
            if let Some(check) = build_label_predicate(dst, &step.node.labels) {
                checks.push(check);
            }
            if let Some(props) = step.node.properties.as_ref() {
                if let Some(check) = build_property_predicate(dst, props) {
                    checks.push(check);
                }
            }
            node = self.attach_ready(node, checks);
            current_src = dst;
        }

        node
    }

    fn plan_node(
        &mut self,
        input: Option<PlanNodeId>,
        var: Option<VarId>,
        labels: &[Vec<String>],
        properties: Option<&ResolvedExpr>,
    ) -> PlanNodeId {
        let var = assigned_node_var(var);

        let node = self.planner.push(LogicalOp::NodeScan(NodeScan {
            input,
            var,
            labels: labels.to_vec(),
        }));
        self.available.insert(var);

        // Inline property predicates e.g. (a:User {id: 5}) and every WHERE
        // conjunct that only needs this node go into one Filter right on
        // the scan, where the optimizer can turn a key test into a seek.
        let mut checks = Vec::new();
        if let Some(props) = properties {
            if let Some(predicate) = build_property_predicate(var, props) {
                checks.push(predicate);
            }
        }
        self.attach_ready(node, checks)
    }

    fn plan_expand(
        &mut self,
        input: PlanNodeId,
        src: VarId,
        dst: VarId,
        step: &ResolvedChain,
    ) -> PlanNodeId {
        self.planner.push(LogicalOp::Expand(Expand {
            input,
            src,
            rel: step.rel.var,
            dst,
            types: step.rel.types.clone(),
            direction: step.rel.direction,
            rel_properties: step.rel.properties.clone(),
            range: step.rel.range.clone(),
        }))
    }

    /// Put `checks` plus every pending conjunct whose variables are now
    /// all bound into one `Filter` above `input`.
    fn attach_ready(&mut self, input: PlanNodeId, mut checks: Vec<ResolvedExpr>) -> PlanNodeId {
        let available = &self.available;
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|p| p.needs.is_subset(available));
        self.pending = waiting;
        checks.extend(ready.into_iter().map(|p| p.expr));

        match and_all(checks) {
            Some(predicate) => self
                .planner
                .push(LogicalOp::Filter(Filter { input, predicate })),
            None => input,
        }
    }

    /// Whether a chain should be walked from its last node instead of its
    /// first: true when the last node is clearly cheaper to start from
    /// (already bound, a key seek, or a smaller label).
    fn should_start_from_tail(&self, head: &ResolvedNode, chain: &[ResolvedChain]) -> bool {
        let (Some(first), Some(last)) = (chain.first(), chain.last()) else {
            return false;
        };
        let head_cost = self
            .start_cost(head)
            .min(self.rel_seek_cost(head, &first.rel));
        let tail_cost = self
            .start_cost(&last.node)
            .min(self.rel_seek_cost(&last.node, &last.rel));
        tail_cost < head_cost
    }

    /// Cost of starting at `end` when the chain's relationship next to it,
    /// `rel`, can be sought by id (`id(r) = value` pending): the optimizer
    /// turns `Expand(NodeScan(end))` into a relationship seek, one row.
    /// `u64::MAX` when it cannot.
    fn rel_seek_cost(&self, end: &ResolvedNode, rel: &ResolvedRel) -> u64 {
        let Some(rel_var) = rel.var else {
            return u64::MAX;
        };
        // Inline properties put a Filter between the scan and the expand,
        // which the seek rewrite does not look through.
        if end.properties.is_some() || rel.properties.is_some() || rel.range.is_some() {
            return u64::MAX;
        }
        if self.has_pending_id_seek(rel_var, crate::optimizer::REL_ID_FUNCTIONS) {
            2
        } else {
            u64::MAX
        }
    }

    /// Whether a pending conjunct `id(var) = value` / `id(var) IN list`
    /// could be evaluated once `var` is bound (the value reads only
    /// variables already available).
    fn has_pending_id_seek(&self, var: VarId, names: &[&str]) -> bool {
        self.pending.iter().any(|pending| {
            if !pending
                .needs
                .iter()
                .all(|v| *v == var || self.available.contains(v))
            {
                return false;
            }
            let ResolvedExpr::Binary { lhs, op, rhs } = &pending.expr else {
                return false;
            };
            let is_id = |e: &ResolvedExpr| crate::optimizer::is_id_of_var(e, var, names);
            match op {
                BinaryOp::Eq => is_id(lhs) || is_id(rhs),
                BinaryOp::In => is_id(lhs),
                _ => false,
            }
        })
    }

    /// Rough number of rows a scan of `node` would start with, if it were
    /// the first node planned from here.
    fn start_cost(&self, node: &ResolvedNode) -> u64 {
        let var = assigned_node_var(node.var);
        if self.available.contains(&var) || self.planner.is_bound(var) {
            return 0;
        }
        // `id(var) = value`: a seek by id, one row.
        if self.has_pending_id_seek(var, crate::optimizer::NODE_ID_FUNCTIONS) {
            return 2;
        }
        let stats = self.planner.stats();
        let label = match node.labels.as_slice() {
            [group, ..] if group.len() == 1 => Some(group[0].as_str()),
            _ => None,
        };
        let Some(label) = label else {
            return stats.node_count as u64 + 1;
        };

        let mut best = stats.label_count(label).unwrap_or(0);
        for key in self.seekable_keys(var, node) {
            // A distinct count exists for every key some node of the label
            // carries, indexed or not.
            let counted = stats
                .node_distinct_values
                .contains_key(&(label.to_string(), key.clone()));
            if counted {
                if let Some(rows) = stats.estimate_node_property_equality(label, &key) {
                    best = best.min(rows);
                }
            }
        }
        best + 1
    }

    /// Properties of `node` that are fixed by an inline map or by a
    /// pending `var.key = value` / `var.key IN list` conjunct that could
    /// be evaluated if the scan started at this node.
    fn seekable_keys(&self, var: VarId, node: &ResolvedNode) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(ResolvedExpr::Map(pairs)) = &node.properties {
            keys.extend(pairs.iter().map(|(k, _)| k.clone()));
        }
        for pending in &self.pending {
            if !pending
                .needs
                .iter()
                .all(|v| *v == var || self.available.contains(v))
            {
                continue;
            }
            let ResolvedExpr::Binary { lhs, op, rhs } = &pending.expr else {
                continue;
            };
            match op {
                BinaryOp::Eq => {
                    if let Some(key) = property_of(lhs, var).or_else(|| property_of(rhs, var)) {
                        keys.push(key);
                    }
                }
                BinaryOp::In => {
                    if let Some(key) = property_of(lhs, var) {
                        keys.push(key);
                    }
                }
                _ => {}
            }
        }
        keys
    }
}

fn property_of(expr: &ResolvedExpr, var: VarId) -> Option<String> {
    match expr {
        ResolvedExpr::Property { expr, property } => match expr.as_ref() {
            ResolvedExpr::Variable(v) if *v == var => Some(property.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// `(n0)-[r1]->(n1)-[r2]-(n2)` becomes `(n2)-[r2]-(n1)<-[r1]-(n0)`: the
/// same matches, walked from the other end.
fn reverse_chain(
    head: &ResolvedNode,
    chain: &[ResolvedChain],
) -> (ResolvedNode, Vec<ResolvedChain>) {
    let mut nodes: Vec<&ResolvedNode> = Vec::with_capacity(chain.len() + 1);
    nodes.push(head);
    nodes.extend(chain.iter().map(|step| &step.node));

    let new_head = (*nodes[chain.len()]).clone();
    let new_chain = (0..chain.len())
        .rev()
        .map(|i| ResolvedChain {
            rel: ResolvedRel {
                direction: flip_direction(chain[i].rel.direction),
                ..chain[i].rel.clone()
            },
            node: nodes[i].clone(),
        })
        .collect();
    (new_head, new_chain)
}

fn flip_direction(direction: Direction) -> Direction {
    match direction {
        Direction::Left => Direction::Right,
        Direction::Right => Direction::Left,
        other => other,
    }
}

/// Extract node and relationship VarIds from a pattern element for path construction.
fn collect_chain_vars(el: &ResolvedPatternElement) -> (Vec<VarId>, Vec<VarId>) {
    match el {
        ResolvedPatternElement::Node { var, .. } => {
            let node_vars = var.iter().copied().collect();
            (node_vars, Vec::new())
        }
        ResolvedPatternElement::ShortestPath { head, chain, .. }
        | ResolvedPatternElement::NodeChain { head, chain } => {
            let mut node_vars = Vec::new();
            let mut rel_vars = Vec::new();

            if let Some(v) = head.var {
                node_vars.push(v);
            }

            for step in chain {
                if let Some(v) = step.rel.var {
                    rel_vars.push(v);
                }
                if let Some(v) = step.node.var {
                    node_vars.push(v);
                }
            }

            (node_vars, rel_vars)
        }
    }
}

fn collect_pattern_var_set(pattern: &ResolvedPattern) -> BTreeSet<VarId> {
    let mut vars = BTreeSet::new();
    for part in &pattern.parts {
        if let Some(v) = part.binding {
            vars.insert(v);
        }
        match &part.element {
            ResolvedPatternElement::Node { var, .. } => vars.extend(*var),
            ResolvedPatternElement::ShortestPath { head, chain, .. }
            | ResolvedPatternElement::NodeChain { head, chain } => {
                vars.extend(head.var);
                for step in chain {
                    vars.extend(step.rel.var);
                    vars.extend(step.node.var);
                }
            }
        }
    }
    vars
}

fn assigned_node_var(var: Option<VarId>) -> VarId {
    var.expect("analyzer assigns a VarId to every node pattern")
}

/// Split an AND-tree into its conjuncts, left to right.
fn split_conjuncts(expr: &ResolvedExpr) -> Vec<ResolvedExpr> {
    let mut out = Vec::new();
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        match e {
            ResolvedExpr::Binary {
                lhs,
                op: BinaryOp::And,
                rhs,
            } => {
                stack.push(rhs);
                stack.push(lhs);
            }
            other => out.push(other.clone()),
        }
    }
    out
}

fn and_all(predicates: Vec<ResolvedExpr>) -> Option<ResolvedExpr> {
    predicates
        .into_iter()
        .reduce(|acc, next| ResolvedExpr::Binary {
            lhs: Box::new(acc),
            op: BinaryOp::And,
            rhs: Box::new(next),
        })
}

/// Every variable `expr` reads, or `None` when it contains a subquery or
/// pattern comprehension (whose pattern variables this walk does not
/// see), which keeps such a conjunct where it was written.
fn conjunct_vars(expr: &ResolvedExpr) -> Option<BTreeSet<VarId>> {
    let mut vars = BTreeSet::new();
    collect_expr_vars(expr, &mut vars).then_some(vars)
}

fn collect_expr_vars(expr: &ResolvedExpr, out: &mut BTreeSet<VarId>) -> bool {
    match expr {
        ResolvedExpr::Variable(v) => {
            out.insert(*v);
            true
        }
        ResolvedExpr::Literal(_) | ResolvedExpr::Parameter(_) => true,
        ResolvedExpr::Property { expr, .. } | ResolvedExpr::Unary { expr, .. } => {
            collect_expr_vars(expr, out)
        }
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            collect_expr_vars(lhs, out) && collect_expr_vars(rhs, out)
        }
        ResolvedExpr::Function { function, args, .. } => {
            !function.is_aggregate() && args.iter().all(|a| collect_expr_vars(a, out))
        }
        ResolvedExpr::List(items) => items.iter().all(|i| collect_expr_vars(i, out)),
        ResolvedExpr::Map(items) => items.iter().all(|(_, v)| collect_expr_vars(v, out)),
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            input.as_deref().is_none_or(|e| collect_expr_vars(e, out))
                && alternatives
                    .iter()
                    .all(|(w, t)| collect_expr_vars(w, out) && collect_expr_vars(t, out))
                && else_expr
                    .as_deref()
                    .is_none_or(|e| collect_expr_vars(e, out))
        }
        ResolvedExpr::ListPredicate {
            list, predicate, ..
        } => collect_expr_vars(list, out) && collect_expr_vars(predicate, out),
        ResolvedExpr::ListComprehension {
            list,
            filter,
            map_expr,
            ..
        } => {
            collect_expr_vars(list, out)
                && filter.as_deref().is_none_or(|e| collect_expr_vars(e, out))
                && map_expr
                    .as_deref()
                    .is_none_or(|e| collect_expr_vars(e, out))
        }
        ResolvedExpr::Reduce {
            init, list, expr, ..
        } => {
            collect_expr_vars(init, out)
                && collect_expr_vars(list, out)
                && collect_expr_vars(expr, out)
        }
        ResolvedExpr::MapProjection { base, selectors } => {
            collect_expr_vars(base, out)
                && selectors.iter().all(|sel| match sel {
                    ResolvedMapSelector::Literal(_, e) => collect_expr_vars(e, out),
                    ResolvedMapSelector::Property(_) | ResolvedMapSelector::AllProperties => true,
                })
        }
        ResolvedExpr::Index { expr, index } => {
            collect_expr_vars(expr, out) && collect_expr_vars(index, out)
        }
        ResolvedExpr::Slice { expr, from, to } => {
            collect_expr_vars(expr, out)
                && from.as_deref().is_none_or(|e| collect_expr_vars(e, out))
                && to.as_deref().is_none_or(|e| collect_expr_vars(e, out))
        }
        ResolvedExpr::ExistsSubquery { .. } | ResolvedExpr::PatternComprehension { .. } => false,
    }
}

/// `node.has_label(v, 'A') AND (node.has_label(v, 'B') OR ...)` for the
/// label groups of a node pattern (groups are ANDed, labels in a group
/// ORed), or `None` when the pattern names no labels.
fn build_label_predicate(var: VarId, groups: &[Vec<String>]) -> Option<ResolvedExpr> {
    let has_label = FunctionId::builtin("node.has_label")?;
    let group_predicates = groups.iter().filter_map(|group| {
        group
            .iter()
            .map(|label| ResolvedExpr::Function {
                function: has_label,
                distinct: false,
                args: vec![
                    ResolvedExpr::Variable(var),
                    ResolvedExpr::Literal(LiteralValue::String(label.clone())),
                ],
            })
            .reduce(|acc, next| ResolvedExpr::Binary {
                lhs: Box::new(acc),
                op: BinaryOp::Or,
                rhs: Box::new(next),
            })
    });
    and_all(group_predicates.collect())
}

fn build_property_predicate(var_id: VarId, props_expr: &ResolvedExpr) -> Option<ResolvedExpr> {
    let ResolvedExpr::Map(pairs) = props_expr else {
        return None;
    };

    let mut predicate: Option<ResolvedExpr> = None;

    for (key, value_expr) in pairs {
        let prop_access = ResolvedExpr::Property {
            expr: Box::new(ResolvedExpr::Variable(var_id)),
            property: key.clone(),
        };

        let eq = ResolvedExpr::Binary {
            lhs: Box::new(prop_access),
            op: BinaryOp::Eq,
            rhs: Box::new(value_expr.clone()),
        };

        predicate = Some(match predicate {
            None => eq,
            Some(existing) => ResolvedExpr::Binary {
                lhs: Box::new(existing),
                op: BinaryOp::And,
                rhs: Box::new(eq),
            },
        });
    }

    predicate
}
