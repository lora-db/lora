use crate::logical::*;
use crate::physical::*;
use lora_analyzer::{symbols::VarId, ResolvedExpr};
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
        self.remove_redundant_limit(&mut plan);
        plan
    }

    fn push_filter_below_projection(&self, plan: &mut LogicalPlan) {
        let len = plan.nodes.len();

        for i in 0..len {
            // Inspect by reference first so we can decide without cloning the
            // potentially-large op payloads.
            let input_id = match &plan.nodes[i] {
                LogicalOp::Filter(f) => f.input,
                _ => continue,
            };

            let should_push = match (&plan.nodes[i], &plan.nodes[input_id]) {
                (LogicalOp::Filter(filter), LogicalOp::Projection(proj)) => {
                    if proj.distinct || proj.include_existing {
                        false
                    } else {
                        let output_vars: BTreeSet<VarId> =
                            proj.items.iter().map(|item| item.output).collect();
                        let pred_vars = collect_vars(&filter.predicate);
                        !pred_vars.iter().any(|v| output_vars.contains(v))
                    }
                }
                _ => false,
            };

            if !should_push {
                continue;
            }

            // Move both nodes out by swap, then rebuild without cloning.
            let placeholder = || LogicalOp::Argument(Argument);
            let filter = match std::mem::replace(&mut plan.nodes[i], placeholder()) {
                LogicalOp::Filter(f) => f,
                _ => unreachable!(),
            };
            let proj = match std::mem::replace(&mut plan.nodes[input_id], placeholder()) {
                LogicalOp::Projection(p) => p,
                _ => unreachable!(),
            };

            plan.nodes[input_id] = LogicalOp::Filter(Filter {
                input: proj.input,
                predicate: filter.predicate,
            });
            plan.nodes[i] = LogicalOp::Projection(Projection {
                input: input_id,
                distinct: proj.distinct,
                items: proj.items,
                include_existing: proj.include_existing,
            });
        }
    }

    fn remove_redundant_limit(&self, _plan: &mut LogicalPlan) {
        // placeholder for future rules
    }

    /// Lower a logical plan by consuming it — each op's owned payload
    /// (expressions, patterns, items) is moved into the physical op rather
    /// than cloned. Callers should not need the logical plan after this.
    pub fn lower_to_physical(&mut self, logical: LogicalPlan) -> PhysicalPlan {
        let LogicalPlan { root, nodes } = logical;

        let nodes = nodes
            .into_iter()
            .map(|op| match op {
                LogicalOp::Argument(_) => PhysicalOp::Argument(ArgumentExec),

                LogicalOp::NodeScan(scan) => {
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
            })
            .collect();

        PhysicalPlan { root, nodes }
    }
}

fn collect_vars(expr: &ResolvedExpr) -> BTreeSet<VarId> {
    let mut vars = BTreeSet::new();
    collect_vars_inner(expr, &mut vars);
    vars
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
