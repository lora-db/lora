//! Cross-cutting executor helpers used by both the read-only and the
//! mutable executor (and, via `pub(crate)` re-exports through
//! `super::mod`, by the streaming pull pipeline in `crate::pull`).
//!
//! Roughly four groups:
//!
//! 1. Row-set primitives: `dedup_rows` / `dedup_rows_by_vars` for
//!    UNION / DISTINCT, [`compute_aggregate_expr`] for the buffered
//!    aggregation path, [`compare_sort_item`] for the buffered Sort
//!    operator. The streaming pipeline in `crate::pull` calls
//!    [`compute_aggregate_expr`] when the streamable-fold fast-path
//!    classifier rejects a projection.
//! 2. Label / property scans: [`scan_node_ids_for_label_groups`],
//!    [`indexed_node_property_candidates`],
//!    [`node_matches_label_groups`], [`node_matches_property_filter`],
//!    [`label_group_candidates_prefiltered`]. Both NodeByLabelScan and
//!    NodeByPropertyScan share these helpers across the buffered and
//!    streaming pipelines.
//! 3. Path construction: [`build_path_value`] for `PathBuild`,
//!    [`variable_length_expand`] (the buffered BFS path used as the
//!    fallback under non-streaming variable-length expansions) and
//!    [`filter_shortest_paths`] for SHORTEST PATH.
//! 4. Value classification: [`value_matches_property_value`] (used
//!    by every property prefilter), [`hydrate_node_record`] /
//!    [`hydrate_relationship_record`] (single-record hydration), and
//!    the [`GroupValueKey`] dedup / group key.
//!
//! Also hosts the small `eval_properties_expr`, `eval_aggregate_arg_values`,
//! `eval_first_or_null`, `dedup_values`, `as_f64_lossy`,
//! `compare_values_for_sort`, `compare_values_total`,
//! `single_label_hint`, `property_lookup_values`, `type_rank`,
//! `flatten_label_groups` private helpers, and the
//! `MAX_VAR_LEN_HOPS` cap on unbounded variable-length expansion.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use lora_analyzer::symbols::VarId;
use lora_analyzer::ResolvedExpr;
use lora_ast::{Direction, RangeLiteral};
use lora_compiler::physical::{
    ExpandExec, LimitExec, NodeByLabelScanExec, NodeByPropertyScanExec, NodeScanExec,
    ProjectionExec, UnwindExec,
};
use lora_store::{GraphStorage, NodeId, Properties, PropertyValue, RelationshipId};

use crate::errors::{value_kind, ExecResult, ExecutorError};
use crate::eval::{eval_expr, eval_expr_result, eval_truthy_result, EvalContext};
use crate::value::{lora_value_to_property, LoraPath, LoraValue, Row};

/// Deadline guard. Returns `QueryTimeout` once the deadline has
/// elapsed; both executors call this every operator-level recursion
/// step and from inside per-row inner loops.
#[inline]
pub(super) fn check_deadline_at(deadline: Instant) -> ExecResult<()> {
    if Instant::now() >= deadline {
        Err(ExecutorError::QueryTimeout)
    } else {
        Ok(())
    }
}

pub(super) fn filter_rows_checked<S: GraphStorage>(
    input_rows: Vec<Row>,
    predicate: &ResolvedExpr,
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<Row>> {
    let mut out = Vec::with_capacity(input_rows.len());
    for row in input_rows {
        if eval_truthy_result(predicate, &row, eval_ctx).map_err(ExecutorError::RuntimeError)? {
            out.push(row);
        }
    }
    Ok(out)
}

pub(super) fn project_rows_checked<S: GraphStorage>(
    input_rows: Vec<Row>,
    op: &ProjectionExec,
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<Row>> {
    let mut out = Vec::with_capacity(input_rows.len());

    for row in input_rows {
        if op.include_existing {
            let mut projected = row;
            for item in &op.items {
                let value = eval_expr_result(&item.expr, &projected, eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                projected.insert_named(item.output, item.name.clone(), value);
            }
            out.push(projected);
        } else {
            let mut projected = Row::new();
            for item in &op.items {
                let value = eval_expr_result(&item.expr, &row, eval_ctx)
                    .map_err(ExecutorError::RuntimeError)?;
                projected.insert_named(item.output, item.name.clone(), value);
            }
            out.push(projected);
        }
    }

    Ok(if op.distinct {
        dedup_rows_by_vars(out)
    } else {
        out
    })
}

pub(super) fn unwind_rows<S: GraphStorage>(
    input_rows: Vec<Row>,
    op: &UnwindExec,
    eval_ctx: &EvalContext<'_, S>,
) -> Vec<Row> {
    let mut out = Vec::new();

    for row in input_rows {
        match eval_expr(&op.expr, &row, eval_ctx) {
            LoraValue::List(values) => {
                for value in values {
                    let mut new_row = row.clone();
                    new_row.insert(op.alias, value);
                    out.push(new_row);
                }
            }
            LoraValue::Null => {}
            other => {
                let mut new_row = row;
                new_row.insert(op.alias, other);
                out.push(new_row);
            }
        }
    }

    out
}

pub(super) fn limit_rows<S: GraphStorage>(
    mut rows: Vec<Row>,
    op: &LimitExec,
    eval_ctx: &EvalContext<'_, S>,
) -> Vec<Row> {
    let limit = op
        .limit
        .as_ref()
        .and_then(|e| eval_expr(e, &Row::new(), eval_ctx).as_i64())
        .unwrap_or(rows.len() as i64)
        .max(0) as usize;

    let skip = op
        .skip
        .as_ref()
        .and_then(|e| eval_expr(e, &Row::new(), eval_ctx).as_i64())
        .unwrap_or(0)
        .max(0) as usize;

    if skip >= rows.len() {
        return Vec::new();
    }

    rows.drain(0..skip);
    rows.truncate(limit);
    rows
}

#[inline]
pub(crate) fn bound_node_id_for_expand(row: &Row, var: VarId) -> ExecResult<Option<NodeId>> {
    match row.get(var) {
        Some(LoraValue::Node(id)) => Ok(Some(*id)),
        Some(other) => Err(ExecutorError::ExpectedNodeForExpand {
            var: format!("{var:?}"),
            found: value_kind(other),
        }),
        None => Ok(None),
    }
}

#[inline]
pub(crate) fn bound_relationship_id_for_expand(
    row: &Row,
    var: VarId,
) -> ExecResult<Option<RelationshipId>> {
    match row.get(var) {
        Some(LoraValue::Relationship(id)) => Ok(Some(*id)),
        Some(other) => Err(ExecutorError::ExpectedRelationshipForExpand {
            var: format!("{var:?}"),
            found: value_kind(other),
        }),
        None => Ok(None),
    }
}

pub(super) fn node_scan_rows<S: GraphStorage>(
    storage: &S,
    base_rows: Vec<Row>,
    op: &NodeScanExec,
    deadline: Option<Instant>,
) -> ExecResult<Vec<Row>> {
    let node_ids = storage.all_node_ids();
    let mut out = Vec::with_capacity(base_rows.len().saturating_mul(node_ids.len()));

    if deadline.is_none() {
        for row in base_rows {
            if let Some(existing_id) = bound_node_id_for_expand(&row, op.var)? {
                if storage.has_node(existing_id) {
                    out.push(row);
                }
                continue;
            }

            for &id in &node_ids {
                let mut new_row = row.clone();
                new_row.insert(op.var, LoraValue::Node(id));
                out.push(new_row);
            }
        }
        return Ok(out);
    }

    for row in base_rows {
        check_optional_deadline(deadline)?;
        if let Some(existing_id) = bound_node_id_for_expand(&row, op.var)? {
            if storage.has_node(existing_id) {
                out.push(row);
            }
            continue;
        }

        for &id in &node_ids {
            check_optional_deadline(deadline)?;
            let mut new_row = row.clone();
            new_row.insert(op.var, LoraValue::Node(id));
            out.push(new_row);
        }
    }

    Ok(out)
}

pub(super) fn node_by_label_scan_rows<S: GraphStorage>(
    storage: &S,
    base_rows: Vec<Row>,
    op: &NodeByLabelScanExec,
    deadline: Option<Instant>,
) -> ExecResult<Vec<Row>> {
    let candidate_ids = scan_node_ids_for_label_groups(storage, &op.labels);
    let candidates_prefiltered = label_group_candidates_prefiltered(&op.labels);
    let mut out = Vec::with_capacity(base_rows.len().saturating_mul(candidate_ids.len()));

    if deadline.is_none() {
        for row in base_rows {
            if let Some(existing_id) = bound_node_id_for_expand(&row, op.var)? {
                let labels_ok = storage
                    .with_node(existing_id, |n| {
                        node_matches_label_groups(&n.labels, &op.labels)
                    })
                    .unwrap_or(false);
                if labels_ok {
                    out.push(row);
                }
                continue;
            }

            for &id in &candidate_ids {
                if !candidates_prefiltered {
                    let labels_ok = storage
                        .with_node(id, |n| node_matches_label_groups(&n.labels, &op.labels))
                        .unwrap_or(false);
                    if !labels_ok {
                        continue;
                    }
                }
                let mut new_row = row.clone();
                new_row.insert(op.var, LoraValue::Node(id));
                out.push(new_row);
            }
        }
        return Ok(out);
    }

    for row in base_rows {
        check_optional_deadline(deadline)?;
        if let Some(existing_id) = bound_node_id_for_expand(&row, op.var)? {
            let labels_ok = storage
                .with_node(existing_id, |n| {
                    node_matches_label_groups(&n.labels, &op.labels)
                })
                .unwrap_or(false);
            if labels_ok {
                out.push(row);
            }
            continue;
        }

        for &id in &candidate_ids {
            check_optional_deadline(deadline)?;
            if !candidates_prefiltered {
                let labels_ok = storage
                    .with_node(id, |n| node_matches_label_groups(&n.labels, &op.labels))
                    .unwrap_or(false);
                if !labels_ok {
                    continue;
                }
            }
            let mut new_row = row.clone();
            new_row.insert(op.var, LoraValue::Node(id));
            out.push(new_row);
        }
    }

    Ok(out)
}

pub(super) fn node_by_property_scan_rows<S: GraphStorage>(
    storage: &S,
    params: &BTreeMap<String, LoraValue>,
    base_rows: Vec<Row>,
    op: &NodeByPropertyScanExec,
    deadline: Option<Instant>,
) -> ExecResult<Vec<Row>> {
    let eval_ctx = EvalContext { storage, params };
    let mut out = Vec::new();

    if deadline.is_none() {
        for row in base_rows {
            let expected = eval_expr(&op.value, &row, &eval_ctx);

            if let Some(existing_id) = bound_node_id_for_expand(&row, op.var)? {
                if node_matches_property_filter(
                    storage,
                    existing_id,
                    &op.labels,
                    &op.key,
                    &expected,
                ) {
                    out.push(row);
                }
                continue;
            }

            let candidates =
                indexed_node_property_candidates(storage, &op.labels, &op.key, &expected);
            for id in candidates.ids {
                if !candidates.prefiltered
                    && !node_matches_property_filter(storage, id, &op.labels, &op.key, &expected)
                {
                    continue;
                }
                let mut new_row = row.clone();
                new_row.insert(op.var, LoraValue::Node(id));
                out.push(new_row);
            }
        }
        return Ok(out);
    }

    for row in base_rows {
        check_optional_deadline(deadline)?;
        let expected = eval_expr(&op.value, &row, &eval_ctx);

        if let Some(existing_id) = bound_node_id_for_expand(&row, op.var)? {
            if node_matches_property_filter(storage, existing_id, &op.labels, &op.key, &expected) {
                out.push(row);
            }
            continue;
        }

        let candidates = indexed_node_property_candidates(storage, &op.labels, &op.key, &expected);
        for id in candidates.ids {
            check_optional_deadline(deadline)?;
            if !candidates.prefiltered
                && !node_matches_property_filter(storage, id, &op.labels, &op.key, &expected)
            {
                continue;
            }
            let mut new_row = row.clone();
            new_row.insert(op.var, LoraValue::Node(id));
            out.push(new_row);
        }
    }

    Ok(out)
}

#[inline]
fn check_optional_deadline(deadline: Option<Instant>) -> ExecResult<()> {
    match deadline {
        Some(deadline) => check_deadline_at(deadline),
        None => Ok(()),
    }
}

pub(super) fn expand_rows<S: GraphStorage>(
    storage: &S,
    params: &BTreeMap<String, LoraValue>,
    input_rows: Vec<Row>,
    op: &ExpandExec,
) -> ExecResult<Vec<Row>> {
    let eval_ctx = EvalContext { storage, params };
    let mut out = Vec::new();

    for row in input_rows {
        let Some(src_node_id) = bound_node_id_for_expand(&row, op.src)? else {
            continue;
        };

        let mut rel_property_filter = None;

        storage.try_for_each_expand_id(
            src_node_id,
            op.direction,
            &op.types,
            |rel_id, dst_id| {
                if let Some(expr) = op.rel_properties.as_ref() {
                    if rel_property_filter.is_none() {
                        let expected = eval_expr(expr, &row, &eval_ctx);
                        let LoraValue::Map(map) = expected else {
                            return Err(ExecutorError::ExpectedPropertyMap {
                                found: value_kind(&expected),
                            });
                        };
                        rel_property_filter = Some(map);
                    }

                    let map = rel_property_filter
                        .as_ref()
                        .expect("relationship property filter initialized above");
                    let matches = storage
                        .with_relationship(rel_id, |rel| {
                            map.iter().all(|(key, expected)| {
                                rel.properties
                                    .get(key)
                                    .map(|actual| value_matches_property_value(expected, actual))
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);
                    if !matches {
                        return Ok(());
                    }
                }

                if let Some(existing_id) = bound_node_id_for_expand(&row, op.dst)? {
                    if existing_id != dst_id {
                        return Ok(());
                    }
                }

                if let Some(rel_var) = op.rel {
                    if let Some(existing_id) = bound_relationship_id_for_expand(&row, rel_var)? {
                        if existing_id != rel_id {
                            return Ok(());
                        }
                    }
                }

                let mut new_row = row.clone();
                if !new_row.contains_key(op.dst) {
                    new_row.insert(op.dst, LoraValue::Node(dst_id));
                }
                if let Some(rel_var) = op.rel {
                    if !new_row.contains_key(rel_var) {
                        new_row.insert(rel_var, LoraValue::Relationship(rel_id));
                    }
                }
                out.push(new_row);
                Ok(())
            },
        )?;
    }

    Ok(out)
}

pub(super) fn expand_var_len_rows<S: GraphStorage>(
    storage: &S,
    input_rows: Vec<Row>,
    op: &ExpandExec,
    range: &RangeLiteral,
) -> ExecResult<Vec<Row>> {
    let (min_hops, max_hops) = resolve_range(range);
    let bind_relationships = op.rel.is_some();
    let mut out = Vec::new();

    for row in input_rows {
        let Some(src_node_id) = bound_node_id_for_expand(&row, op.src)? else {
            continue;
        };

        let expansions = variable_length_expand(
            storage,
            src_node_id,
            op.direction,
            &op.types,
            min_hops,
            max_hops,
            bind_relationships,
        );

        for result in expansions {
            let mut new_row = row.clone();
            new_row.insert(op.dst, LoraValue::Node(result.dst_node_id));

            if let Some(rel_var) = op.rel {
                let rel_list = LoraValue::List(
                    result
                        .rel_ids
                        .into_iter()
                        .map(LoraValue::Relationship)
                        .collect(),
                );
                new_row.insert(rel_var, rel_list);
            }

            out.push(new_row);
        }
    }

    Ok(out)
}

pub(super) fn properties_to_value_map(props: &Properties) -> LoraValue {
    let mut map = BTreeMap::new();
    for (k, v) in props.iter() {
        map.insert(k.clone(), LoraValue::from(v));
    }
    LoraValue::Map(map)
}

/// Dedup rows that share the same schema (same VarId set). Compares rows by
/// a Vec<GroupValueKey> keyed on VarId iteration order — avoids the per-row
/// column-name String clones of `dedup_rows`. Used by DISTINCT projection.
pub(crate) fn dedup_rows_by_vars(rows: Vec<Row>) -> Vec<Row> {
    let mut seen: BTreeSet<Vec<GroupValueKey>> = BTreeSet::new();
    let mut out = Vec::new();

    for row in rows {
        let key: Vec<GroupValueKey> = row
            .iter()
            .map(|(_, val)| GroupValueKey::from_value(val))
            .collect();
        if seen.insert(key) {
            out.push(row);
        }
    }

    out
}

/// Dedup rows using named entries so rows with different VarIds but the same
/// column name + value are collapsed. Needed for UNION where each branch has
/// its own VarIds.
pub(crate) fn dedup_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut seen: BTreeSet<Vec<(String, GroupValueKey)>> = BTreeSet::new();
    let mut out = Vec::new();

    for row in rows {
        let key: Vec<(String, GroupValueKey)> = row
            .iter_named()
            .map(|(_, name, val)| (name.into_owned(), GroupValueKey::from_value(val)))
            .collect();
        if seen.insert(key) {
            out.push(row);
        }
    }

    out
}

pub(super) fn eval_properties_expr<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    storage: &S,
    params: &BTreeMap<String, LoraValue>,
) -> ExecResult<Properties> {
    let eval_ctx = EvalContext { storage, params };

    match eval_expr(expr, row, &eval_ctx) {
        LoraValue::Map(map) => {
            let mut out = Properties::new();
            for (k, v) in map {
                let prop = lora_value_to_property(v)
                    .map_err(|e| ExecutorError::RuntimeError(e.to_string()))?;
                out.insert(k, prop);
            }
            Ok(out)
        }
        other => Err(ExecutorError::ExpectedPropertyMap {
            found: value_kind(&other),
        }),
    }
}

pub(crate) fn compute_aggregate_expr<S: GraphStorage>(
    expr: &ResolvedExpr,
    rows: &[Row],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<LoraValue> {
    match expr {
        ResolvedExpr::Function {
            name,
            distinct,
            args,
        } => {
            let func = name.to_ascii_lowercase();

            match func.as_str() {
                "count" => {
                    if args.is_empty() {
                        return Ok(LoraValue::Int(rows.len() as i64));
                    }

                    let mut values = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?;
                    values.retain(|v| !matches!(v, LoraValue::Null));

                    if *distinct {
                        values = dedup_values(values);
                    }

                    Ok(LoraValue::Int(values.len() as i64))
                }

                "collect" => {
                    if args.is_empty() {
                        return Ok(LoraValue::List(Vec::new()));
                    }

                    let mut values = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?;

                    if *distinct {
                        values = dedup_values(values);
                    }

                    Ok(LoraValue::List(values))
                }

                "sum" => {
                    if args.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    let mut values = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?;

                    if *distinct {
                        values = dedup_values(values);
                    }

                    let nums = values
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect::<Vec<_>>();

                    if nums.is_empty() {
                        Ok(LoraValue::Null)
                    } else if nums.iter().all(|n| n.fract() == 0.0) {
                        Ok(LoraValue::Int(nums.iter().sum::<f64>() as i64))
                    } else {
                        Ok(LoraValue::Float(nums.iter().sum::<f64>()))
                    }
                }

                "avg" => {
                    if args.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    let mut values = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?;

                    if *distinct {
                        values = dedup_values(values);
                    }

                    let nums = values
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect::<Vec<_>>();

                    if nums.is_empty() {
                        Ok(LoraValue::Null)
                    } else {
                        Ok(LoraValue::Float(
                            nums.iter().sum::<f64>() / nums.len() as f64,
                        ))
                    }
                }

                "min" => {
                    if args.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    let mut values = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?;
                    values.retain(|v| !matches!(v, LoraValue::Null));

                    if *distinct {
                        values = dedup_values(values);
                    }

                    Ok(values
                        .into_iter()
                        .min_by(compare_values_total)
                        .unwrap_or(LoraValue::Null))
                }

                "max" => {
                    if args.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    let mut values = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?;
                    values.retain(|v| !matches!(v, LoraValue::Null));

                    if *distinct {
                        values = dedup_values(values);
                    }

                    Ok(values
                        .into_iter()
                        .max_by(compare_values_total)
                        .unwrap_or(LoraValue::Null))
                }

                "stdev" | "stdevp" => {
                    if args.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    let nums: Vec<f64> = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect();

                    let is_population = func == "stdevp";

                    if nums.is_empty() || (!is_population && nums.len() < 2) {
                        return Ok(LoraValue::Float(0.0));
                    }

                    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                    let variance_sum: f64 = nums.iter().map(|x| (x - mean).powi(2)).sum();
                    let denom = if is_population {
                        nums.len() as f64
                    } else {
                        (nums.len() - 1) as f64
                    };
                    Ok(LoraValue::Float((variance_sum / denom).sqrt()))
                }

                "percentilecont" => {
                    if args.len() < 2 {
                        return Ok(LoraValue::Null);
                    }

                    let Some(first) = rows.first() else {
                        return Ok(LoraValue::Null);
                    };

                    let percentile = eval_expr_result(&args[1], first, eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?
                        .as_f64()
                        .unwrap_or(0.5);
                    let mut nums: Vec<f64> = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect();

                    if nums.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

                    let index = percentile * (nums.len() - 1) as f64;
                    let lower = index.floor() as usize;
                    let upper = index.ceil() as usize;
                    let fraction = index - lower as f64;

                    if lower == upper || upper >= nums.len() {
                        Ok(LoraValue::Float(nums[lower]))
                    } else {
                        Ok(LoraValue::Float(
                            nums[lower] * (1.0 - fraction) + nums[upper] * fraction,
                        ))
                    }
                }

                "percentiledisc" => {
                    if args.len() < 2 {
                        return Ok(LoraValue::Null);
                    }

                    let Some(first) = rows.first() else {
                        return Ok(LoraValue::Null);
                    };

                    let percentile = eval_expr_result(&args[1], first, eval_ctx)
                        .map_err(ExecutorError::RuntimeError)?
                        .as_f64()
                        .unwrap_or(0.5);
                    let mut nums: Vec<f64> = eval_aggregate_arg_values(&args[0], rows, eval_ctx)?
                        .into_iter()
                        .filter_map(as_f64_lossy)
                        .collect();

                    if nums.is_empty() {
                        return Ok(LoraValue::Null);
                    }

                    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

                    let index = (percentile * (nums.len() - 1) as f64).round() as usize;
                    let index = index.min(nums.len() - 1);
                    Ok(LoraValue::Float(nums[index]))
                }

                _ => eval_first_or_null(expr, rows, eval_ctx),
            }
        }

        _ => eval_first_or_null(expr, rows, eval_ctx),
    }
}

fn eval_aggregate_arg_values<S: GraphStorage>(
    expr: &ResolvedExpr,
    rows: &[Row],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<Vec<LoraValue>> {
    rows.iter()
        .map(|row| eval_expr_result(expr, row, eval_ctx).map_err(ExecutorError::RuntimeError))
        .collect()
}

fn eval_first_or_null<S: GraphStorage>(
    expr: &ResolvedExpr,
    rows: &[Row],
    eval_ctx: &EvalContext<'_, S>,
) -> ExecResult<LoraValue> {
    match rows.first() {
        Some(row) => eval_expr_result(expr, row, eval_ctx).map_err(ExecutorError::RuntimeError),
        None => Ok(LoraValue::Null),
    }
}

fn dedup_values(values: Vec<LoraValue>) -> Vec<LoraValue> {
    let mut seen: BTreeSet<GroupValueKey> = BTreeSet::new();
    let mut out = Vec::new();

    for value in values {
        let key = GroupValueKey::from_value(&value);
        if seen.insert(key) {
            out.push(value);
        }
    }

    out
}

fn as_f64_lossy(v: LoraValue) -> Option<f64> {
    match v {
        LoraValue::Int(i) => Some(i as f64),
        LoraValue::Float(f) => Some(f),
        _ => None,
    }
}

pub(super) fn compare_values_total(a: &LoraValue, b: &LoraValue) -> Ordering {
    use LoraValue::*;

    match (a, b) {
        (Bool(x), Bool(y)) => x.cmp(y),
        (Int(x), Int(y)) => x.cmp(y),
        (Float(x), Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Int(x), Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal),
        (Float(x), Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal),
        (String(x), String(y)) => x.cmp(y),
        (Binary(x), Binary(y)) => x.segments().cmp(y.segments()),
        (Node(x), Node(y)) => x.cmp(y),
        (Relationship(x), Relationship(y)) => x.cmp(y),
        (Date(x), Date(y)) => x.cmp(y),
        (DateTime(x), DateTime(y)) => x.cmp(y),
        (Duration(x), Duration(y)) => x.cmp(y),
        (Vector(x), Vector(y)) => x.to_key_string().cmp(&y.to_key_string()),
        _ => type_rank(a)
            .cmp(&type_rank(b))
            .then_with(|| format!("{a:?}").cmp(&format!("{b:?}"))),
    }
}

pub fn value_matches_property_value(expected: &LoraValue, actual: &PropertyValue) -> bool {
    match (expected, actual) {
        (LoraValue::Null, PropertyValue::Null) => true,
        (LoraValue::Bool(a), PropertyValue::Bool(b)) => a == b,
        (LoraValue::Int(a), PropertyValue::Int(b)) => a == b,
        (LoraValue::Float(a), PropertyValue::Float(b)) => a == b,
        (LoraValue::Int(a), PropertyValue::Float(b)) => (*a as f64) == *b,
        (LoraValue::Float(a), PropertyValue::Int(b)) => *a == (*b as f64),
        (LoraValue::String(a), PropertyValue::String(b)) => a == b,
        (LoraValue::Binary(a), PropertyValue::Binary(b)) => a == b,

        (LoraValue::List(xs), PropertyValue::List(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| value_matches_property_value(x, y))
        }

        (LoraValue::Map(xm), PropertyValue::Map(ym)) => xm.iter().all(|(k, xv)| {
            ym.get(k)
                .map(|yv| value_matches_property_value(xv, yv))
                .unwrap_or(false)
        }),

        (LoraValue::Date(a), PropertyValue::Date(b)) => a == b,
        (LoraValue::DateTime(a), PropertyValue::DateTime(b)) => a == b,
        (LoraValue::LocalDateTime(a), PropertyValue::LocalDateTime(b)) => a == b,
        (LoraValue::Time(a), PropertyValue::Time(b)) => a == b,
        (LoraValue::LocalTime(a), PropertyValue::LocalTime(b)) => a == b,
        (LoraValue::Duration(a), PropertyValue::Duration(b)) => a == b,
        (LoraValue::Point(a), PropertyValue::Point(b)) => a == b,
        (LoraValue::Vector(a), PropertyValue::Vector(b)) => a == b,

        _ => false,
    }
}

pub(crate) fn node_matches_property_filter<S: GraphStorage>(
    storage: &S,
    node_id: NodeId,
    labels: &[Vec<String>],
    key: &str,
    expected: &LoraValue,
) -> bool {
    storage
        .with_node(node_id, |node| {
            node_matches_label_groups(&node.labels, labels)
                && node
                    .properties
                    .get(key)
                    .map(|actual| value_matches_property_value(expected, actual))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn single_label_hint(labels: &[Vec<String>]) -> Option<&str> {
    if labels.len() == 1 && labels[0].len() == 1 {
        Some(labels[0][0].as_str())
    } else {
        None
    }
}

fn property_lookup_values(expected: &LoraValue) -> Option<Vec<PropertyValue>> {
    let property = lora_value_to_property(expected.clone()).ok()?;
    let mut values = vec![property.clone()];

    match property {
        PropertyValue::Int(i) => {
            values.push(PropertyValue::Float(i as f64));
        }
        PropertyValue::Float(f)
            if f.is_finite()
                && f.fract() == 0.0
                && f >= i64::MIN as f64
                && f <= i64::MAX as f64 =>
        {
            values.push(PropertyValue::Int(f as i64));
        }
        _ => {}
    }

    Some(values)
}

pub(crate) struct NodePropertyCandidates {
    pub(crate) ids: Vec<NodeId>,
    pub(crate) prefiltered: bool,
}

pub(crate) fn indexed_node_property_candidates<S: GraphStorage>(
    storage: &S,
    labels: &[Vec<String>],
    key: &str,
    expected: &LoraValue,
) -> NodePropertyCandidates {
    let Some(values) = property_lookup_values(expected) else {
        return NodePropertyCandidates {
            ids: scan_node_ids_for_label_groups(storage, labels),
            prefiltered: false,
        };
    };

    let label_hint = single_label_hint(labels);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        for id in storage.find_node_ids_by_property(label_hint, key, &value) {
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    NodePropertyCandidates {
        ids: out,
        prefiltered: labels.is_empty() || label_hint.is_some(),
    }
}

/// Build a LoraPath from the node and relationship variables currently in a row.
///
/// For variable-length relationships (stored as a List of Relationship values),
/// intermediate nodes are reconstructed from the storage by walking the
/// relationship chain.
pub(crate) fn build_path_value<S: GraphStorage>(
    row: &Row,
    node_vars: &[VarId],
    rel_vars: &[VarId],
    storage: &S,
) -> LoraValue {
    let (raw_nodes, rels, has_var_len) = path_bindings(row, node_vars, rel_vars);

    let nodes = if has_var_len && !rels.is_empty() && raw_nodes.len() == 2 {
        reconstruct_var_len_nodes(raw_nodes[0], &rels, storage)
    } else {
        raw_nodes
    };

    LoraValue::Path(LoraPath { nodes, rels })
}

#[inline]
fn path_bindings(
    row: &Row,
    node_vars: &[VarId],
    rel_vars: &[VarId],
) -> (Vec<NodeId>, Vec<RelationshipId>, bool) {
    let mut raw_nodes = Vec::new();
    let mut rels = Vec::new();
    let mut has_var_len = false;

    for &nv in node_vars {
        match row.get(nv) {
            Some(LoraValue::Node(id)) => raw_nodes.push(*id),
            Some(LoraValue::List(items)) => {
                for item in items {
                    if let LoraValue::Node(id) = item {
                        raw_nodes.push(*id);
                    }
                }
            }
            _ => {}
        }
    }

    for &rv in rel_vars {
        match row.get(rv) {
            Some(LoraValue::Relationship(id)) => rels.push(*id),
            Some(LoraValue::List(items)) => {
                has_var_len = true;
                for item in items {
                    if let LoraValue::Relationship(id) = item {
                        rels.push(*id);
                    }
                }
            }
            _ => {}
        }
    }

    (raw_nodes, rels, has_var_len)
}

#[inline]
fn reconstruct_var_len_nodes<S: GraphStorage>(
    start: NodeId,
    rels: &[RelationshipId],
    storage: &S,
) -> Vec<NodeId> {
    let mut ordered = Vec::with_capacity(rels.len() + 1);
    ordered.push(start);
    let mut current = start;
    for &rel_id in rels {
        if let Some((src, dst)) = storage.relationship_endpoints(rel_id) {
            let next = if src == current { dst } else { src };
            ordered.push(next);
            current = next;
        }
    }
    ordered
}

fn type_rank(v: &LoraValue) -> u8 {
    match v {
        LoraValue::Null => 0,
        LoraValue::Bool(_) => 1,
        LoraValue::Int(_) | LoraValue::Float(_) => 2,
        LoraValue::String(_) => 3,
        LoraValue::Binary(_) => 4,
        LoraValue::Date(_) => 5,
        LoraValue::DateTime(_) => 6,
        LoraValue::LocalDateTime(_) => 7,
        LoraValue::Time(_) => 8,
        LoraValue::LocalTime(_) => 9,
        LoraValue::Duration(_) => 10,
        LoraValue::Point(_) => 11,
        LoraValue::Vector(_) => 12,
        LoraValue::List(_) => 13,
        LoraValue::Map(_) => 14,
        LoraValue::Node(_) => 15,
        LoraValue::Relationship(_) => 16,
        LoraValue::Path(_) => 17,
    }
}

/// Check whether a node's labels satisfy all label groups.
/// Each group is a disjunction (OR): the node must have at least one label
/// from the group.  Groups are conjunctive (AND): all groups must be satisfied.
pub(crate) fn node_matches_label_groups(node_labels: &[String], groups: &[Vec<String>]) -> bool {
    groups
        .iter()
        .all(|group| group.iter().any(|l| node_labels.iter().any(|nl| nl == l)))
}

/// Scan the graph for candidate node IDs matching the label groups. Uses the
/// label index for the pick-first-label phase and avoids cloning NodeRecords.
pub(crate) fn scan_node_ids_for_label_groups<S: GraphStorage>(
    storage: &S,
    groups: &[Vec<String>],
) -> Vec<NodeId> {
    if groups.is_empty() {
        return storage.all_node_ids();
    }
    if groups.len() == 1 {
        return label_group_candidate_ids(storage, &groups[0]);
    }

    let mut best: Option<Vec<NodeId>> = None;
    for group in groups {
        let ids = label_group_candidate_ids(storage, group);
        if ids.is_empty() {
            return Vec::new();
        }
        if best
            .as_ref()
            .map(|current| ids.len() < current.len())
            .unwrap_or(true)
        {
            best = Some(ids);
        }
    }

    best.unwrap_or_default()
}

pub(crate) fn label_group_candidates_prefiltered(groups: &[Vec<String>]) -> bool {
    groups.len() <= 1
}

fn label_group_candidate_ids<S: GraphStorage>(storage: &S, group: &[String]) -> Vec<NodeId> {
    match group {
        [] => Vec::new(),
        [label] => storage.node_ids_by_label(label),
        labels => {
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            for label in labels {
                for id in storage.node_ids_by_label(label) {
                    if seen.insert(id) {
                        out.push(id);
                    }
                }
            }
            out
        }
    }
}

pub(crate) fn hydrate_node_record(node: &lora_store::NodeRecord) -> LoraValue {
    let mut map = BTreeMap::new();
    map.insert("kind".to_string(), LoraValue::String("node".to_string()));
    map.insert("id".to_string(), LoraValue::Int(node.id as i64));
    map.insert(
        "labels".to_string(),
        LoraValue::List(
            node.labels
                .iter()
                .map(|s| LoraValue::String(s.clone()))
                .collect(),
        ),
    );
    map.insert(
        "properties".to_string(),
        properties_to_value_map(&node.properties),
    );
    LoraValue::Map(map)
}

pub(crate) fn hydrate_relationship_record(rel: &lora_store::RelationshipRecord) -> LoraValue {
    let mut map = BTreeMap::new();
    map.insert(
        "kind".to_string(),
        LoraValue::String("relationship".to_string()),
    );
    map.insert("id".to_string(), LoraValue::Int(rel.id as i64));
    map.insert("startId".to_string(), LoraValue::Int(rel.src as i64));
    map.insert("endId".to_string(), LoraValue::Int(rel.dst as i64));
    map.insert("type".to_string(), LoraValue::String(rel.rel_type.clone()));
    map.insert(
        "properties".to_string(),
        properties_to_value_map(&rel.properties),
    );
    LoraValue::Map(map)
}

/// Flatten label groups into a simple Vec<String> (for CREATE/MERGE where
/// disjunction doesn't apply — all labels are created).
pub(super) fn flatten_label_groups(groups: &[Vec<String>]) -> Vec<String> {
    groups.iter().flat_map(|g| g.iter().cloned()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GroupValueKey {
    Null,
    Bool(bool),
    Int(i64),
    Float(String),
    String(String),
    Binary(Vec<Vec<u8>>),
    List(Vec<GroupValueKey>),
    Map(Vec<(String, GroupValueKey)>),
    Node(u64),
    Relationship(u64),
}

impl GroupValueKey {
    pub(crate) fn from_value(v: &LoraValue) -> Self {
        match v {
            LoraValue::Null => Self::Null,
            LoraValue::Bool(x) => Self::Bool(*x),
            LoraValue::Int(x) => Self::Int(*x),
            LoraValue::Float(x) => Self::Float(x.to_string()),
            LoraValue::String(x) => Self::String(x.clone()),
            LoraValue::Binary(x) => Self::Binary(x.segments().to_vec()),
            LoraValue::List(xs) => Self::List(xs.iter().map(Self::from_value).collect()),
            LoraValue::Map(m) => Self::Map(
                m.iter()
                    .map(|(k, v)| (k.clone(), Self::from_value(v)))
                    .collect(),
            ),
            LoraValue::Node(id) => Self::Node(*id),
            LoraValue::Relationship(id) => Self::Relationship(*id),
            LoraValue::Path(_) => Self::Null,
            // Temporal types: use their string representation as group key
            LoraValue::Date(d) => Self::String(d.to_string()),
            LoraValue::DateTime(dt) => Self::String(dt.to_string()),
            LoraValue::LocalDateTime(dt) => Self::String(dt.to_string()),
            LoraValue::Time(t) => Self::String(t.to_string()),
            LoraValue::LocalTime(t) => Self::String(t.to_string()),
            LoraValue::Duration(dur) => Self::String(dur.to_string()),
            LoraValue::Point(p) => Self::String(p.to_string()),
            LoraValue::Vector(v) => Self::String(format!("vector:{}", v.to_key_string())),
        }
    }
}

/// Compute effective (min_hops, max_hops) from a `RangeLiteral`.
///
/// Lora semantics:
/// - `*`       → 1..∞   (start=None, end=None)
/// - `*2..5`   → 2..5   (start=Some(2), end=Some(5))
/// - `*..3`    → 1..3   (start=None, end=Some(3))
/// - `*2..`    → 2..∞   (start=Some(2), end=None)
/// - `*3`      → 3..3   (start=Some(3), end=None, no dots → exactly 3)
/// - `*0..1`   → 0..1
///
/// For unbounded upper, we cap at `MAX_VAR_LEN_HOPS` to prevent runaway.
const MAX_VAR_LEN_HOPS: u64 = 100;

pub(crate) fn resolve_range(range: &RangeLiteral) -> (u64, u64) {
    let min_hops = range.start.unwrap_or(1);
    let max_hops = range.end.unwrap_or(MAX_VAR_LEN_HOPS);
    (min_hops, max_hops)
}

/// An entry produced during BFS variable-length expansion.
pub(crate) struct VarLenResult {
    /// The destination node at the end of this path.
    pub(crate) dst_node_id: NodeId,
    /// The relationship IDs traversed (in order).
    pub(crate) rel_ids: Vec<u64>,
}

/// Perform variable-length expansion from `start_node_id` following
/// relationships of the given `types` and `direction`, collecting all
/// reachable nodes at hop distances in `[min_hops, max_hops]`.
///
/// Uses BFS with relationship-uniqueness per path (each path does not
/// reuse the same relationship, but may revisit nodes).
pub(crate) fn variable_length_expand<S: GraphStorage>(
    storage: &S,
    start_node_id: NodeId,
    direction: Direction,
    types: &[String],
    min_hops: u64,
    max_hops: u64,
    bind_relationships: bool,
) -> Vec<VarLenResult> {
    let mut results = Vec::new();

    // Each frontier entry: (current_node_id, relationships_used_so_far)
    let mut frontier: Vec<(NodeId, Vec<u64>)> = vec![(start_node_id, Vec::new())];

    for depth in 1..=max_hops {
        // On the final hop we don't need to build next_frontier at all; every
        // path gets recorded and then the loop terminates. Avoids one full
        // pass of Vec clones on deep traversals.
        let is_last_hop = depth == max_hops;
        let mut next_frontier: Vec<(NodeId, Vec<u64>)> = Vec::new();

        for (current_node, rels_used) in &frontier {
            // ID-only expand avoids cloning full records/properties for every
            // neighbour on every hop.
            for (rel_id, neighbor_id) in storage.expand_ids(*current_node, direction, types) {
                // Relationship-uniqueness: skip if this relationship was already
                // traversed on this particular path.
                if rels_used.contains(&rel_id) {
                    continue;
                }

                if is_last_hop {
                    // Terminal hop: just record the result. Allocate rel_ids
                    // once (no duplicate clone) by extending a fresh copy.
                    if depth >= min_hops {
                        let mut rel_ids = Vec::with_capacity(rels_used.len() + 1);
                        rel_ids.extend_from_slice(rels_used);
                        rel_ids.push(rel_id);
                        results.push(VarLenResult {
                            dst_node_id: neighbor_id,
                            rel_ids: if bind_relationships {
                                rel_ids
                            } else {
                                Vec::new()
                            },
                        });
                    }
                    continue;
                }

                let mut new_rels = Vec::with_capacity(rels_used.len() + 1);
                new_rels.extend_from_slice(rels_used);
                new_rels.push(rel_id);

                if depth >= min_hops {
                    results.push(VarLenResult {
                        dst_node_id: neighbor_id,
                        rel_ids: if bind_relationships {
                            new_rels.clone()
                        } else {
                            Vec::new()
                        },
                    });
                }

                next_frontier.push((neighbor_id, new_rels));
            }
        }

        if is_last_hop || next_frontier.is_empty() {
            break;
        }

        frontier = next_frontier;
    }

    // Handle min_hops == 0: include the start node itself at depth 0.
    if min_hops == 0 {
        results.insert(
            0,
            VarLenResult {
                dst_node_id: start_node_id,
                rel_ids: Vec::new(),
            },
        );
    }

    results
}

/// Filter rows to keep only shortest paths.
/// `all` = false → keep one shortest path; `all` = true → keep all shortest.
pub(crate) fn filter_shortest_paths(rows: Vec<Row>, path_var: VarId, all: bool) -> Vec<Row> {
    if rows.is_empty() {
        return rows;
    }

    // Compute path length for each row
    let lengths: Vec<usize> = rows
        .iter()
        .map(|row| match row.get(path_var) {
            Some(LoraValue::Path(p)) => p.rels.len(),
            _ => usize::MAX,
        })
        .collect();

    let min_len = lengths.iter().copied().min().unwrap_or(usize::MAX);

    let mut result: Vec<Row> = rows
        .into_iter()
        .zip(lengths.iter())
        .filter(|(_, len)| **len == min_len)
        .map(|(row, _)| row)
        .collect();

    if !all && result.len() > 1 {
        result.truncate(1);
    }

    result
}
