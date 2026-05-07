use crate::logical::*;
use crate::physical::*;
use lora_analyzer::{symbols::VarId, LiteralValue, ResolvedExpr};
use lora_ast::BinaryOp;
use std::collections::BTreeSet;

pub struct Optimizer;

impl Default for Optimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Optimizer {
    pub fn new() -> Self {
        Self
    }

    pub fn optimize(&mut self, mut plan: LogicalPlan) -> LogicalPlan {
        self.push_filter_below_projection(&mut plan);
        self.use_property_indexed_node_scans(&mut plan);
        self.annotate_top_k_sorts(&mut plan);
        self.remove_redundant_limit(&mut plan);
        plan
    }

    fn push_filter_below_projection(&self, plan: &mut LogicalPlan) {
        let len = plan.nodes.len();

        for i in 0..len {
            let input_id = match &plan.nodes[i] {
                LogicalOp::Filter(f) => f.input,
                _ => continue,
            };

            if !can_push_filter_below_projection(&plan.nodes[i], &plan.nodes[input_id]) {
                continue;
            }

            push_filter_below_projection_at(plan, i, input_id);
        }
    }

    fn remove_redundant_limit(&self, _plan: &mut LogicalPlan) {
        // placeholder for future rules
    }

    fn annotate_top_k_sorts(&self, plan: &mut LogicalPlan) {
        let len = plan.nodes.len();
        for i in 0..len {
            let Some((input, bound)) = limit_sort_bound(&plan.nodes[i]) else {
                continue;
            };

            if let Some(sort) = sort_op_mut(&mut plan.nodes[input]) {
                sort.top_k = merge_top_k_bound(sort.top_k, bound);
            }
        }
    }

    fn use_property_indexed_node_scans(&self, plan: &mut LogicalPlan) {
        let len = plan.nodes.len();

        for i in 0..len {
            let (input_id, predicate) = match &plan.nodes[i] {
                LogicalOp::Filter(f) => (f.input, &f.predicate),
                _ => continue,
            };

            let (var, key, value) =
                match property_equality_candidate(predicate, &plan.nodes[input_id]) {
                    Some(candidate) => candidate,
                    None => continue,
                };

            let replacement = match &plan.nodes[input_id] {
                LogicalOp::NodeScan(scan) => {
                    Some(LogicalOp::NodeByPropertyScan(NodeByPropertyScan {
                        input: scan.input,
                        var,
                        labels: scan.labels.clone(),
                        key,
                        value,
                    }))
                }
                LogicalOp::NodeByPropertyScan(_) => None,
                _ => None,
            };

            if let Some(replacement) = replacement {
                plan.nodes[input_id] = replacement;
            }
        }
    }

    /// Lower a logical plan by consuming it — each op's owned payload
    /// (expressions, patterns, items) is moved into the physical op rather
    /// than cloned. Callers should not need the logical plan after this.
    pub fn lower_to_physical(&mut self, logical: LogicalPlan) -> PhysicalPlan {
        let LogicalPlan { root, nodes } = logical;

        let nodes = nodes.into_iter().map(lower_logical_op).collect();

        PhysicalPlan { root, nodes }
    }
}

fn can_push_filter_below_projection(filter: &LogicalOp, input: &LogicalOp) -> bool {
    let (LogicalOp::Filter(filter), LogicalOp::Projection(proj)) = (filter, input) else {
        return false;
    };

    if proj.distinct || proj.include_existing {
        return false;
    }

    let output_vars: BTreeSet<VarId> = proj.items.iter().map(|item| item.output).collect();
    let pred_vars = collect_vars(&filter.predicate);
    !pred_vars.iter().any(|v| output_vars.contains(v))
}

fn push_filter_below_projection_at(
    plan: &mut LogicalPlan,
    filter_id: PlanNodeId,
    projection_id: PlanNodeId,
) {
    let placeholder = || LogicalOp::Argument(Argument);
    let filter = match std::mem::replace(&mut plan.nodes[filter_id], placeholder()) {
        LogicalOp::Filter(f) => f,
        _ => unreachable!(),
    };
    let proj = match std::mem::replace(&mut plan.nodes[projection_id], placeholder()) {
        LogicalOp::Projection(p) => p,
        _ => unreachable!(),
    };

    plan.nodes[projection_id] = LogicalOp::Filter(Filter {
        input: proj.input,
        predicate: filter.predicate,
    });
    plan.nodes[filter_id] = LogicalOp::Projection(Projection {
        input: projection_id,
        distinct: proj.distinct,
        items: proj.items,
        include_existing: proj.include_existing,
    });
}

fn lower_logical_op(op: LogicalOp) -> PhysicalOp {
    match op {
        LogicalOp::Argument(_) => PhysicalOp::Argument(ArgumentExec),

        LogicalOp::NodeScan(scan) => lower_node_scan(scan),

        LogicalOp::NodeByPropertyScan(scan) => {
            PhysicalOp::NodeByPropertyScan(NodeByPropertyScanExec {
                input: scan.input,
                var: scan.var,
                labels: scan.labels,
                key: scan.key,
                value: scan.value,
            })
        }

        LogicalOp::Expand(expand) => PhysicalOp::Expand(ExpandExec {
            input: expand.input,
            src: expand.src,
            rel: expand.rel,
            dst: expand.dst,
            types: expand.types,
            direction: expand.direction,
            rel_properties: expand.rel_properties,
            range: expand.range,
        }),

        LogicalOp::Filter(filter) => PhysicalOp::Filter(FilterExec {
            input: filter.input,
            predicate: filter.predicate,
        }),

        LogicalOp::Projection(proj) => PhysicalOp::Projection(ProjectionExec {
            input: proj.input,
            distinct: proj.distinct,
            items: proj.items,
            include_existing: proj.include_existing,
        }),

        LogicalOp::Unwind(unwind) => PhysicalOp::Unwind(UnwindExec {
            input: unwind.input,
            expr: unwind.expr,
            alias: unwind.alias,
        }),

        LogicalOp::Aggregation(agg) => PhysicalOp::HashAggregation(HashAggregationExec {
            input: agg.input,
            group_by: agg.group_by,
            aggregates: agg.aggregates,
        }),

        LogicalOp::Sort(sort) => PhysicalOp::Sort(SortExec {
            input: sort.input,
            items: sort.items,
            top_k: sort.top_k,
        }),

        LogicalOp::Limit(limit) => PhysicalOp::Limit(LimitExec {
            input: limit.input,
            skip: limit.skip,
            limit: limit.limit,
        }),

        LogicalOp::Create(create) => PhysicalOp::Create(CreateExec {
            input: create.input,
            pattern: create.pattern,
        }),

        LogicalOp::Merge(merge) => PhysicalOp::Merge(MergeExec {
            input: merge.input,
            pattern_part: merge.pattern_part,
            actions: merge.actions,
        }),

        LogicalOp::Delete(delete) => PhysicalOp::Delete(DeleteExec {
            input: delete.input,
            detach: delete.detach,
            expressions: delete.expressions,
        }),

        LogicalOp::Set(set) => PhysicalOp::Set(SetExec {
            input: set.input,
            items: set.items,
        }),

        LogicalOp::Remove(remove) => PhysicalOp::Remove(RemoveExec {
            input: remove.input,
            items: remove.items,
        }),

        LogicalOp::OptionalMatch(om) => PhysicalOp::OptionalMatch(OptionalMatchExec {
            input: om.input,
            inner: om.inner,
            new_vars: om.new_vars,
        }),

        LogicalOp::PathBuild(pb) => PhysicalOp::PathBuild(PathBuildExec {
            input: pb.input,
            output: pb.output,
            node_vars: pb.node_vars,
            rel_vars: pb.rel_vars,
            shortest_path_all: pb.shortest_path_all,
        }),
    }
}

fn lower_node_scan(scan: NodeScan) -> PhysicalOp {
    if scan.labels.is_empty() {
        PhysicalOp::NodeScan(NodeScanExec {
            input: scan.input,
            var: scan.var,
        })
    } else {
        PhysicalOp::NodeByLabelScan(NodeByLabelScanExec {
            input: scan.input,
            var: scan.var,
            labels: scan.labels,
        })
    }
}

fn collect_vars(expr: &ResolvedExpr) -> BTreeSet<VarId> {
    let mut vars = BTreeSet::new();
    collect_vars_inner(expr, &mut vars);
    vars
}

fn property_equality_candidate(
    predicate: &ResolvedExpr,
    input: &LogicalOp,
) -> Option<(VarId, String, ResolvedExpr)> {
    let LogicalOp::NodeScan(scan) = input else {
        return None;
    };

    property_equality_for_var(predicate, scan.var)
}

fn property_equality_for_var(
    predicate: &ResolvedExpr,
    var: VarId,
) -> Option<(VarId, String, ResolvedExpr)> {
    let ResolvedExpr::Binary { lhs, op, rhs } = predicate else {
        return None;
    };

    if matches!(op, BinaryOp::And) {
        return property_equality_for_var(lhs, var).or_else(|| property_equality_for_var(rhs, var));
    }

    if !matches!(op, BinaryOp::Eq) {
        return None;
    }

    property_access_for_var(lhs, var)
        .filter(|_| !collect_vars(rhs).contains(&var))
        .map(|key| (var, key, (**rhs).clone()))
        .or_else(|| {
            property_access_for_var(rhs, var)
                .filter(|_| !collect_vars(lhs).contains(&var))
                .map(|key| (var, key, (**lhs).clone()))
        })
}

fn static_limit_bound(limit: &Limit) -> Option<usize> {
    let limit_rows = match &limit.limit {
        Some(expr) => static_non_negative_usize(expr)?,
        None => return None,
    };
    let skip_rows = limit
        .skip
        .as_ref()
        .and_then(static_non_negative_usize)
        .unwrap_or(0);
    Some(skip_rows.saturating_add(limit_rows))
}

fn limit_sort_bound(op: &LogicalOp) -> Option<(PlanNodeId, usize)> {
    let LogicalOp::Limit(limit) = op else {
        return None;
    };

    static_limit_bound(limit).map(|bound| (limit.input, bound))
}

fn sort_op_mut(op: &mut LogicalOp) -> Option<&mut Sort> {
    match op {
        LogicalOp::Sort(sort) => Some(sort),
        _ => None,
    }
}

fn merge_top_k_bound(current: Option<usize>, bound: usize) -> Option<usize> {
    Some(current.map(|current| current.min(bound)).unwrap_or(bound))
}

fn static_non_negative_usize(expr: &ResolvedExpr) -> Option<usize> {
    match expr {
        ResolvedExpr::Literal(LiteralValue::Integer(value)) => {
            Some((*value).max(0).try_into().unwrap_or(usize::MAX))
        }
        _ => None,
    }
}

fn property_access_for_var(expr: &ResolvedExpr, var: VarId) -> Option<String> {
    match expr {
        ResolvedExpr::Property { expr, property } => match &**expr {
            ResolvedExpr::Variable(v) if *v == var => Some(property.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn collect_vars_inner(expr: &ResolvedExpr, out: &mut BTreeSet<VarId>) {
    match expr {
        ResolvedExpr::Variable(v) => {
            out.insert(*v);
        }
        ResolvedExpr::Property { expr, .. } => collect_vars_inner(expr, out),
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            collect_vars_inner(lhs, out);
            collect_vars_inner(rhs, out);
        }
        ResolvedExpr::Unary { expr, .. } => collect_vars_inner(expr, out),
        ResolvedExpr::Function { args, .. } => {
            for arg in args {
                collect_vars_inner(arg, out);
            }
        }
        ResolvedExpr::List(items) => {
            for item in items {
                collect_vars_inner(item, out);
            }
        }
        ResolvedExpr::Map(items) => {
            for (_, v) in items {
                collect_vars_inner(v, out);
            }
        }
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            if let Some(e) = input {
                collect_vars_inner(e, out);
            }
            for (w, t) in alternatives {
                collect_vars_inner(w, out);
                collect_vars_inner(t, out);
            }
            if let Some(e) = else_expr {
                collect_vars_inner(e, out);
            }
        }
        ResolvedExpr::ListPredicate {
            variable,
            list,
            predicate,
            ..
        } => {
            out.insert(*variable);
            collect_vars_inner(list, out);
            collect_vars_inner(predicate, out);
        }
        ResolvedExpr::ListComprehension {
            variable,
            list,
            filter,
            map_expr,
            ..
        } => {
            out.insert(*variable);
            collect_vars_inner(list, out);
            if let Some(f) = filter {
                collect_vars_inner(f, out);
            }
            if let Some(m) = map_expr {
                collect_vars_inner(m, out);
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
            collect_vars_inner(init, out);
            collect_vars_inner(list, out);
            collect_vars_inner(expr, out);
        }
        ResolvedExpr::Index { expr, index } => {
            collect_vars_inner(expr, out);
            collect_vars_inner(index, out);
        }
        ResolvedExpr::Slice { expr, from, to } => {
            collect_vars_inner(expr, out);
            if let Some(f) = from {
                collect_vars_inner(f, out);
            }
            if let Some(t) = to {
                collect_vars_inner(t, out);
            }
        }
        ResolvedExpr::MapProjection { base, selectors } => {
            collect_vars_inner(base, out);
            for sel in selectors {
                if let lora_analyzer::ResolvedMapSelector::Literal(_, e) = sel {
                    collect_vars_inner(e, out);
                }
            }
        }
        _ => {}
    }
}
