//! Hash aggregation operator source plus the streaming fold-only fast
//! path.
//!
//! Two materialization strategies share a single [`HashAggregationSource`]:
//!
//! - When every projection is a streamable fold (count / sum / min / max
//!   / avg without DISTINCT), [`materialize_streaming`] folds per-group
//!   running state on the fly. Memory is O(groups), not O(input rows),
//!   which is critical on `count(*)`-style workloads at scale. The
//!   buffered executor in `crate::executor` reuses the same fast path
//!   via the `pub(crate)` exports of [`classify_streamable_aggregates`],
//!   [`StreamableAggSpec`], and [`AggState`].
//! - Otherwise we drain upstream, group by key, then call
//!   `compute_aggregate_expr` on each group. This is the original
//!   buffered shape, kept for `collect`, `stdev`, `percentile*`, and any
//!   aggregate with `DISTINCT`.
//!
//! [`materialize_streaming`]: HashAggregationSource::materialize_streaming

use std::collections::BTreeMap;

use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::eval_expr_result;
use crate::executor::{compute_aggregate_expr, GroupValueKey};
use crate::value::{LoraValue, Row};

use super::traits::{drain, hydrate_value, RowSource, StreamCtx};

// ============================================================================
// Streaming fold-only aggregation (count / sum / min / max / avg, no DISTINCT)
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamableAggKind {
    /// `count()` / `count(*)` — count input rows.
    CountAll,
    /// `count(expr)` — count rows where the expression is non-null.
    CountField,
    /// `sum(expr)` over numeric values, NULLs ignored.
    Sum,
    /// `min(expr)` ignoring NULLs.
    Min,
    /// `max(expr)` ignoring NULLs.
    Max,
    /// `avg(expr)` ignoring NULLs.
    Avg,
}

pub(crate) struct StreamableAggSpec {
    pub(crate) kind: StreamableAggKind,
    /// `None` for `count(*)`; the expression to evaluate per row otherwise.
    pub(crate) arg: Option<ResolvedExpr>,
}

#[derive(Clone, Debug)]
pub(crate) enum AggState {
    Count(i64),
    /// Running sum tracking integer-only-so-far so we can emit `Int` when
    /// every contributing value was integer (matching the existing
    /// `compute_aggregate_expr` semantics).
    Sum {
        sum: f64,
        all_int: bool,
        any: bool,
    },
    Min(Option<LoraValue>),
    Max(Option<LoraValue>),
    Avg {
        sum: f64,
        count: usize,
    },
}

impl AggState {
    pub(crate) fn seed(kind: StreamableAggKind) -> Self {
        match kind {
            StreamableAggKind::CountAll | StreamableAggKind::CountField => AggState::Count(0),
            StreamableAggKind::Sum => AggState::Sum {
                sum: 0.0,
                all_int: true,
                any: false,
            },
            StreamableAggKind::Min => AggState::Min(None),
            StreamableAggKind::Max => AggState::Max(None),
            StreamableAggKind::Avg => AggState::Avg { sum: 0.0, count: 0 },
        }
    }

    pub(crate) fn fold(&mut self, kind: StreamableAggKind, value: LoraValue) {
        match self {
            AggState::Count(n) => match kind {
                StreamableAggKind::CountAll => *n += 1,
                StreamableAggKind::CountField if !matches!(value, LoraValue::Null) => *n += 1,
                _ => {}
            },
            AggState::Sum { sum, all_int, any } => match value {
                LoraValue::Null => {}
                LoraValue::Int(i) => {
                    *sum += i as f64;
                    *any = true;
                }
                LoraValue::Float(f) => {
                    *sum += f;
                    *all_int = false;
                    *any = true;
                }
                _ => {}
            },
            AggState::Min(slot) => {
                if matches!(value, LoraValue::Null) {
                    return;
                }
                match slot {
                    None => *slot = Some(value),
                    Some(cur) => {
                        if cmp_values_total(&value, cur) == std::cmp::Ordering::Less {
                            *cur = value;
                        }
                    }
                }
            }
            AggState::Max(slot) => {
                if matches!(value, LoraValue::Null) {
                    return;
                }
                match slot {
                    None => *slot = Some(value),
                    Some(cur) => {
                        if cmp_values_total(&value, cur) == std::cmp::Ordering::Greater {
                            *cur = value;
                        }
                    }
                }
            }
            AggState::Avg { sum, count } => {
                let n = match value {
                    LoraValue::Int(i) => Some(i as f64),
                    LoraValue::Float(f) => Some(f),
                    _ => None,
                };
                if let Some(n) = n {
                    *sum += n;
                    *count += 1;
                }
            }
        }
    }

    pub(crate) fn finalize(self, _kind: StreamableAggKind) -> LoraValue {
        match self {
            AggState::Count(n) => LoraValue::Int(n),
            AggState::Sum { sum, all_int, any } => {
                if !any {
                    LoraValue::Null
                } else if all_int && sum.fract() == 0.0 {
                    LoraValue::Int(sum as i64)
                } else {
                    LoraValue::Float(sum)
                }
            }
            AggState::Min(v) | AggState::Max(v) => v.unwrap_or(LoraValue::Null),
            AggState::Avg { sum, count } => {
                if count == 0 {
                    LoraValue::Null
                } else {
                    LoraValue::Float(sum / count as f64)
                }
            }
        }
    }
}

struct StreamingGroup {
    /// First input row in this group, retained so we can evaluate the
    /// `group_by` projections for the output without buffering more rows.
    first_row: Row,
    aggs: Vec<AggState>,
}

impl StreamingGroup {
    fn new(specs: &[StreamableAggSpec], first_row: Row) -> Self {
        Self {
            first_row,
            aggs: specs.iter().map(|spec| AggState::seed(spec.kind)).collect(),
        }
    }
}

/// If every aggregate in `projections` is a streamable fold (count, sum,
/// min, max, avg with no DISTINCT), return the per-projection specs.
/// Otherwise return `None` so the caller falls back to the buffered path.
pub(crate) fn classify_streamable_aggregates(
    projections: &[ResolvedProjection],
) -> Option<Vec<StreamableAggSpec>> {
    let mut specs = Vec::with_capacity(projections.len());
    for proj in projections {
        let spec = streamable_spec(&proj.expr)?;
        specs.push(spec);
    }
    Some(specs)
}

fn streamable_spec(expr: &ResolvedExpr) -> Option<StreamableAggSpec> {
    match expr {
        ResolvedExpr::Function {
            name,
            distinct,
            args,
        } => {
            if *distinct {
                return None;
            }
            let name = name.to_ascii_lowercase();
            let kind = match name.as_str() {
                "count" if args.is_empty() => StreamableAggKind::CountAll,
                "count" if args.len() == 1 => StreamableAggKind::CountField,
                "sum" if args.len() == 1 => StreamableAggKind::Sum,
                "min" if args.len() == 1 => StreamableAggKind::Min,
                "max" if args.len() == 1 => StreamableAggKind::Max,
                "avg" if args.len() == 1 => StreamableAggKind::Avg,
                _ => return None,
            };
            let arg = if args.is_empty() {
                None
            } else {
                Some(args[0].clone())
            };
            Some(StreamableAggSpec { kind, arg })
        }
        _ => None,
    }
}

fn cmp_values_total(a: &LoraValue, b: &LoraValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (LoraValue::Int(x), LoraValue::Int(y)) => x.cmp(y),
        (LoraValue::Float(x), LoraValue::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (LoraValue::Int(x), LoraValue::Float(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (LoraValue::Float(x), LoraValue::Int(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (LoraValue::String(x), LoraValue::String(y)) => x.cmp(y),
        (LoraValue::Bool(x), LoraValue::Bool(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// Lazy-buffered aggregation source. Aggregation must observe every
/// input row before it can emit the first group, so this source drains
/// upstream on first pull, builds grouped rows, then yields them one
/// at a time to downstream consumers.
pub struct HashAggregationSource<'a, S: GraphStorage> {
    state: HashAggregationState<'a, S>,
}

enum HashAggregationState<'a, S: GraphStorage> {
    Pending {
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        group_by: &'a [ResolvedProjection],
        aggregates: &'a [ResolvedProjection],
    },
    Yielding(std::vec::IntoIter<Row>),
}

impl<'a, S: GraphStorage> HashAggregationSource<'a, S> {
    pub(super) fn new(
        upstream: Box<dyn RowSource + 'a>,
        ctx: StreamCtx<'a, S>,
        group_by: &'a [ResolvedProjection],
        aggregates: &'a [ResolvedProjection],
    ) -> Self {
        Self {
            state: HashAggregationState::Pending {
                upstream,
                ctx,
                group_by,
                aggregates,
            },
        }
    }

    fn materialize(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        group_by: &[ResolvedProjection],
        aggregates: &[ResolvedProjection],
    ) -> ExecResult<Vec<Row>> {
        // Fast path: when every aggregate is a fold-only function (count,
        // sum, min, max, avg, all without DISTINCT), compute the aggregate
        // running state per group as we iterate the upstream — never
        // buffering the input rows. This turns aggregation memory from
        // O(input_rows) into O(groups), which on large scans is the
        // difference between MB allocations and KB.
        if let Some(specs) = classify_streamable_aggregates(aggregates) {
            return Self::materialize_streaming(upstream, ctx, group_by, aggregates, &specs);
        }

        let input_rows = drain(upstream.as_mut())?;
        let eval_ctx = ctx.eval_ctx();
        let mut groups: BTreeMap<Vec<GroupValueKey>, Vec<Row>> = BTreeMap::new();

        if group_by.is_empty() {
            groups.insert(Vec::new(), input_rows);
        } else {
            for row in input_rows {
                let mut key = Vec::with_capacity(group_by.len());
                for proj in group_by {
                    let value = eval_expr_result(&proj.expr, &row, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?;
                    key.push(GroupValueKey::from_value(&value));
                }
                groups.entry(key).or_default().push(row);
            }
        }

        let mut out = Vec::new();
        for rows in groups.into_values() {
            let mut result = Row::new();
            if let Some(first) = rows.first() {
                for proj in group_by {
                    let value = eval_expr_result(&proj.expr, first, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?;
                    let value = hydrate_value(value, ctx.storage);
                    result.insert_named(proj.output, proj.name.clone(), value);
                }
            }
            for proj in aggregates {
                let value = compute_aggregate_expr(&proj.expr, &rows, &eval_ctx)?;
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            out.push(result);
        }

        Ok(out)
    }

    /// Streaming fold path: build per-group running aggregate state as we
    /// pull each upstream row, then emit one output row per group at the
    /// end. Memory is O(groups), not O(input_rows).
    fn materialize_streaming(
        upstream: &mut Box<dyn RowSource + 'a>,
        ctx: &StreamCtx<'a, S>,
        group_by: &[ResolvedProjection],
        aggregates: &[ResolvedProjection],
        specs: &[StreamableAggSpec],
    ) -> ExecResult<Vec<Row>> {
        let eval_ctx = ctx.eval_ctx();

        // No-group-by fast path: skip the `BTreeMap` entirely and fold into
        // a single accumulator. The BTreeMap entry/insert overhead per row
        // dominates pure `RETURN count(*)` workloads at scale, and there is
        // no point indexing groups when there's only ever one.
        if group_by.is_empty() {
            let mut aggs: Vec<AggState> = specs.iter().map(|s| AggState::seed(s.kind)).collect();
            while let Some(row) = upstream.next_row()? {
                for (i, spec) in specs.iter().enumerate() {
                    let value = match &spec.arg {
                        Some(arg) => eval_expr_result(arg, &row, &eval_ctx)
                            .map_err(ExecutorError::RuntimeError)?,
                        None => LoraValue::Null,
                    };
                    aggs[i].fold(spec.kind, value);
                }
            }
            let mut result = Row::new();
            for (i, proj) in aggregates.iter().enumerate() {
                let value = std::mem::replace(&mut aggs[i], AggState::seed(specs[i].kind))
                    .finalize(specs[i].kind);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            return Ok(vec![result]);
        }

        let mut groups: BTreeMap<Vec<GroupValueKey>, StreamingGroup> = BTreeMap::new();

        while let Some(row) = upstream.next_row()? {
            let mut key = Vec::with_capacity(group_by.len());
            for proj in group_by {
                let value = eval_expr_result(&proj.expr, &row, &eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                key.push(GroupValueKey::from_value(&value));
            }

            // First time we see this key, capture the row as the
            // representative for group_by output evaluation. Subsequent
            // rows in the same group only feed the aggregates.
            let entry = groups
                .entry(key)
                .or_insert_with(|| StreamingGroup::new(specs, row.clone()));

            for (i, spec) in specs.iter().enumerate() {
                let value = match &spec.arg {
                    Some(arg) => eval_expr_result(arg, &row, &eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?,
                    None => LoraValue::Null,
                };
                entry.aggs[i].fold(spec.kind, value);
            }
        }

        let mut out = Vec::with_capacity(groups.len());
        for group in groups.into_values() {
            let mut result = Row::new();
            for proj in group_by {
                let value = eval_expr_result(&proj.expr, &group.first_row, &eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                let value = hydrate_value(value, ctx.storage);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            for (i, proj) in aggregates.iter().enumerate() {
                let value = group.aggs[i].clone().finalize(specs[i].kind);
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            out.push(result);
        }
        Ok(out)
    }
}

impl<'a, S: GraphStorage> RowSource for HashAggregationSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        loop {
            match &mut self.state {
                HashAggregationState::Pending {
                    upstream,
                    ctx,
                    group_by,
                    aggregates,
                } => {
                    let rows = Self::materialize(upstream, ctx, group_by, aggregates)?;
                    self.state = HashAggregationState::Yielding(rows.into_iter());
                }
                HashAggregationState::Yielding(it) => return Ok(it.next()),
            }
        }
    }
}
