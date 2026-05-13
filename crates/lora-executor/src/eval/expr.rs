//! Expression dispatcher: `eval_expr` plus its support code.
//!
//! Hosts:
//! - The [`EvalContext`] borrowed by every operator source.
//! - [`eval_expr`] — the recursive walker over [`ResolvedExpr`].
//! - [`eval_expr_result`] / [`eval_truthy_result`] — the
//!   result-form wrappers that reify the thread-local error slot
//!   into a `Result`.
//! - Literal and property-access helpers ([`eval_literal`],
//!   [`eval_property`]).
//! - The pattern matchers that back EXISTS subqueries and
//!   pattern comprehensions.

#[allow(unused_imports)]
use crate::value::LoraPath;
use crate::value::{LoraValue, Row};
use lora_analyzer::{LiteralValue, ResolvedExpr, ResolvedMapSelector};
use lora_ast::ListPredicateKind;
use lora_store::GraphStorage;
use std::collections::BTreeMap;

use super::binops::{eval_binary, eval_unary, value_eq};
use super::errors::{clear_eval_error, take_eval_error};
use super::functions::eval_function;

pub struct EvalContext<'a, S: GraphStorage> {
    pub storage: &'a S,
    pub params: &'a BTreeMap<String, LoraValue>,
}

pub fn eval_expr<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    match expr {
        ResolvedExpr::Variable(var_id) => row.get(*var_id).cloned().unwrap_or(LoraValue::Null),

        ResolvedExpr::Literal(lit) => eval_literal(lit),

        ResolvedExpr::List(items) => {
            LoraValue::List(items.iter().map(|e| eval_expr(e, row, ctx)).collect())
        }

        ResolvedExpr::Map(items) => {
            let mut map = BTreeMap::new();
            for (k, v) in items {
                map.insert(k.clone(), eval_expr(v, row, ctx));
            }
            LoraValue::Map(map)
        }

        ResolvedExpr::Property { expr, property } => {
            let base = eval_expr(expr, row, ctx);
            eval_property(&base, property, ctx)
        }

        ResolvedExpr::Binary { lhs, op, rhs } => {
            let l = eval_expr(lhs, row, ctx);
            let r = eval_expr(rhs, row, ctx);
            eval_binary(op, l, r)
        }

        ResolvedExpr::Unary { op, expr } => {
            let v = eval_expr(expr, row, ctx);
            eval_unary(*op, v)
        }

        ResolvedExpr::Function {
            function,
            distinct: _,
            args,
        } => {
            let args: Vec<LoraValue> = args.iter().map(|a| eval_expr(a, row, ctx)).collect();
            eval_function(*function, &args, ctx)
        }

        ResolvedExpr::Parameter(name) => ctx.params.get(name).cloned().unwrap_or(LoraValue::Null),

        ResolvedExpr::ListPredicate {
            kind,
            variable,
            list,
            predicate,
        } => {
            let list_val = eval_expr(list, row, ctx);
            match list_val {
                LoraValue::List(items) => {
                    let total = items.len();
                    // Clone the row once, then rebind `variable` per iteration
                    // instead of cloning the entire row + item per element.
                    let mut inner_row = row.clone();
                    let mut count = 0usize;
                    for item in items {
                        inner_row.insert(*variable, item);
                        if eval_expr(predicate, &inner_row, ctx).is_truthy() {
                            count += 1;
                        }
                    }
                    match kind {
                        ListPredicateKind::Any => LoraValue::Bool(count > 0),
                        ListPredicateKind::All => LoraValue::Bool(count == total),
                        ListPredicateKind::None => LoraValue::Bool(count == 0),
                        ListPredicateKind::Single => LoraValue::Bool(count == 1),
                    }
                }
                LoraValue::Null => LoraValue::Null,
                _ => LoraValue::Bool(false),
            }
        }

        ResolvedExpr::ListComprehension {
            variable,
            list,
            filter,
            map_expr,
        } => {
            let list_val = eval_expr(list, row, ctx);
            match list_val {
                LoraValue::List(items) => {
                    let mut result = Vec::with_capacity(items.len());
                    let mut inner_row = row.clone();
                    for item in items {
                        // When no map_expr, we need the item after filter —
                        // stash it in the row binding and read it back if kept.
                        inner_row.insert(*variable, item);
                        if let Some(f) = filter {
                            if !eval_expr(f, &inner_row, ctx).is_truthy() {
                                continue;
                            }
                        }
                        let val = if let Some(m) = map_expr {
                            eval_expr(m, &inner_row, ctx)
                        } else {
                            // Clone the binding; we still need inner_row for
                            // the next iteration.
                            inner_row.get(*variable).cloned().unwrap_or(LoraValue::Null)
                        };
                        result.push(val);
                    }
                    LoraValue::List(result)
                }
                LoraValue::Null => LoraValue::Null,
                _ => LoraValue::Null,
            }
        }

        ResolvedExpr::Reduce {
            accumulator,
            init,
            variable,
            list,
            expr,
        } => {
            let init_val = eval_expr(init, row, ctx);
            let list_val = eval_expr(list, row, ctx);
            match list_val {
                LoraValue::List(items) => {
                    // One cloned row reused across all iterations; accumulator
                    // and variable rebind per item.
                    let mut inner_row = row.clone();
                    let mut acc = init_val;
                    for item in items {
                        inner_row.insert(*accumulator, acc);
                        inner_row.insert(*variable, item);
                        acc = eval_expr(expr, &inner_row, ctx);
                    }
                    acc
                }
                LoraValue::Null => LoraValue::Null,
                _ => LoraValue::Null,
            }
        }

        ResolvedExpr::Index { expr, index } => {
            let base = eval_expr(expr, row, ctx);
            let idx = eval_expr(index, row, ctx);
            match (base, idx) {
                (LoraValue::List(items), LoraValue::Int(i)) => {
                    let i = if i < 0 {
                        match i64::try_from(items.len())
                            .ok()
                            .and_then(|len| len.checked_add(i))
                        {
                            Some(i) if i >= 0 => i as usize,
                            _ => return LoraValue::Null,
                        }
                    } else {
                        i as usize
                    };
                    items.get(i).cloned().unwrap_or(LoraValue::Null)
                }
                (LoraValue::Map(m), LoraValue::String(key)) => {
                    m.get(&key).cloned().unwrap_or(LoraValue::Null)
                }
                _ => LoraValue::Null,
            }
        }

        ResolvedExpr::Slice { expr, from, to } => {
            let base = eval_expr(expr, row, ctx);
            match base {
                LoraValue::List(items) => {
                    let len = items.len() as i64;
                    let start = from
                        .as_ref()
                        .map(|e| eval_expr(e, row, ctx).as_i64().unwrap_or(0))
                        .unwrap_or(0)
                        .max(0)
                        .min(len) as usize;
                    let end = to
                        .as_ref()
                        .map(|e| eval_expr(e, row, ctx).as_i64().unwrap_or(len))
                        .unwrap_or(len)
                        .max(0)
                        .min(len) as usize;
                    if start >= end {
                        LoraValue::List(Vec::new())
                    } else {
                        LoraValue::List(items[start..end].to_vec())
                    }
                }
                _ => LoraValue::Null,
            }
        }

        ResolvedExpr::MapProjection { base, selectors } => {
            let base_val = eval_expr(base, row, ctx);
            let mut result = BTreeMap::new();

            for sel in selectors {
                match sel {
                    ResolvedMapSelector::Property(key) => {
                        let val = eval_property(&base_val, key, ctx);
                        result.insert(key.clone(), val);
                    }
                    ResolvedMapSelector::AllProperties => {
                        // Borrow the stored record (when the backend supports it)
                        // via `with_node` / `with_relationship`; otherwise the
                        // closure still runs against an owned fetch.
                        match &base_val {
                            LoraValue::Node(id) => {
                                ctx.storage.with_node(*id, |node| {
                                    for (k, v) in &node.properties {
                                        result.insert(k.clone(), LoraValue::from(v));
                                    }
                                });
                            }
                            LoraValue::Relationship(id) => {
                                ctx.storage.with_relationship(*id, |rel| {
                                    for (k, v) in &rel.properties {
                                        result.insert(k.clone(), LoraValue::from(v));
                                    }
                                });
                            }
                            LoraValue::Map(m) => {
                                for (k, v) in m {
                                    result.insert(k.clone(), v.clone());
                                }
                            }
                            _ => {}
                        }
                    }
                    ResolvedMapSelector::Literal(key, expr) => {
                        let val = eval_expr(expr, row, ctx);
                        result.insert(key.clone(), val);
                    }
                }
            }

            LoraValue::Map(result)
        }

        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            if let Some(input) = input {
                let input_val = eval_expr(input, row, ctx);

                for (when, then) in alternatives {
                    let when_val = eval_expr(when, row, ctx);
                    if value_eq(&input_val, &when_val) {
                        return eval_expr(then, row, ctx);
                    }
                }

                else_expr
                    .as_ref()
                    .map(|e| eval_expr(e, row, ctx))
                    .unwrap_or(LoraValue::Null)
            } else {
                for (when, then) in alternatives {
                    let when_val = eval_expr(when, row, ctx);
                    if when_val.is_truthy() {
                        return eval_expr(then, row, ctx);
                    }
                }

                else_expr
                    .as_ref()
                    .map(|e| eval_expr(e, row, ctx))
                    .unwrap_or(LoraValue::Null)
            }
        }

        ResolvedExpr::ExistsSubquery { pattern, where_ } => {
            eval_exists_subquery(pattern, where_.as_deref(), row, ctx)
        }

        ResolvedExpr::PatternComprehension {
            pattern,
            where_,
            map_expr,
        } => eval_pattern_comprehension(pattern, where_.as_deref(), map_expr, row, ctx),
    }
}

fn eval_exists_subquery<S: GraphStorage>(
    pattern: &lora_analyzer::ResolvedPattern,
    where_: Option<&ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    use lora_analyzer::ResolvedPatternElement;

    if pattern.parts.is_empty() {
        return LoraValue::Bool(exists_candidate_matches(where_, row, ctx));
    }

    // Process each pattern part. The final part short-circuits because EXISTS
    // only cares whether at least one complete binding survives the WHERE.
    let mut candidate_rows = vec![row.clone()];

    for (part_idx, part) in pattern.parts.iter().enumerate() {
        let is_last_part = part_idx + 1 == pattern.parts.len();
        let mut next_rows = Vec::new();
        for current_row in &candidate_rows {
            match &part.element {
                ResolvedPatternElement::Node {
                    var,
                    labels,
                    properties,
                } => {
                    let tmp_node = lora_analyzer::ResolvedNode {
                        var: *var,
                        labels: labels.clone(),
                        properties: properties.clone(),
                    };
                    let matched_rows = match_node_pattern(&tmp_node, current_row, ctx);
                    if is_last_part {
                        if matched_rows
                            .iter()
                            .any(|r| exists_candidate_matches(where_, r, ctx))
                        {
                            return LoraValue::Bool(true);
                        }
                    } else {
                        next_rows.extend(matched_rows);
                    }
                }
                ResolvedPatternElement::ShortestPath { head, chain, .. }
                | ResolvedPatternElement::NodeChain { head, chain } => {
                    let head_rows = match_node_pattern(head, current_row, ctx);
                    for hr in head_rows {
                        let mut frontier = vec![hr];
                        for step in chain {
                            let mut step_rows = Vec::new();
                            for fr in &frontier {
                                let src_node_id = find_last_node_in_row(fr, head.var, chain, step);
                                if let Some(sid) = src_node_id {
                                    let _ = ctx.storage.try_for_each_expand_id(
                                        sid,
                                        step.rel.direction,
                                        &step.rel.types,
                                        |rel_id, dst_id| {
                                            let matched = ctx
                                                .storage
                                                .with_node(dst_id, |dst| {
                                                    node_matches_labels(
                                                        &dst.labels,
                                                        &step.node.labels,
                                                    ) && node_matches_properties(
                                                        &dst.properties,
                                                        &step.node.properties,
                                                        fr,
                                                        ctx,
                                                    )
                                                })
                                                .unwrap_or(false);
                                            if !matched {
                                                return Ok::<(), ()>(());
                                            }
                                            let mut r = fr.clone();
                                            if let Some(rv) = step.rel.var {
                                                r.insert(rv, LoraValue::Relationship(rel_id));
                                            }
                                            if let Some(nv) = step.node.var {
                                                r.insert(nv, LoraValue::Node(dst_id));
                                            }
                                            step_rows.push(r);
                                            Ok(())
                                        },
                                    );
                                }
                            }
                            frontier = step_rows;
                        }
                        if is_last_part {
                            if frontier
                                .iter()
                                .any(|r| exists_candidate_matches(where_, r, ctx))
                            {
                                return LoraValue::Bool(true);
                            }
                        } else {
                            next_rows.extend(frontier);
                        }
                    }
                }
            }
        }
        if next_rows.is_empty() {
            return LoraValue::Bool(false);
        }
        candidate_rows = next_rows;
    }

    LoraValue::Bool(false)
}

fn exists_candidate_matches<S: GraphStorage>(
    where_: Option<&ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> bool {
    where_
        .map(|where_expr| eval_expr(where_expr, row, ctx).is_truthy())
        .unwrap_or(true)
}

fn eval_pattern_comprehension<S: GraphStorage>(
    pattern: &lora_analyzer::ResolvedPattern,
    where_: Option<&ResolvedExpr>,
    map_expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    // Reuse the same pattern matching as EXISTS
    let mut candidate_rows = vec![row.clone()];

    for part in &pattern.parts {
        let mut next_rows = Vec::new();
        for current_row in &candidate_rows {
            match &part.element {
                lora_analyzer::ResolvedPatternElement::Node {
                    var,
                    labels,
                    properties,
                } => {
                    let tmp_node = lora_analyzer::ResolvedNode {
                        var: *var,
                        labels: labels.clone(),
                        properties: properties.clone(),
                    };
                    next_rows.extend(match_node_pattern(&tmp_node, current_row, ctx));
                }
                lora_analyzer::ResolvedPatternElement::ShortestPath { head, chain, .. }
                | lora_analyzer::ResolvedPatternElement::NodeChain { head, chain } => {
                    let head_rows = match_node_pattern(head, current_row, ctx);
                    for hr in head_rows {
                        let mut frontier = vec![hr];
                        for step in chain {
                            let mut step_rows = Vec::new();
                            for fr in &frontier {
                                let src_node_id = find_last_node_in_row(fr, head.var, chain, step);
                                if let Some(sid) = src_node_id {
                                    let _ = ctx.storage.try_for_each_expand_id(
                                        sid,
                                        step.rel.direction,
                                        &step.rel.types,
                                        |rel_id, dst_id| {
                                            let matched = ctx
                                                .storage
                                                .with_node(dst_id, |dst| {
                                                    node_matches_labels(
                                                        &dst.labels,
                                                        &step.node.labels,
                                                    ) && node_matches_properties(
                                                        &dst.properties,
                                                        &step.node.properties,
                                                        fr,
                                                        ctx,
                                                    )
                                                })
                                                .unwrap_or(false);
                                            if !matched {
                                                return Ok::<(), ()>(());
                                            }
                                            let mut r = fr.clone();
                                            if let Some(rv) = step.rel.var {
                                                r.insert(rv, LoraValue::Relationship(rel_id));
                                            }
                                            if let Some(nv) = step.node.var {
                                                r.insert(nv, LoraValue::Node(dst_id));
                                            }
                                            step_rows.push(r);
                                            Ok(())
                                        },
                                    );
                                }
                            }
                            frontier = step_rows;
                        }
                        next_rows.extend(frontier);
                    }
                }
            }
        }
        candidate_rows = next_rows;
    }

    if let Some(where_expr) = where_ {
        candidate_rows.retain(|r| eval_expr(where_expr, r, ctx).is_truthy());
    }

    // Map each matched row through the map expression
    LoraValue::List(
        candidate_rows
            .iter()
            .map(|r| eval_expr(map_expr, r, ctx))
            .collect(),
    )
}

fn match_node_pattern<S: GraphStorage>(
    node: &lora_analyzer::ResolvedNode,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Vec<Row> {
    // If the variable is already bound in the row, check the existing binding
    // without cloning the whole record.
    if let Some(var) = node.var {
        if let Some(LoraValue::Node(id)) = row.get(var) {
            let matched = ctx
                .storage
                .with_node(*id, |n| {
                    node_matches_labels(&n.labels, &node.labels)
                        && node_matches_properties(&n.properties, &node.properties, row, ctx)
                })
                .unwrap_or(false);
            if matched {
                return vec![row.clone()];
            }
            return Vec::new();
        }
    }

    // Candidate discovery only needs IDs — defer record lookup until after
    // label/property filtering so we can borrow once per matching candidate.
    let first_label = node.labels.iter().flat_map(|g| g.iter()).next();
    let candidate_ids: Vec<lora_store::NodeId> = match first_label {
        Some(label) => ctx.storage.node_ids_by_label(label),
        None => ctx.storage.all_node_ids(),
    };

    let mut out = Vec::new();
    for id in candidate_ids {
        let matched = ctx
            .storage
            .with_node(id, |n| {
                node_matches_labels(&n.labels, &node.labels)
                    && node_matches_properties(&n.properties, &node.properties, row, ctx)
            })
            .unwrap_or(false);
        if !matched {
            continue;
        }
        let mut r = row.clone();
        if let Some(v) = node.var {
            r.insert(v, LoraValue::Node(id));
        }
        out.push(r);
    }
    out
}

/// Find the node ID of the current source node for a chain step.
fn find_last_node_in_row(
    row: &Row,
    head_var: Option<lora_analyzer::symbols::VarId>,
    chain: &[lora_analyzer::ResolvedChain],
    current_step: &lora_analyzer::ResolvedChain,
) -> Option<u64> {
    // Walk through the chain to find the previous node's variable
    let mut prev_var = head_var;
    for step in chain {
        if std::ptr::eq(step, current_step) {
            break;
        }
        prev_var = step.node.var;
    }
    prev_var.and_then(|v| match row.get(v) {
        Some(LoraValue::Node(id)) => Some(*id),
        _ => None,
    })
}

fn node_matches_labels(node_labels: &[String], groups: &[Vec<String>]) -> bool {
    groups
        .iter()
        .all(|group| group.iter().any(|l| node_labels.iter().any(|nl| nl == l)))
}

fn node_matches_properties<S: GraphStorage>(
    props: &BTreeMap<String, lora_store::PropertyValue>,
    expected: &Option<ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> bool {
    let Some(props_expr) = expected else {
        return true;
    };
    let expected = eval_expr(props_expr, row, ctx);
    if let LoraValue::Map(exp) = expected {
        exp.iter().all(|(k, v)| {
            props
                .get(k)
                .map(|pv| crate::executor::value_matches_property_value(v, pv))
                .unwrap_or(false)
        })
    } else {
        true
    }
}

pub fn eval_expr_result<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Result<LoraValue, String> {
    clear_eval_error();
    let value = eval_expr(expr, row, ctx);
    match take_eval_error() {
        Some(err) => Err(err),
        None => Ok(value),
    }
}

pub fn eval_truthy_result<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Result<bool, String> {
    Ok(eval_expr_result(expr, row, ctx)?.is_truthy())
}

fn eval_literal(lit: &LiteralValue) -> LoraValue {
    match lit {
        LiteralValue::Integer(v) => LoraValue::Int(*v),
        LiteralValue::Float(v) => LoraValue::Float(*v),
        LiteralValue::String(v) => LoraValue::String(v.clone()),
        LiteralValue::TypeName(v) => LoraValue::String(v.clone()),
        LiteralValue::Bool(v) => LoraValue::Bool(*v),
        LiteralValue::Null => LoraValue::Null,
    }
}

fn eval_property<S: GraphStorage>(
    base: &LoraValue,
    key: &str,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    match base {
        LoraValue::Map(map) => {
            // Direct key lookup first.
            if let Some(v) = map.get(key) {
                return v.clone();
            }
            // For hydrated entity maps (have a "properties" sub-map), look inside.
            if let Some(LoraValue::Map(props)) = map.get("properties") {
                if let Some(v) = props.get(key) {
                    return v.clone();
                }
            }
            LoraValue::Null
        }

        LoraValue::Node(id) => ctx
            .storage
            .with_node(*id, |node| {
                node.properties
                    .get(key)
                    .map(LoraValue::from)
                    .unwrap_or(LoraValue::Null)
            })
            .unwrap_or(LoraValue::Null),

        LoraValue::Relationship(id) => ctx
            .storage
            .with_relationship(*id, |rel| {
                rel.properties
                    .get(key)
                    .map(LoraValue::from)
                    .unwrap_or(LoraValue::Null)
            })
            .unwrap_or(LoraValue::Null),

        LoraValue::Date(d) => match key {
            "year" => LoraValue::Int(d.year as i64),
            "month" => LoraValue::Int(d.month as i64),
            "day" => LoraValue::Int(d.day as i64),
            "dayOfWeek" => LoraValue::Int(d.day_of_week() as i64),
            "dayOfYear" => LoraValue::Int(d.day_of_year() as i64),
            _ => LoraValue::Null,
        },

        LoraValue::DateTime(dt) => match key {
            "year" => LoraValue::Int(dt.year as i64),
            "month" => LoraValue::Int(dt.month as i64),
            "day" => LoraValue::Int(dt.day as i64),
            "hour" => LoraValue::Int(dt.hour as i64),
            "minute" => LoraValue::Int(dt.minute as i64),
            "second" => LoraValue::Int(dt.second as i64),
            "millisecond" => LoraValue::Int((dt.nanosecond / 1_000_000) as i64),
            "dayOfWeek" => LoraValue::Int(dt.date().day_of_week() as i64),
            "dayOfYear" => LoraValue::Int(dt.date().day_of_year() as i64),
            _ => LoraValue::Null,
        },

        LoraValue::LocalDateTime(dt) => match key {
            "year" => LoraValue::Int(dt.year as i64),
            "month" => LoraValue::Int(dt.month as i64),
            "day" => LoraValue::Int(dt.day as i64),
            "hour" => LoraValue::Int(dt.hour as i64),
            "minute" => LoraValue::Int(dt.minute as i64),
            "second" => LoraValue::Int(dt.second as i64),
            "millisecond" => LoraValue::Int((dt.nanosecond / 1_000_000) as i64),
            _ => LoraValue::Null,
        },

        LoraValue::Time(t) => match key {
            "hour" => LoraValue::Int(t.hour as i64),
            "minute" => LoraValue::Int(t.minute as i64),
            "second" => LoraValue::Int(t.second as i64),
            "millisecond" => LoraValue::Int((t.nanosecond / 1_000_000) as i64),
            _ => LoraValue::Null,
        },

        LoraValue::LocalTime(t) => match key {
            "hour" => LoraValue::Int(t.hour as i64),
            "minute" => LoraValue::Int(t.minute as i64),
            "second" => LoraValue::Int(t.second as i64),
            "millisecond" => LoraValue::Int((t.nanosecond / 1_000_000) as i64),
            _ => LoraValue::Null,
        },

        LoraValue::Duration(dur) => match key {
            "years" => LoraValue::Int(dur.years_component()),
            "months" => LoraValue::Int(dur.months_component()),
            "days" => LoraValue::Int(dur.days_component()),
            "hours" => LoraValue::Int(dur.hours_component()),
            "minutes" => LoraValue::Int(dur.minutes_component()),
            "seconds" => LoraValue::Int(dur.seconds_component()),
            _ => LoraValue::Null,
        },

        LoraValue::Point(p) => match key {
            "x" => LoraValue::Float(p.x),
            "y" => LoraValue::Float(p.y),
            "z" => p.z.map(LoraValue::Float).unwrap_or(LoraValue::Null),
            "latitude" => {
                if p.is_geographic() {
                    LoraValue::Float(p.latitude())
                } else {
                    LoraValue::Null
                }
            }
            "longitude" => {
                if p.is_geographic() {
                    LoraValue::Float(p.longitude())
                } else {
                    LoraValue::Null
                }
            }
            "height" => p.height().map(LoraValue::Float).unwrap_or(LoraValue::Null),
            "srid" => LoraValue::Int(p.srid as i64),
            "crs" => LoraValue::String(p.crs_name().to_string()),
            _ => LoraValue::Null,
        },

        _ => LoraValue::Null,
    }
}
