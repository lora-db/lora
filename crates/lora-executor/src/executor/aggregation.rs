//! Buffered hash-aggregation helpers shared by read-only and mutable
//! executors.

use std::collections::BTreeMap;

use lora_analyzer::ResolvedProjection;
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::{eval_expr_result, EvalContext};
use crate::value::{LoraValue, Row};

use super::helpers::{compute_aggregate_expr, GroupValueKey};

pub(crate) fn aggregate_rows<S, H>(
    input_rows: Vec<Row>,
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    eval_ctx: &EvalContext<'_, S>,
    hydrate_value: H,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
    H: Fn(LoraValue) -> LoraValue,
{
    if let Some(specs) = crate::pull::classify_streamable_aggregates(aggregates) {
        return aggregate_streamable_rows(
            input_rows,
            group_by,
            aggregates,
            &specs,
            eval_ctx,
            hydrate_value,
        );
    }

    aggregate_buffered_rows(input_rows, group_by, aggregates, eval_ctx, hydrate_value)
}

fn aggregate_buffered_rows<S, H>(
    input_rows: Vec<Row>,
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    eval_ctx: &EvalContext<'_, S>,
    hydrate_value: H,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
    H: Fn(LoraValue) -> LoraValue,
{
    let mut groups: BTreeMap<Vec<GroupValueKey>, Vec<Row>> = BTreeMap::new();

    if group_by.is_empty() {
        groups.insert(Vec::new(), input_rows);
    } else {
        for row in input_rows {
            let key = group_key(group_by, &row, eval_ctx)?;
            groups.entry(key).or_default().push(row);
        }
    }

    let mut out = Vec::with_capacity(groups.len());
    for rows in groups.into_values() {
        out.push(build_buffered_group_row(
            &rows,
            group_by,
            aggregates,
            eval_ctx,
            &hydrate_value,
        )?);
    }

    Ok(out)
}

fn aggregate_streamable_rows<S, H>(
    input_rows: Vec<Row>,
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    specs: &[crate::pull::StreamableAggSpec],
    eval_ctx: &EvalContext<'_, S>,
    hydrate_value: H,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
    H: Fn(LoraValue) -> LoraValue,
{
    if group_by.is_empty() {
        return aggregate_single_group(input_rows, aggregates, specs, eval_ctx);
    }

    let mut groups: BTreeMap<Vec<GroupValueKey>, StreamingGroup> = BTreeMap::new();

    for row in input_rows {
        let key = group_key(group_by, &row, eval_ctx)?;
        let entry = groups
            .entry(key)
            .or_insert_with(|| StreamingGroup::new(specs, row.clone()));
        fold_streaming_aggs(&mut entry.aggs, specs, &row, eval_ctx)?;
    }

    let mut out = Vec::with_capacity(groups.len());
    for group in groups.into_values() {
        let mut result = Row::new();
        insert_group_by_values(
            &mut result,
            group_by,
            &group.first_row,
            eval_ctx,
            &hydrate_value,
        )?;
        insert_finalized_aggs(&mut result, aggregates, specs, group.aggs);
        out.push(result);
    }

    Ok(out)
}

fn aggregate_single_group<S>(
    input_rows: Vec<Row>,
    aggregates: &[ResolvedProjection],
    specs: &[crate::pull::StreamableAggSpec],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
{
    if specs
        .iter()
        .all(|spec| matches!(spec.kind, crate::pull::StreamableAggKind::CountAll))
    {
        let count = LoraValue::Int(input_rows.len() as i64);
        let mut result = Row::new();
        for proj in aggregates {
            result.insert_named(proj.output, proj.name.clone(), count.clone());
        }
        return Ok(vec![result]);
    }

    let mut aggs = seed_aggs(specs);
    for row in &input_rows {
        fold_streaming_aggs(&mut aggs, specs, row, eval_ctx)?;
    }

    let mut result = Row::new();
    insert_finalized_aggs(&mut result, aggregates, specs, aggs);
    Ok(vec![result])
}

fn build_buffered_group_row<S, H>(
    rows: &[Row],
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    eval_ctx: &EvalContext<'_, S>,
    hydrate_value: H,
) -> ExecResult<Row>
where
    S: GraphStorage,
    H: Fn(LoraValue) -> LoraValue,
{
    let mut result = Row::new();

    if let Some(first) = rows.first() {
        insert_group_by_values(&mut result, group_by, first, eval_ctx, hydrate_value)?;
    }

    for proj in aggregates {
        let value = compute_aggregate_expr(&proj.expr, rows, eval_ctx)?;
        result.insert_named(proj.output, proj.name.clone(), value);
    }

    Ok(result)
}

fn insert_group_by_values<S, H>(
    result: &mut Row,
    group_by: &[ResolvedProjection],
    row: &Row,
    eval_ctx: &EvalContext<'_, S>,
    hydrate_value: H,
) -> ExecResult<()>
where
    S: GraphStorage,
    H: Fn(LoraValue) -> LoraValue,
{
    for proj in group_by {
        let value =
            eval_expr_result(&proj.expr, row, eval_ctx).map_err(ExecutorError::RuntimeError)?;
        result.insert_named(proj.output, proj.name.clone(), hydrate_value(value));
    }

    Ok(())
}

fn group_key<S>(
    group_by: &[ResolvedProjection],
    row: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<GroupValueKey>>
where
    S: GraphStorage,
{
    let mut key = Vec::with_capacity(group_by.len());
    for proj in group_by {
        let value =
            eval_expr_result(&proj.expr, row, eval_ctx).map_err(ExecutorError::RuntimeError)?;
        key.push(GroupValueKey::from_value(&value));
    }
    Ok(key)
}

fn fold_streaming_aggs<S>(
    aggs: &mut [crate::pull::AggState],
    specs: &[crate::pull::StreamableAggSpec],
    row: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<()>
where
    S: GraphStorage,
{
    for (agg, spec) in aggs.iter_mut().zip(specs) {
        let value = match &spec.arg {
            Some(arg) => {
                eval_expr_result(arg, row, eval_ctx).map_err(ExecutorError::RuntimeError)?
            }
            None => LoraValue::Null,
        };
        agg.fold(spec.kind, value);
    }

    Ok(())
}

fn insert_finalized_aggs(
    result: &mut Row,
    aggregates: &[ResolvedProjection],
    specs: &[crate::pull::StreamableAggSpec],
    aggs: Vec<crate::pull::AggState>,
) {
    for ((proj, spec), agg) in aggregates.iter().zip(specs).zip(aggs) {
        result.insert_named(proj.output, proj.name.clone(), agg.finalize(spec.kind));
    }
}

fn seed_aggs(specs: &[crate::pull::StreamableAggSpec]) -> Vec<crate::pull::AggState> {
    specs
        .iter()
        .map(|spec| crate::pull::AggState::seed(spec.kind))
        .collect()
}

struct StreamingGroup {
    first_row: Row,
    aggs: Vec<crate::pull::AggState>,
}

impl StreamingGroup {
    fn new(specs: &[crate::pull::StreamableAggSpec], first_row: Row) -> Self {
        Self {
            first_row,
            aggs: seed_aggs(specs),
        }
    }
}
