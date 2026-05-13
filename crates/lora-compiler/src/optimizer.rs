use crate::logical::*;
use crate::physical::*;
use lora_analyzer::{symbols::VarId, LiteralValue, ResolvedExpr};
use lora_ast::BinaryOp;
use lora_store::GraphStats;
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

    /// Run the rewrite pipeline. `stats` drives cost-based selection
    /// when multiple index rewrites match the same `Filter(NodeScan)`;
    /// pass [`GraphStats::default()`] to disable scoring (every rewrite
    /// commits unconditionally, the pre-cost-model behaviour).
    pub fn optimize(&mut self, mut plan: LogicalPlan, stats: &GraphStats) -> LogicalPlan {
        self.push_filter_below_projection(&mut plan);
        self.use_indexed_node_scans(&mut plan, stats);
        self.use_indexed_rel_scans(&mut plan, stats);
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

            let Some(input) = plan.nodes.get(input_id) else {
                continue;
            };

            if !can_push_filter_below_projection(&plan.nodes[i], input) {
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

    /// Cost-driven rewrite pass. For each `Filter(NodeScan)` pattern,
    /// collect every catalog-backed index rewrite (equality, range,
    /// text, spatial), drop tautological candidates that select every
    /// row, and commit the cheapest candidate when it strictly beats
    /// the un-rewritten scan according to [`score_logical_op`].
    fn use_indexed_node_scans(&self, plan: &mut LogicalPlan, stats: &GraphStats) {
        let len = plan.nodes.len();
        for i in 0..len {
            let (input_id, predicate) = match &plan.nodes[i] {
                LogicalOp::Filter(f) => (f.input, f.predicate.clone()),
                _ => continue,
            };
            let LogicalOp::NodeScan(scan) = &plan.nodes[input_id] else {
                continue;
            };

            let candidates = collect_index_candidates(scan, &predicate, stats);
            let Some(best) = pick_best_candidate(&plan.nodes[input_id], candidates, stats) else {
                continue;
            };
            plan.nodes[input_id] = best;
        }
    }

    /// Rewrite `Filter(Expand(NodeScan(empty_labels), ...), pred-on-rel)`
    /// into a relationship-targeted index scan when the predicate
    /// matches a known shape (`r.prop CMP value`, `r.prop STARTS WITH …`,
    /// `point.withinBBox(r.prop, …)`). The rewrite is gated on the
    /// source NodeScan being unconstrained. The original `Filter` stays
    /// in place so any residual predicates outside the extracted index
    /// condition still run with normal expression semantics.
    fn use_indexed_rel_scans(&self, plan: &mut LogicalPlan, stats: &GraphStats) {
        let len = plan.nodes.len();
        for i in 0..len {
            let (filter_input, predicate) = match &plan.nodes[i] {
                LogicalOp::Filter(f) => (f.input, f.predicate.clone()),
                _ => continue,
            };
            let LogicalOp::Expand(expand) = &plan.nodes[filter_input] else {
                continue;
            };
            // Variable-length expansions and inline rel-property
            // patterns aren't handled by the index-targeted scan op.
            if expand.range.is_some() || expand.rel_properties.is_some() {
                continue;
            }
            let Some(rel_var) = expand.rel else {
                continue;
            };
            // Source must be a root, unconstrained NodeScan; otherwise
            // replacing it can bypass upstream rows or an already-bound
            // source variable.
            let LogicalOp::NodeScan(src_scan) = &plan.nodes[expand.input] else {
                continue;
            };
            if src_scan.input.is_some() || !src_scan.labels.is_empty() {
                continue;
            }
            let nodescan_input = src_scan.input;
            let expand = expand.clone();

            let candidates =
                collect_rel_index_candidates(&expand, &predicate, rel_var, nodescan_input, stats);
            let Some(best) = pick_best_candidate(&plan.nodes[filter_input], candidates, stats)
            else {
                continue;
            };
            // The new scan binds (src, rel, dst) and prefilters by the
            // indexed predicate. Keep the Filter above it so unrelated
            // conjuncts are not dropped.
            plan.nodes[filter_input] = best;
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
    let filter = match plan.nodes.get(filter_id).cloned() {
        Some(LogicalOp::Filter(f)) => f,
        _ => return,
    };
    let proj = match plan.nodes.get(projection_id).cloned() {
        Some(LogicalOp::Projection(p)) => p,
        _ => return,
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

        LogicalOp::NodeByPropertyRangeScan(scan) => {
            PhysicalOp::NodeByPropertyRangeScan(NodeByPropertyRangeScanExec {
                input: scan.input,
                var: scan.var,
                labels: scan.labels,
                key: scan.key,
                lo: scan.lo,
                lo_inclusive: scan.lo_inclusive,
                hi: scan.hi,
                hi_inclusive: scan.hi_inclusive,
            })
        }

        LogicalOp::NodeByTextScan(scan) => PhysicalOp::NodeByTextScan(NodeByTextScanExec {
            input: scan.input,
            var: scan.var,
            labels: scan.labels,
            key: scan.key,
            predicate: scan.predicate,
            query: scan.query,
        }),

        LogicalOp::NodeByPointScan(scan) => PhysicalOp::NodeByPointScan(NodeByPointScanExec {
            input: scan.input,
            var: scan.var,
            labels: scan.labels,
            key: scan.key,
            predicate: scan.predicate,
        }),

        LogicalOp::RelByPropertyRangeScan(scan) => {
            PhysicalOp::RelByPropertyRangeScan(RelByPropertyRangeScanExec {
                input: scan.input,
                src: scan.src,
                rel: scan.rel,
                dst: scan.dst,
                types: scan.types,
                direction: scan.direction,
                key: scan.key,
                lo: scan.lo,
                lo_inclusive: scan.lo_inclusive,
                hi: scan.hi,
                hi_inclusive: scan.hi_inclusive,
            })
        }

        LogicalOp::RelByTextScan(scan) => PhysicalOp::RelByTextScan(RelByTextScanExec {
            input: scan.input,
            src: scan.src,
            rel: scan.rel,
            dst: scan.dst,
            types: scan.types,
            direction: scan.direction,
            key: scan.key,
            predicate: scan.predicate,
            query: scan.query,
        }),

        LogicalOp::RelByPointScan(scan) => PhysicalOp::RelByPointScan(RelByPointScanExec {
            input: scan.input,
            src: scan.src,
            rel: scan.rel,
            dst: scan.dst,
            types: scan.types,
            direction: scan.direction,
            key: scan.key,
            predicate: scan.predicate,
        }),

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

/// Build every applicable index rewrite for a `Filter(NodeScan)` site,
/// dropping ones whose extracted predicate is trivially true (would
/// "match everything" — see [`is_tautological_*`] helpers). Each entry
/// is a fully-formed `LogicalOp` ready to drop into `plan.nodes[input]`.
fn collect_index_candidates(
    scan: &NodeScan,
    predicate: &ResolvedExpr,
    stats: &GraphStats,
) -> Vec<LogicalOp> {
    let mut out = Vec::new();

    if let Some((var, key, value)) = property_equality_for_var(predicate, scan.var) {
        out.push(LogicalOp::NodeByPropertyScan(NodeByPropertyScan {
            input: scan.input,
            var,
            labels: scan.labels.clone(),
            key,
            value,
        }));
    }

    if let Some(bounds) = collect_range_bounds(predicate, scan.var) {
        if !is_tautological_range(&bounds)
            && first_simple_label(&scan.labels)
                .is_some_and(|label| stats.has_node_range_index(label, &bounds.key))
        {
            out.push(LogicalOp::NodeByPropertyRangeScan(
                NodeByPropertyRangeScan {
                    input: scan.input,
                    var: scan.var,
                    labels: scan.labels.clone(),
                    key: bounds.key,
                    lo: bounds.lo,
                    lo_inclusive: bounds.lo_inclusive,
                    hi: bounds.hi,
                    hi_inclusive: bounds.hi_inclusive,
                },
            ));
        }
    }

    if let Some(candidate) = text_predicate_for_var(predicate, scan.var) {
        if !is_tautological_text(&candidate)
            && first_simple_label(&scan.labels)
                .is_some_and(|label| stats.has_node_text_index(label, &candidate.key))
        {
            out.push(LogicalOp::NodeByTextScan(NodeByTextScan {
                input: scan.input,
                var: scan.var,
                labels: scan.labels.clone(),
                key: candidate.key,
                predicate: candidate.predicate,
                query: candidate.query,
            }));
        }
    }

    if let Some(candidate) = point_predicate_for_var(predicate, scan.var) {
        if !is_tautological_point(&candidate)
            && first_simple_label(&scan.labels)
                .is_some_and(|label| stats.has_node_point_index(label, &candidate.key))
        {
            out.push(LogicalOp::NodeByPointScan(NodeByPointScan {
                input: scan.input,
                var: scan.var,
                labels: scan.labels.clone(),
                key: candidate.key,
                predicate: candidate.predicate,
            }));
        }
    }

    out
}

/// Build every applicable rel-targeted index rewrite for a
/// `Filter(Expand(NodeScan, …), pred-on-rel-var)` site. Mirrors
/// [`collect_index_candidates`] for nodes; tautological predicates are
/// dropped through the same `is_tautological_*` helpers.
fn collect_rel_index_candidates(
    expand: &Expand,
    predicate: &ResolvedExpr,
    rel_var: VarId,
    input: Option<PlanNodeId>,
    stats: &GraphStats,
) -> Vec<LogicalOp> {
    let mut out = Vec::new();

    if let Some(candidate) = text_predicate_for_var(predicate, rel_var) {
        if !is_tautological_text(&candidate)
            && rel_types_have_index(&expand.types, |ty| {
                stats.has_relationship_text_index(ty, &candidate.key)
            })
        {
            out.push(LogicalOp::RelByTextScan(RelByTextScan {
                input,
                src: expand.src,
                rel: rel_var,
                dst: expand.dst,
                types: expand.types.clone(),
                direction: expand.direction,
                key: candidate.key,
                predicate: candidate.predicate,
                query: candidate.query,
            }));
        }
    }

    if let Some(bounds) = collect_range_bounds(predicate, rel_var) {
        if !is_tautological_range(&bounds)
            && rel_types_have_index(&expand.types, |ty| {
                stats.has_relationship_range_index(ty, &bounds.key)
            })
        {
            out.push(LogicalOp::RelByPropertyRangeScan(RelByPropertyRangeScan {
                input,
                src: expand.src,
                rel: rel_var,
                dst: expand.dst,
                types: expand.types.clone(),
                direction: expand.direction,
                key: bounds.key,
                lo: bounds.lo,
                lo_inclusive: bounds.lo_inclusive,
                hi: bounds.hi,
                hi_inclusive: bounds.hi_inclusive,
            }));
        }
    }

    if let Some(candidate) = point_predicate_for_var(predicate, rel_var) {
        if !is_tautological_point(&candidate)
            && rel_types_have_index(&expand.types, |ty| {
                stats.has_relationship_point_index(ty, &candidate.key)
            })
        {
            out.push(LogicalOp::RelByPointScan(RelByPointScan {
                input,
                src: expand.src,
                rel: rel_var,
                dst: expand.dst,
                types: expand.types.clone(),
                direction: expand.direction,
                key: candidate.key,
                predicate: candidate.predicate,
            }));
        }
    }

    out
}

fn rel_types_have_index<F>(types: &[String], mut has_index: F) -> bool
where
    F: FnMut(&str) -> bool,
{
    !types.is_empty() && types.iter().all(|ty| has_index(ty))
}

/// Choose the cheapest replacement for the original `Filter(NodeScan)`
/// input among `candidates`, breaking ties by the caller-provided
/// collection order so behaviour is deterministic. Returns
/// `None` when no candidate is strictly cheaper than the original — in
/// that case the caller leaves the plan unchanged.
fn pick_best_candidate(
    original: &LogicalOp,
    candidates: Vec<LogicalOp>,
    stats: &GraphStats,
) -> Option<LogicalOp> {
    if candidates.is_empty() {
        return None;
    }

    let baseline = score_logical_op(original, stats);

    let mut best: Option<(LogicalOp, Option<u64>)> = None;
    for candidate in candidates {
        let score = score_logical_op(&candidate, stats);
        if !improves_over(score, baseline) {
            continue;
        }
        let take = match &best {
            None => true,
            Some((_, current_best)) => is_cheaper(score, *current_best),
        };
        if take {
            best = Some((candidate, score));
        }
    }

    best.map(|(op, _)| op)
}

/// `true` when committing a candidate with `score` is at least as good
/// as keeping the original (`baseline`). With unknown stats the
/// optimizer keeps its pre-cost-model behaviour: a `None`/`None`
/// comparison still commits already-collected candidates. Catalog
/// checks happen before this point for range/text/point operators.
fn improves_over(score: Option<u64>, baseline: Option<u64>) -> bool {
    match (score, baseline) {
        (Some(s), Some(b)) => s <= b,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => true,
    }
}

fn is_cheaper(score: Option<u64>, current_best: Option<u64>) -> bool {
    match (score, current_best) {
        (Some(s), Some(b)) => s < b,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// Estimated row count produced by `op`. `None` means "no information"
/// — typically because the relevant labels or property are not in the
/// stats snapshot. Mirrors the per-operator estimates that `EXPLAIN`
/// surfaces in [`crate::plan_tree::PlanTreeNode::estimated_rows`], so
/// the optimizer's pick agrees with what users see in the plan tree.
fn score_logical_op(op: &LogicalOp, stats: &GraphStats) -> Option<u64> {
    match op {
        LogicalOp::NodeScan(scan) => match label_estimate(&scan.labels, stats) {
            Some(rows) => Some(rows),
            None if scan.labels.is_empty() => Some(stats.node_count as u64),
            None => None,
        },
        LogicalOp::NodeByPropertyScan(scan) => {
            let label = first_simple_label(&scan.labels)?;
            stats.estimate_node_property_equality(label, &scan.key)
        }
        LogicalOp::NodeByPropertyRangeScan(scan) => {
            // Conservative one-third selectivity: a one-sided range
            // typically narrows by less than half; a two-sided range
            // narrows further. Better than full-label scan for any
            // useful range, never worse than `label_count`.
            let base = label_estimate(&scan.labels, stats)?;
            let denom = match (scan.lo.is_some(), scan.hi.is_some()) {
                (true, true) => 4,
                _ => 3,
            };
            Some(base.div_ceil(denom))
        }
        LogicalOp::NodeByTextScan(scan) => {
            let base = label_estimate(&scan.labels, stats)?;
            // Prefix/suffix probes typically narrow more than CONTAINS.
            let denom = match scan.predicate {
                TextPredicate::StartsWith | TextPredicate::EndsWith => 4,
                TextPredicate::Contains => 2,
            };
            Some(base.div_ceil(denom))
        }
        LogicalOp::NodeByPointScan(scan) => {
            let base = label_estimate(&scan.labels, stats)?;
            // Spatial probes: bbox/distance usually returns a small
            // fraction of the labelled set.
            Some(base.div_ceil(5))
        }
        LogicalOp::Filter(_) => None,
        LogicalOp::Expand(expand) => {
            // Used as the baseline when evaluating rel-index rewrites.
            // Without per-edge histograms we approximate edges-of-type
            // by `relationship_type_count`, falling back to the global
            // relationship total when no type is named.
            let count = if expand.types.is_empty() {
                stats.relationship_count as u64
            } else {
                let mut total: u64 = 0;
                for ty in &expand.types {
                    total = total.saturating_add(stats.relationship_type_count(ty)?);
                }
                total
            };
            // Undirected expansion produces both orientations.
            Some(match expand.direction {
                lora_ast::Direction::Undirected => count.saturating_mul(2),
                _ => count,
            })
        }
        LogicalOp::RelByPropertyRangeScan(scan) => {
            let base = rel_type_estimate(&scan.types, stats)?;
            let denom = match (scan.lo.is_some(), scan.hi.is_some()) {
                (true, true) => 4,
                _ => 3,
            };
            let est = base.div_ceil(denom);
            Some(match scan.direction {
                lora_ast::Direction::Undirected => est.saturating_mul(2),
                _ => est,
            })
        }
        LogicalOp::RelByTextScan(scan) => {
            let base = rel_type_estimate(&scan.types, stats)?;
            let denom = match scan.predicate {
                TextPredicate::StartsWith | TextPredicate::EndsWith => 4,
                TextPredicate::Contains => 2,
            };
            let est = base.div_ceil(denom);
            Some(match scan.direction {
                lora_ast::Direction::Undirected => est.saturating_mul(2),
                _ => est,
            })
        }
        LogicalOp::RelByPointScan(scan) => {
            let base = rel_type_estimate(&scan.types, stats)?;
            let est = base.div_ceil(5);
            Some(match scan.direction {
                lora_ast::Direction::Undirected => est.saturating_mul(2),
                _ => est,
            })
        }
        _ => None,
    }
}

fn rel_type_estimate(types: &[String], stats: &GraphStats) -> Option<u64> {
    if types.is_empty() {
        return Some(stats.relationship_count as u64);
    }
    let mut total: u64 = 0;
    for ty in types {
        total = total.saturating_add(stats.relationship_type_count(ty)?);
    }
    Some(total)
}

/// Return the count of nodes covered by a `labels` group, taking the
/// first DNF disjunction's first literal. Mirrors `labels_estimate`
/// from `lora-database/src/database/explain.rs` — keeping the two in
/// sync ensures `EXPLAIN` and the optimizer agree.
fn label_estimate(labels: &[Vec<String>], stats: &GraphStats) -> Option<u64> {
    let label = first_simple_label(labels)?;
    stats.label_count(label)
}

fn first_simple_label(labels: &[Vec<String>]) -> Option<&str> {
    labels.first()?.first().map(String::as_str)
}

fn is_tautological_range(bounds: &RangeBounds) -> bool {
    let lo_open = match (&bounds.lo, bounds.lo_inclusive) {
        (None, _) => true,
        (Some(expr), false) => matches!(
            expr,
            ResolvedExpr::Literal(LiteralValue::Integer(v)) if *v == i64::MIN
        ),
        (Some(_), true) => false,
    };
    let hi_open = match (&bounds.hi, bounds.hi_inclusive) {
        (None, _) => true,
        (Some(expr), false) => matches!(
            expr,
            ResolvedExpr::Literal(LiteralValue::Integer(v)) if *v == i64::MAX
        ),
        (Some(_), true) => false,
    };
    lo_open && hi_open
}

fn is_tautological_text(candidate: &TextCandidate) -> bool {
    matches!(
        &candidate.query,
        ResolvedExpr::Literal(LiteralValue::String(s)) if s.is_empty()
    )
}

fn is_tautological_point(candidate: &PointCandidate) -> bool {
    match &candidate.predicate {
        PointPredicate::WithinBBox {
            lower_left,
            upper_right,
        } => is_world_bbox(lower_left, upper_right),
        PointPredicate::WithinDistance { .. } => false,
    }
}

/// `{longitude: -180, latitude: -90}::POINT` to
/// `{longitude: 180, latitude: 90}::POINT` covers every WGS-84 point.
/// Detecting the literal form avoids a trigram lookup that would yield
/// every indexed row only to be re-filtered to the same set.
fn is_world_bbox(lower_left: &ResolvedExpr, upper_right: &ResolvedExpr) -> bool {
    /// Cypher parses `-180` as `Unary{Neg, Literal(180)}`, not as a
    /// negative integer literal — peel one such layer so the
    /// world-bbox detection works on natural query forms.
    fn const_number(expr: &ResolvedExpr) -> Option<f64> {
        match expr {
            ResolvedExpr::Literal(LiteralValue::Float(v)) => Some(*v),
            ResolvedExpr::Literal(LiteralValue::Integer(v)) => Some(*v as f64),
            ResolvedExpr::Unary {
                op: lora_ast::UnaryOp::Neg,
                expr,
            } => const_number(expr).map(|v| -v),
            ResolvedExpr::Unary {
                op: lora_ast::UnaryOp::Pos,
                expr,
            } => const_number(expr),
            _ => None,
        }
    }

    fn point_lon_lat(expr: &ResolvedExpr) -> Option<(f64, f64)> {
        let items = point_literal_map(expr)?;
        let mut lon: Option<f64> = None;
        let mut lat: Option<f64> = None;
        for (key, value) in items {
            let n = const_number(value)?;
            match key.as_str() {
                "longitude" | "x" => lon = Some(n),
                "latitude" | "y" => lat = Some(n),
                _ => {}
            }
        }
        Some((lon?, lat?))
    }

    let Some((ll_lon, ll_lat)) = point_lon_lat(lower_left) else {
        return false;
    };
    let Some((ur_lon, ur_lat)) = point_lon_lat(upper_right) else {
        return false;
    };
    ll_lon <= -180.0 && ll_lat <= -90.0 && ur_lon >= 180.0 && ur_lat >= 90.0
}

fn point_literal_map(expr: &ResolvedExpr) -> Option<&Vec<(String, ResolvedExpr)>> {
    let ResolvedExpr::Function { function, args, .. } = expr else {
        return None;
    };
    if function.eq_ignore_ascii_case("geo.point") && args.len() == 1 {
        let ResolvedExpr::Map(items) = &args[0] else {
            return None;
        };
        return Some(items);
    }
    if function.eq_ignore_ascii_case("cast.to") && args.len() == 2 {
        let ResolvedExpr::Map(items) = &args[0] else {
            return None;
        };
        let ResolvedExpr::Literal(LiteralValue::TypeName(target)) = &args[1] else {
            return None;
        };
        if target.eq_ignore_ascii_case("POINT") {
            return Some(items);
        }
    }
    None
}

struct RangeBounds {
    key: String,
    lo: Option<ResolvedExpr>,
    lo_inclusive: bool,
    hi: Option<ResolvedExpr>,
    hi_inclusive: bool,
}

/// Walk an AND-tree and collect any `var.prop CMP literal` bounds.
/// Returns `None` if no comparison touches `var.prop` for a single
/// property key — we don't try to combine multi-property bounds in v1.
fn collect_range_bounds(predicate: &ResolvedExpr, var: VarId) -> Option<RangeBounds> {
    let mut key: Option<String> = None;
    let mut lo: Option<ResolvedExpr> = None;
    let mut lo_inclusive = false;
    let mut hi: Option<ResolvedExpr> = None;
    let mut hi_inclusive = false;
    let mut any = false;

    walk_and_for_range(
        predicate,
        var,
        &mut |found_key, side, value, inclusive| match side {
            RangeSide::Lower => {
                if key.as_deref().map(|k| k != found_key).unwrap_or(false) {
                    return;
                }
                key = Some(found_key.to_string());
                if lo
                    .as_ref()
                    .map(|current| lower_bound_is_tighter(&value, inclusive, current, lo_inclusive))
                    .unwrap_or(true)
                {
                    lo = Some(value);
                    lo_inclusive = inclusive;
                }
                any = true;
            }
            RangeSide::Upper => {
                if key.as_deref().map(|k| k != found_key).unwrap_or(false) {
                    return;
                }
                key = Some(found_key.to_string());
                if hi
                    .as_ref()
                    .map(|current| upper_bound_is_tighter(&value, inclusive, current, hi_inclusive))
                    .unwrap_or(true)
                {
                    hi = Some(value);
                    hi_inclusive = inclusive;
                }
                any = true;
            }
        },
    );

    if !any {
        return None;
    }
    Some(RangeBounds {
        key: key?,
        lo,
        lo_inclusive,
        hi,
        hi_inclusive,
    })
}

#[derive(Clone, Copy)]
enum RangeSide {
    Lower,
    Upper,
}

fn lower_bound_is_tighter(
    candidate: &ResolvedExpr,
    candidate_inclusive: bool,
    current: &ResolvedExpr,
    current_inclusive: bool,
) -> bool {
    match compare_literal_bounds(candidate, current) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Equal) => !candidate_inclusive && current_inclusive,
        _ => false,
    }
}

fn upper_bound_is_tighter(
    candidate: &ResolvedExpr,
    candidate_inclusive: bool,
    current: &ResolvedExpr,
    current_inclusive: bool,
) -> bool {
    match compare_literal_bounds(candidate, current) {
        Some(std::cmp::Ordering::Less) => true,
        Some(std::cmp::Ordering::Equal) => !candidate_inclusive && current_inclusive,
        _ => false,
    }
}

fn compare_literal_bounds(lhs: &ResolvedExpr, rhs: &ResolvedExpr) -> Option<std::cmp::Ordering> {
    match (literal_number(lhs), literal_number(rhs)) {
        (Some(a), Some(b)) => return a.partial_cmp(&b),
        (Some(_), None) | (None, Some(_)) => return None,
        (None, None) => {}
    }

    match (lhs, rhs) {
        (
            ResolvedExpr::Literal(LiteralValue::String(a)),
            ResolvedExpr::Literal(LiteralValue::String(b)),
        ) => Some(a.cmp(b)),
        _ => None,
    }
}

fn literal_number(expr: &ResolvedExpr) -> Option<f64> {
    match expr {
        ResolvedExpr::Literal(LiteralValue::Integer(v)) => Some(*v as f64),
        ResolvedExpr::Literal(LiteralValue::Float(v)) => Some(*v),
        ResolvedExpr::Unary {
            op: lora_ast::UnaryOp::Neg,
            expr,
        } => literal_number(expr).map(|v| -v),
        ResolvedExpr::Unary {
            op: lora_ast::UnaryOp::Pos,
            expr,
        } => literal_number(expr),
        _ => None,
    }
}

fn walk_and_for_range<F>(predicate: &ResolvedExpr, var: VarId, visit: &mut F)
where
    F: FnMut(&str, RangeSide, ResolvedExpr, bool),
{
    if let ResolvedExpr::Binary {
        lhs,
        op: BinaryOp::And,
        rhs,
    } = predicate
    {
        walk_and_for_range(lhs, var, visit);
        walk_and_for_range(rhs, var, visit);
        return;
    }

    let ResolvedExpr::Binary { lhs, op, rhs } = predicate else {
        return;
    };

    let (side, inclusive) = match op {
        BinaryOp::Gt => (RangeSide::Lower, false),
        BinaryOp::Ge => (RangeSide::Lower, true),
        BinaryOp::Lt => (RangeSide::Upper, false),
        BinaryOp::Le => (RangeSide::Upper, true),
        _ => return,
    };

    if let Some(key) = property_access_for_var(lhs, var) {
        if !collect_vars(rhs).contains(&var) {
            visit(&key, side, (**rhs).clone(), inclusive);
            return;
        }
    }
    if let Some(key) = property_access_for_var(rhs, var) {
        if !collect_vars(lhs).contains(&var) {
            // Mirror `value CMP var.prop` to `var.prop FLIPPED_CMP value`.
            let flipped = match side {
                RangeSide::Lower => RangeSide::Upper,
                RangeSide::Upper => RangeSide::Lower,
            };
            visit(&key, flipped, (**lhs).clone(), inclusive);
        }
    }
}

struct TextCandidate {
    key: String,
    predicate: TextPredicate,
    query: ResolvedExpr,
}

struct PointCandidate {
    key: String,
    predicate: PointPredicate,
}

fn point_predicate_for_var(predicate: &ResolvedExpr, var: VarId) -> Option<PointCandidate> {
    if let ResolvedExpr::Binary {
        lhs,
        op: BinaryOp::And,
        rhs,
    } = predicate
    {
        return point_predicate_for_var(lhs, var).or_else(|| point_predicate_for_var(rhs, var));
    }

    // geo.within_bbox(n.prop, ll, ur)
    if let ResolvedExpr::Function { function, args, .. } = predicate {
        if function.eq_ignore_ascii_case("geo.within_bbox") && args.len() == 3 {
            if let Some(key) = property_access_for_var(&args[0], var) {
                if !collect_vars(&args[1]).contains(&var) && !collect_vars(&args[2]).contains(&var)
                {
                    return Some(PointCandidate {
                        key,
                        predicate: PointPredicate::WithinBBox {
                            lower_left: args[1].clone(),
                            upper_right: args[2].clone(),
                        },
                    });
                }
            }
        }
    }

    // point.distance(n.prop, c) OP d  (where OP is <, <=)
    let ResolvedExpr::Binary { lhs, op, rhs } = predicate else {
        return None;
    };
    let inclusive = match op {
        BinaryOp::Le => true,
        BinaryOp::Lt => false,
        // Symmetric form: d >= point.distance(n.prop, c)
        BinaryOp::Ge => true,
        BinaryOp::Gt => false,
        _ => return None,
    };

    let (call_side, scalar_side) = match op {
        BinaryOp::Le | BinaryOp::Lt => ((**lhs).clone(), (**rhs).clone()),
        BinaryOp::Ge | BinaryOp::Gt => ((**rhs).clone(), (**lhs).clone()),
        _ => return None,
    };

    let ResolvedExpr::Function { function, args, .. } = &call_side else {
        return None;
    };
    if !function.eq_ignore_ascii_case("geo.distance") {
        return None;
    }
    if args.len() != 2 {
        return None;
    }
    let key = property_access_for_var(&args[0], var)?;
    if collect_vars(&args[1]).contains(&var) || collect_vars(&scalar_side).contains(&var) {
        return None;
    }
    Some(PointCandidate {
        key,
        predicate: PointPredicate::WithinDistance {
            center: args[1].clone(),
            max_distance: scalar_side,
            inclusive,
        },
    })
}

fn text_predicate_for_var(predicate: &ResolvedExpr, var: VarId) -> Option<TextCandidate> {
    let ResolvedExpr::Binary { lhs, op, rhs } = predicate else {
        return None;
    };

    if matches!(op, BinaryOp::And) {
        return text_predicate_for_var(lhs, var).or_else(|| text_predicate_for_var(rhs, var));
    }

    let kind = match op {
        BinaryOp::StartsWith => TextPredicate::StartsWith,
        BinaryOp::EndsWith => TextPredicate::EndsWith,
        BinaryOp::Contains => TextPredicate::Contains,
        _ => return None,
    };

    let key = property_access_for_var(lhs, var)?;
    if collect_vars(rhs).contains(&var) {
        return None;
    }
    Some(TextCandidate {
        key,
        predicate: kind,
        query: (**rhs).clone(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use lora_store::GraphStats;

    fn stats_with_label(label: &str, total: usize, distinct: Option<usize>) -> GraphStats {
        let mut s = GraphStats {
            node_count: total,
            ..Default::default()
        };
        s.nodes_by_label.insert(label.to_string(), total);
        if let Some(d) = distinct {
            s.node_distinct_values
                .insert((label.to_string(), "id".to_string()), d);
        }
        s
    }

    fn person_labels() -> Vec<Vec<String>> {
        vec![vec!["Person".to_string()]]
    }

    fn lit_int(v: i64) -> ResolvedExpr {
        ResolvedExpr::Literal(LiteralValue::Integer(v))
    }

    fn lit_str(s: &str) -> ResolvedExpr {
        ResolvedExpr::Literal(LiteralValue::String(s.to_string()))
    }

    fn lit_float(v: f64) -> ResolvedExpr {
        ResolvedExpr::Literal(LiteralValue::Float(v))
    }

    fn point_lonlat(lon: f64, lat: f64) -> ResolvedExpr {
        ResolvedExpr::Function {
            function: lora_analyzer::FunctionId::builtin("cast.to")
                .expect("cast.to builtin exists"),
            distinct: false,
            args: vec![
                ResolvedExpr::Map(vec![
                    ("longitude".to_string(), lit_float(lon)),
                    ("latitude".to_string(), lit_float(lat)),
                ]),
                ResolvedExpr::Literal(LiteralValue::TypeName("POINT".to_string())),
            ],
        }
    }

    // ---------- score_logical_op ----------

    #[test]
    fn score_label_scan_returns_label_count() {
        let stats = stats_with_label("Person", 1_000, None);
        let op = LogicalOp::NodeScan(NodeScan {
            input: None,
            var: VarId(0),
            labels: person_labels(),
        });
        assert_eq!(score_logical_op(&op, &stats), Some(1_000));
    }

    #[test]
    fn score_property_scan_uses_distinct() {
        // 1000 nodes, 100 distinct values for `id`: ~10 per value.
        let stats = stats_with_label("Person", 1_000, Some(100));
        let op = LogicalOp::NodeByPropertyScan(NodeByPropertyScan {
            input: None,
            var: VarId(0),
            labels: person_labels(),
            key: "id".to_string(),
            value: lit_int(7),
        });
        assert_eq!(score_logical_op(&op, &stats), Some(10));
    }

    #[test]
    fn score_property_scan_high_distinct_beats_label_scan() {
        // distinct == total → uniform-distribution heuristic gives 1
        // estimated row, well below the 100-row label scan.
        let stats = stats_with_label("Person", 100, Some(100));
        let label_score = score_logical_op(
            &LogicalOp::NodeScan(NodeScan {
                input: None,
                var: VarId(0),
                labels: person_labels(),
            }),
            &stats,
        );
        let property_score = score_logical_op(
            &LogicalOp::NodeByPropertyScan(NodeByPropertyScan {
                input: None,
                var: VarId(0),
                labels: person_labels(),
                key: "id".to_string(),
                value: lit_int(7),
            }),
            &stats,
        );
        assert!(property_score < label_score);
    }

    #[test]
    fn score_returns_none_without_label_stats() {
        // Empty stats: every estimator should fail open so the optimizer
        // can fall back to the legacy "commit any matching rewrite"
        // behaviour through `improves_over(None, None)`.
        let stats = GraphStats::default();
        let op = LogicalOp::NodeScan(NodeScan {
            input: None,
            var: VarId(0),
            labels: person_labels(),
        });
        assert_eq!(score_logical_op(&op, &stats), None);
    }

    // ---------- improves_over ----------

    #[test]
    fn improves_over_legacy_fallback_when_both_unknown() {
        // `(None, None)` must resolve to "commit the rewrite" — that is
        // the contract for environments without stats.
        assert!(improves_over(None, None));
    }

    #[test]
    fn improves_over_keeps_baseline_when_candidate_unknown() {
        assert!(!improves_over(None, Some(10)));
    }

    #[test]
    fn improves_over_strictly_better_or_equal_wins() {
        assert!(improves_over(Some(5), Some(10)));
        assert!(improves_over(Some(10), Some(10)));
        assert!(!improves_over(Some(11), Some(10)));
    }

    // ---------- pick_best_candidate ----------

    #[test]
    fn pick_best_candidate_picks_lowest_score() {
        let stats = stats_with_label("Person", 1_200, Some(100));
        let original = LogicalOp::NodeScan(NodeScan {
            input: None,
            var: VarId(0),
            labels: person_labels(),
        });
        // property scan ~12 rows (1200 / 100), range scan ~400 rows.
        let candidates = vec![
            LogicalOp::NodeByPropertyRangeScan(NodeByPropertyRangeScan {
                input: None,
                var: VarId(0),
                labels: person_labels(),
                key: "age".to_string(),
                lo: Some(lit_int(30)),
                lo_inclusive: false,
                hi: None,
                hi_inclusive: false,
            }),
            LogicalOp::NodeByPropertyScan(NodeByPropertyScan {
                input: None,
                var: VarId(0),
                labels: person_labels(),
                key: "id".to_string(),
                value: lit_int(7),
            }),
        ];
        let pick = pick_best_candidate(&original, candidates, &stats).expect("expected a pick");
        assert!(matches!(pick, LogicalOp::NodeByPropertyScan(_)));
    }

    #[test]
    fn pick_best_candidate_returns_none_when_no_candidate_improves() {
        // Stats put baseline at 1 row (1 node, distinct=1) and the
        // property scan also at 1 — but no rewrite improves, and we'd
        // still commit the candidate because of the s<=b fallback.
        // To prove we *can* return None, give the candidate a higher
        // score than baseline.
        let mut stats = stats_with_label("Person", 1, None);
        // 1 Person, 1 distinct → property scan = 1, label scan = 1.
        // Add a rare label so label_estimate gives 1 but the candidate
        // has no distinct entry → unknown score → can't improve.
        stats.nodes_by_label.insert("Tiny".to_string(), 1);
        let original = LogicalOp::NodeScan(NodeScan {
            input: None,
            var: VarId(0),
            labels: vec![vec!["Tiny".to_string()]],
        });
        // Candidate references a different label → label_estimate fails
        // → score is None. Baseline is Some(1). improves_over(None,
        // Some(1)) = false → no rewrite.
        let candidates = vec![LogicalOp::NodeByPropertyScan(NodeByPropertyScan {
            input: None,
            var: VarId(0),
            labels: vec![vec!["Missing".to_string()]],
            key: "id".to_string(),
            value: lit_int(7),
        })];
        assert!(pick_best_candidate(&original, candidates, &stats).is_none());
    }

    // ---------- tautology guards ----------

    #[test]
    fn unbounded_low_range_is_tautological() {
        let bounds = RangeBounds {
            key: "age".to_string(),
            lo: Some(lit_int(i64::MIN)),
            lo_inclusive: false,
            hi: None,
            hi_inclusive: false,
        };
        assert!(is_tautological_range(&bounds));
    }

    #[test]
    fn unbounded_high_range_is_tautological() {
        let bounds = RangeBounds {
            key: "age".to_string(),
            lo: None,
            lo_inclusive: false,
            hi: Some(lit_int(i64::MAX)),
            hi_inclusive: false,
        };
        assert!(is_tautological_range(&bounds));
    }

    #[test]
    fn doubly_unbounded_range_is_tautological() {
        let bounds = RangeBounds {
            key: "age".to_string(),
            lo: Some(lit_int(i64::MIN)),
            lo_inclusive: false,
            hi: Some(lit_int(i64::MAX)),
            hi_inclusive: false,
        };
        assert!(is_tautological_range(&bounds));
    }

    #[test]
    fn ordinary_range_is_not_tautological() {
        let bounds = RangeBounds {
            key: "age".to_string(),
            lo: Some(lit_int(0)),
            lo_inclusive: false,
            hi: Some(lit_int(100)),
            hi_inclusive: false,
        };
        assert!(!is_tautological_range(&bounds));
    }

    #[test]
    fn empty_string_starts_with_is_tautological() {
        let candidate = TextCandidate {
            key: "name".to_string(),
            predicate: TextPredicate::StartsWith,
            query: lit_str(""),
        };
        assert!(is_tautological_text(&candidate));
    }

    #[test]
    fn nonempty_string_starts_with_is_not_tautological() {
        let candidate = TextCandidate {
            key: "name".to_string(),
            predicate: TextPredicate::StartsWith,
            query: lit_str("A"),
        };
        assert!(!is_tautological_text(&candidate));
    }

    #[test]
    fn world_bbox_is_tautological() {
        let candidate = PointCandidate {
            key: "loc".to_string(),
            predicate: PointPredicate::WithinBBox {
                lower_left: point_lonlat(-180.0, -90.0),
                upper_right: point_lonlat(180.0, 90.0),
            },
        };
        assert!(is_tautological_point(&candidate));
    }

    #[test]
    fn city_bbox_is_not_tautological() {
        let candidate = PointCandidate {
            key: "loc".to_string(),
            predicate: PointPredicate::WithinBBox {
                lower_left: point_lonlat(4.7, 52.3),
                upper_right: point_lonlat(5.0, 52.5),
            },
        };
        assert!(!is_tautological_point(&candidate));
    }

    #[test]
    fn distance_bbox_is_never_tautological() {
        // Distance probes always narrow — no constant form folds them
        // into "all rows".
        let candidate = PointCandidate {
            key: "loc".to_string(),
            predicate: PointPredicate::WithinDistance {
                center: point_lonlat(0.0, 0.0),
                max_distance: lit_int(1_000_000_000),
                inclusive: true,
            },
        };
        assert!(!is_tautological_point(&candidate));
    }
}
