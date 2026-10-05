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

use lora_analyzer::{AggregateFunction, ResolvedExpr, ResolvedProjection};
use lora_store::{GraphStorage, LoraDuration};

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::eval_expr_result;
use crate::executor::{compare_values_total, compute_aggregate_expr, GroupValueKey};
use crate::value::{LoraValue, Row};

use super::{drain, RowSource, StreamCtx};

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
    /// `sum()`; also the running total behind `avg()`.
    Sum(Total),
    Min(Option<LoraValue>),
    Max(Option<LoraValue>),
    Avg(Total),
}

/// A running total that keeps each kind of addend apart: integers add
/// exactly (overflow is an error, like `+`), floats as floats, durations
/// as durations. Non-numeric, non-duration values are ignored.
#[derive(Clone, Debug, Default)]
pub(crate) struct Total {
    int: i64,
    float: f64,
    duration: Option<LoraDuration>,
    ints: usize,
    floats: usize,
    durations: usize,
    overflowed: bool,
}

impl Total {
    fn add(&mut self, value: LoraValue) {
        match value {
            LoraValue::Int(i) => {
                match self.int.checked_add(i) {
                    Some(sum) => self.int = sum,
                    None => self.overflowed = true,
                }
                self.ints += 1;
            }
            LoraValue::Float(f) => {
                self.float += f;
                self.floats += 1;
            }
            LoraValue::Duration(d) => {
                let sum = match &self.duration {
                    None => Some(d),
                    Some(cur) => cur.try_add(&d),
                };
                match sum {
                    Some(sum) => self.duration = Some(sum),
                    None => self.overflowed = true,
                }
                self.durations += 1;
            }
            _ => {}
        }
    }

    /// The sum: an integer when every addend was one, a float once any was
    /// a float, a duration for durations; null with no addends.
    fn sum(&self, name: &str) -> ExecResult<LoraValue> {
        if self.overflowed {
            return Err(ExecutorError::RuntimeError(format!("{name}() overflowed")));
        }
        match (&self.duration, self.ints + self.floats) {
            (Some(_), n) if n > 0 => Err(ExecutorError::RuntimeError(format!(
                "{name}() can't add durations and numbers"
            ))),
            (Some(d), _) => Ok(LoraValue::Duration(d.clone())),
            (None, 0) => Ok(LoraValue::Null),
            (None, _) if self.floats == 0 => Ok(LoraValue::Int(self.int)),
            (None, _) => Ok(LoraValue::Float(self.int as f64 + self.float)),
        }
    }

    /// The mean: a float for numbers, a duration for durations.
    fn avg(&self) -> ExecResult<LoraValue> {
        let numbers = self.ints + self.floats;
        match self.sum("avg")? {
            LoraValue::Duration(d) => d
                .try_div_int(self.durations as i64)
                .map(LoraValue::Duration)
                .ok_or_else(|| ExecutorError::RuntimeError("avg() overflowed".into())),
            LoraValue::Int(i) => Ok(LoraValue::Float(i as f64 / numbers as f64)),
            LoraValue::Float(f) => Ok(LoraValue::Float(f / numbers as f64)),
            other => Ok(other),
        }
    }
}

impl AggState {
    pub(crate) fn seed(kind: StreamableAggKind) -> Self {
        match kind {
            StreamableAggKind::CountAll | StreamableAggKind::CountField => AggState::Count(0),
            StreamableAggKind::Sum => AggState::Sum(Total::default()),
            StreamableAggKind::Min => AggState::Min(None),
            StreamableAggKind::Max => AggState::Max(None),
            StreamableAggKind::Avg => AggState::Avg(Total::default()),
        }
    }

    pub(crate) fn fold(&mut self, kind: StreamableAggKind, value: LoraValue) {
        match self {
            AggState::Count(n) => match kind {
                StreamableAggKind::CountAll => *n += 1,
                StreamableAggKind::CountField if !matches!(value, LoraValue::Null) => *n += 1,
                _ => {}
            },
            AggState::Sum(total) | AggState::Avg(total) => total.add(value),
            AggState::Min(slot) => {
                if matches!(value, LoraValue::Null) {
                    return;
                }
                match slot {
                    None => *slot = Some(value),
                    Some(cur) => {
                        if compare_values_total(&value, cur) == std::cmp::Ordering::Less {
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
                        if compare_values_total(&value, cur) == std::cmp::Ordering::Greater {
                            *cur = value;
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn finalize(self, _kind: StreamableAggKind) -> ExecResult<LoraValue> {
        match self {
            AggState::Count(n) => Ok(LoraValue::Int(n)),
            AggState::Sum(total) => total.sum("sum"),
            AggState::Min(v) | AggState::Max(v) => Ok(v.unwrap_or(LoraValue::Null)),
            AggState::Avg(total) => total.avg(),
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
            function,
            distinct,
            args,
        } => {
            if *distinct {
                return None;
            }
            let kind = match function.as_aggregate() {
                Some(AggregateFunction::Count) if args.is_empty() => StreamableAggKind::CountAll,
                Some(AggregateFunction::Count) if args.len() == 1 => StreamableAggKind::CountField,
                Some(AggregateFunction::Sum) if args.len() == 1 => StreamableAggKind::Sum,
                Some(AggregateFunction::Min) if args.len() == 1 => StreamableAggKind::Min,
                Some(AggregateFunction::Max) if args.len() == 1 => StreamableAggKind::Max,
                Some(AggregateFunction::Avg) if args.len() == 1 => StreamableAggKind::Avg,
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
                        .map_err(ExecutorError::from_eval)?;
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
                        .map_err(ExecutorError::from_eval)?;
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
                            .map_err(ExecutorError::from_eval)?,
                        None => LoraValue::Null,
                    };
                    aggs[i].fold(spec.kind, value);
                }
            }
            let mut result = Row::new();
            for (i, proj) in aggregates.iter().enumerate() {
                let value = std::mem::replace(&mut aggs[i], AggState::seed(specs[i].kind))
                    .finalize(specs[i].kind)?;
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            return Ok(vec![result]);
        }

        let mut groups: BTreeMap<Vec<GroupValueKey>, StreamingGroup> = BTreeMap::new();

        while let Some(row) = upstream.next_row()? {
            let mut key = Vec::with_capacity(group_by.len());
            for proj in group_by {
                let value = eval_expr_result(&proj.expr, &row, &eval_ctx)
                    .map_err(ExecutorError::from_eval)?;
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
                    Some(arg) => {
                        eval_expr_result(arg, &row, &eval_ctx).map_err(ExecutorError::from_eval)?
                    }
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
                    .map_err(ExecutorError::from_eval)?;
                result.insert_named(proj.output, proj.name.clone(), value);
            }
            for (i, proj) in aggregates.iter().enumerate() {
                let value = group.aggs[i].clone().finalize(specs[i].kind)?;
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
