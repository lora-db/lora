//! Buffered hash-aggregation helpers shared by read-only and mutable
//! executors.

use std::collections::BTreeMap;

use lora_analyzer::{ResolvedExpr, ResolvedProjection};
use lora_store::GraphStorage;

use crate::errors::{ExecResult, ExecutorError};
use crate::eval::{eval_expr_result, EvalContext};
use crate::value::{LoraValue, Row};

use super::helpers::{
    compute_aggregate_expr, project_item, GroupValueKey, RowKeyBuilder, RowKeyPart,
};

/// Group `input_rows` and fold `aggregates` per group.
///
/// Grouping-key values are emitted exactly as evaluated: a node or
/// relationship stays a `Node(id)` / `Relationship(id)` reference (also
/// when nested inside a list or map key), so downstream clauses
/// (`CREATE`, `MERGE`, `SET`, expansion, `id()`, equality) still see the
/// entity. Hydration to the `{id, labels, properties}` output form
/// happens only at the top of the pipeline.
pub(crate) fn aggregate_rows<S>(
    input_rows: Vec<Row>,
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
{
    if let Some(specs) = crate::pull::classify_streamable_aggregates(aggregates) {
        return aggregate_streamable_rows(input_rows, group_by, aggregates, &specs, eval_ctx);
    }

    aggregate_buffered_rows(input_rows, group_by, aggregates, eval_ctx)
}

fn aggregate_buffered_rows<S>(
    input_rows: Vec<Row>,
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
{
    let mut groups: BTreeMap<Vec<RowKeyPart>, Vec<Row>> = BTreeMap::new();

    if group_by.is_empty() {
        groups.insert(Vec::new(), input_rows);
    } else {
        let mut keys = RowKeyBuilder::default();
        let mut key = Vec::with_capacity(group_by.len());
        for row in input_rows {
            group_key(&mut keys, &mut key, group_by, &row, eval_ctx)?;
            match groups.get_mut(key.as_slice()) {
                Some(rows) => rows.push(row),
                None => {
                    groups.insert(std::mem::take(&mut key), vec![row]);
                }
            }
        }
    }

    let mut out = Vec::with_capacity(groups.len());
    for rows in groups.into_values() {
        out.push(build_buffered_group_row(
            &rows, group_by, aggregates, eval_ctx,
        )?);
    }

    Ok(out)
}

fn aggregate_streamable_rows<S>(
    input_rows: Vec<Row>,
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    specs: &[crate::pull::StreamableAggSpec],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<Row>>
where
    S: GraphStorage,
{
    if group_by.is_empty() {
        return aggregate_single_group(input_rows, aggregates, specs, eval_ctx);
    }

    let mut groups: BTreeMap<Vec<RowKeyPart>, StreamingGroup> = BTreeMap::new();
    let mut keys = RowKeyBuilder::default();
    let mut key = Vec::with_capacity(group_by.len());

    for row in input_rows {
        group_key(&mut keys, &mut key, group_by, &row, eval_ctx)?;
        let entry = match groups.get_mut(key.as_slice()) {
            Some(entry) => entry,
            None => groups
                .entry(std::mem::take(&mut key))
                .or_insert_with(|| StreamingGroup::new(specs, row.clone())),
        };
        fold_streaming_aggs(&mut entry.aggs, specs, &row, eval_ctx)?;
    }

    let mut out = Vec::with_capacity(groups.len());
    for group in groups.into_values() {
        let mut result = Row::new();
        insert_group_by_values(&mut result, group_by, &group.first_row, eval_ctx)?;
        insert_finalized_aggs(&mut result, aggregates, specs, group.aggs)?;
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
    insert_finalized_aggs(&mut result, aggregates, specs, aggs)?;
    Ok(vec![result])
}

fn build_buffered_group_row<S>(
    rows: &[Row],
    group_by: &[ResolvedProjection],
    aggregates: &[ResolvedProjection],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Row>
where
    S: GraphStorage,
{
    let mut result = Row::new();

    if let Some(first) = rows.first() {
        insert_group_by_values(&mut result, group_by, first, eval_ctx)?;
    }

    for proj in aggregates {
        let value = compute_aggregate_expr(&proj.expr, rows, eval_ctx)?;
        result.insert_named(proj.output, proj.name.clone(), value);
    }

    Ok(result)
}

pub(crate) fn insert_group_by_values<S>(
    result: &mut Row,
    group_by: &[ResolvedProjection],
    row: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<()>
where
    S: GraphStorage,
{
    for proj in group_by {
        project_item(result, row, proj, eval_ctx)?;
    }

    Ok(())
}

/// `row`'s grouping key, built in `key` (cleared first). A grouping key
/// that is a bare variable is keyed from the row's slot: a carried list is
/// neither copied nor, when the previous row shared it, keyed again.
///
/// Callers reuse one `key` across rows and look groups up by the borrowed
/// slice, so a row of an existing group allocates no key; only a new
/// group's key is moved into the map.
pub(crate) fn group_key<S>(
    keys: &mut RowKeyBuilder,
    key: &mut Vec<RowKeyPart>,
    group_by: &[ResolvedProjection],
    row: &Row,
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<()>
where
    S: GraphStorage,
{
    key.clear();
    for (column, proj) in group_by.iter().enumerate() {
        if let ResolvedExpr::Variable(var) = &proj.expr {
            if let Some(value) = row.get(*var) {
                key.push(keys.part(column, row, *var, value));
                continue;
            }
        }
        let value =
            eval_expr_result(&proj.expr, row, eval_ctx).map_err(ExecutorError::from_eval)?;
        key.push(RowKeyPart::Inline(GroupValueKey::from_value(&value)));
    }
    Ok(())
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
        crate::pull::fold_one(agg, spec, row, eval_ctx)?;
    }

    Ok(())
}

fn insert_finalized_aggs(
    result: &mut Row,
    aggregates: &[ResolvedProjection],
    specs: &[crate::pull::StreamableAggSpec],
    aggs: Vec<crate::pull::AggState>,
) -> ExecResult<()> {
    for ((proj, spec), agg) in aggregates.iter().zip(specs).zip(aggs) {
        result.insert_named(proj.output, proj.name.clone(), agg.finalize(spec.kind)?);
    }
    Ok(())
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
