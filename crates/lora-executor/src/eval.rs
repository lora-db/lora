use crate::value::{Row, LoraValue};
#[allow(unused_imports)]
use crate::value::LoraPath;
use lora_analyzer::{LiteralValue, ResolvedExpr, ResolvedMapSelector};
use lora_ast::{BinaryOp, ListPredicateKind, UnaryOp};
use lora_store::{
    GraphStorage, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime,
    LoraPoint, LoraTime, PointKeyFamily, point_distance, resolve_srid, srid_is_3d,
};
use std::collections::BTreeMap;

pub struct EvalContext<'a, S: GraphStorage + ?Sized> {
    pub storage: &'a S,
    pub params: &'a BTreeMap<String, LoraValue>,
}

pub fn eval_expr<S: GraphStorage + ?Sized>(
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
            let mut map = std::collections::BTreeMap::new();
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
            name,
            distinct: _,
            args,
        } => {
            let args: Vec<LoraValue> = args.iter().map(|a| eval_expr(a, row, ctx)).collect();
            eval_function(name, &args, ctx)
        }

        ResolvedExpr::Parameter(name) => {
            ctx.params.get(name).cloned().unwrap_or(LoraValue::Null)
        }

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
                            inner_row
                                .get(*variable)
                                .cloned()
                                .unwrap_or(LoraValue::Null)
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
                    let i = if i < 0 { (items.len() as i64 + i) as usize } else { i as usize };
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
                    let start = from.as_ref()
                        .map(|e| eval_expr(e, row, ctx).as_i64().unwrap_or(0))
                        .unwrap_or(0)
                        .max(0)
                        .min(len) as usize;
                    let end = to.as_ref()
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
            let mut result = std::collections::BTreeMap::new();

            for sel in selectors {
                match sel {
                    ResolvedMapSelector::Property(key) => {
                        let val = eval_property(&base_val, key, ctx);
                        result.insert(key.clone(), val);
                    }
                    ResolvedMapSelector::AllProperties => {
                        // Borrow the stored record and convert properties in a
                        // single walk (no intermediate PropertyValue clone).
                        match &base_val {
                            LoraValue::Node(id) => {
                                if let Some(node) = ctx.storage.node_ref(*id) {
                                    for (k, v) in &node.properties {
                                        result.insert(k.clone(), LoraValue::from(v));
                                    }
                                }
                            }
                            LoraValue::Relationship(id) => {
                                if let Some(rel) = ctx.storage.relationship_ref(*id) {
                                    for (k, v) in &rel.properties {
                                        result.insert(k.clone(), LoraValue::from(v));
                                    }
                                }
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
        } => {
            eval_pattern_comprehension(pattern, where_.as_deref(), map_expr, row, ctx)
        }
    }
}

fn eval_exists_subquery<S: GraphStorage + ?Sized>(
    pattern: &lora_analyzer::ResolvedPattern,
    where_: Option<&ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    use lora_analyzer::ResolvedPatternElement;

    // Process each pattern part. For EXISTS, any part producing a match means true.
    let mut candidate_rows = vec![row.clone()];

    for part in &pattern.parts {
        let mut next_rows = Vec::new();
        for current_row in &candidate_rows {
            match &part.element {
                ResolvedPatternElement::Node { var, labels, properties } => {
                    let tmp_node = lora_analyzer::ResolvedNode {
                        var: *var,
                        labels: labels.clone(),
                        properties: properties.clone(),
                    };
                    next_rows.extend(match_node_pattern(&tmp_node, current_row, ctx));
                }
                ResolvedPatternElement::ShortestPath { head, chain, .. } |
                ResolvedPatternElement::NodeChain { head, chain } => {
                    let head_rows = match_node_pattern(head, current_row, ctx);
                    for hr in head_rows {
                        let mut frontier = vec![hr];
                        for step in chain {
                            let mut step_rows = Vec::new();
                            for fr in &frontier {
                                let src_node_id = find_last_node_in_row(fr, head.var, chain, step);
                                if let Some(sid) = src_node_id {
                                    for (rel_id, dst_id) in ctx
                                        .storage
                                        .expand_ids(sid, step.rel.direction, &step.rel.types)
                                    {
                                        let Some(dst) = ctx.storage.node_ref(dst_id) else {
                                            continue;
                                        };
                                        if !node_matches_labels(&dst.labels, &step.node.labels) {
                                            continue;
                                        }
                                        if !node_matches_properties(
                                            &dst.properties,
                                            &step.node.properties,
                                            fr,
                                            ctx,
                                        ) {
                                            continue;
                                        }
                                        let mut r = fr.clone();
                                        if let Some(rv) = step.rel.var {
                                            r.insert(rv, LoraValue::Relationship(rel_id));
                                        }
                                        if let Some(nv) = step.node.var {
                                            r.insert(nv, LoraValue::Node(dst_id));
                                        }
                                        step_rows.push(r);
                                    }
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

    // Apply WHERE filter if present
    if let Some(where_expr) = where_ {
        candidate_rows.retain(|r| eval_expr(where_expr, r, ctx).is_truthy());
    }

    LoraValue::Bool(!candidate_rows.is_empty())
}

fn eval_pattern_comprehension<S: GraphStorage + ?Sized>(
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
                lora_analyzer::ResolvedPatternElement::Node { var, labels, properties } => {
                    let tmp_node = lora_analyzer::ResolvedNode {
                        var: *var,
                        labels: labels.clone(),
                        properties: properties.clone(),
                    };
                    next_rows.extend(match_node_pattern(&tmp_node, current_row, ctx));
                }
                lora_analyzer::ResolvedPatternElement::ShortestPath { head, chain, .. } |
                lora_analyzer::ResolvedPatternElement::NodeChain { head, chain } => {
                    let head_rows = match_node_pattern(head, current_row, ctx);
                    for hr in head_rows {
                        let mut frontier = vec![hr];
                        for step in chain {
                            let mut step_rows = Vec::new();
                            for fr in &frontier {
                                let src_node_id = find_last_node_in_row(fr, head.var, chain, step);
                                if let Some(sid) = src_node_id {
                                    for (rel_id, dst_id) in ctx
                                        .storage
                                        .expand_ids(sid, step.rel.direction, &step.rel.types)
                                    {
                                        let Some(dst) = ctx.storage.node_ref(dst_id) else {
                                            continue;
                                        };
                                        if !node_matches_labels(&dst.labels, &step.node.labels) {
                                            continue;
                                        }
                                        if !node_matches_properties(
                                            &dst.properties,
                                            &step.node.properties,
                                            fr,
                                            ctx,
                                        ) {
                                            continue;
                                        }
                                        let mut r = fr.clone();
                                        if let Some(rv) = step.rel.var {
                                            r.insert(rv, LoraValue::Relationship(rel_id));
                                        }
                                        if let Some(nv) = step.node.var {
                                            r.insert(nv, LoraValue::Node(dst_id));
                                        }
                                        step_rows.push(r);
                                    }
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

fn match_node_pattern<S: GraphStorage + ?Sized>(
    node: &lora_analyzer::ResolvedNode,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Vec<Row> {
    // If the variable is already bound in the row, use that binding — borrow
    // instead of cloning the whole record.
    if let Some(var) = node.var {
        if let Some(LoraValue::Node(id)) = row.get(var) {
            if let Some(n) = ctx.storage.node_ref(*id) {
                if node_matches_labels(&n.labels, &node.labels)
                    && node_matches_properties(&n.properties, &node.properties, row, ctx)
                {
                    return vec![row.clone()];
                }
            }
            return Vec::new();
        }
    }

    // Candidate discovery only needs IDs — defer record lookup until after
    // label/property filtering so we can borrow once per matching candidate.
    let first_label = node
        .labels
        .iter()
        .flat_map(|g| g.iter())
        .next();
    let candidate_ids: Vec<lora_store::NodeId> = match first_label {
        Some(label) => ctx.storage.node_ids_by_label(label),
        None => ctx.storage.all_node_ids(),
    };

    let mut out = Vec::new();
    for id in candidate_ids {
        let Some(n) = ctx.storage.node_ref(id) else {
            continue;
        };
        if !node_matches_labels(&n.labels, &node.labels) {
            continue;
        }
        if !node_matches_properties(&n.properties, &node.properties, row, ctx) {
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
    groups.iter().all(|group| {
        group.iter().any(|l| node_labels.iter().any(|nl| nl == l))
    })
}

fn node_matches_properties<S: GraphStorage + ?Sized>(
    props: &std::collections::BTreeMap<String, lora_store::PropertyValue>,
    expected: &Option<ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> bool {
    let Some(props_expr) = expected else { return true; };
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

fn eval_literal(lit: &LiteralValue) -> LoraValue {
    match lit {
        LiteralValue::Integer(v) => LoraValue::Int(*v),
        LiteralValue::Float(v) => LoraValue::Float(*v),
        LiteralValue::String(v) => LoraValue::String(v.clone()),
        LiteralValue::Bool(v) => LoraValue::Bool(*v),
        LiteralValue::Null => LoraValue::Null,
    }
}

fn eval_property<S: GraphStorage + ?Sized>(
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

        LoraValue::Node(id) => {
            let Some(node) = ctx.storage.node_ref(*id) else {
                return LoraValue::Null;
            };
            node.properties
                .get(key)
                .map(LoraValue::from)
                .unwrap_or(LoraValue::Null)
        }

        LoraValue::Relationship(id) => {
            let Some(rel) = ctx.storage.relationship_ref(*id) else {
                return LoraValue::Null;
            };
            rel.properties
                .get(key)
                .map(LoraValue::from)
                .unwrap_or(LoraValue::Null)
        }

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

fn eval_unary(op: UnaryOp, value: LoraValue) -> LoraValue {
    match op {
        UnaryOp::Not => {
            if matches!(value, LoraValue::Null) {
                LoraValue::Null
            } else {
                LoraValue::Bool(!value.is_truthy())
            }
        }
        UnaryOp::Pos => value,
        UnaryOp::Neg => match value {
            LoraValue::Int(v) => LoraValue::Int(-v),
            LoraValue::Float(v) => LoraValue::Float(-v),
            _ => LoraValue::Null,
        },
    }
}

fn eval_binary(op: &BinaryOp, lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match op {
        // Lora three-valued boolean logic:
        // null AND false → false; null AND true → null; null AND null → null
        // null OR  true  → true;  null OR  false → null; null OR  null → null
        BinaryOp::And => {
            let l_null = matches!(lhs, LoraValue::Null);
            let r_null = matches!(rhs, LoraValue::Null);
            if l_null || r_null {
                // false AND null → false; null AND false → false
                if (!l_null && !lhs.is_truthy()) || (!r_null && !rhs.is_truthy()) {
                    LoraValue::Bool(false)
                } else {
                    LoraValue::Null
                }
            } else {
                LoraValue::Bool(lhs.is_truthy() && rhs.is_truthy())
            }
        }
        BinaryOp::Or => {
            let l_null = matches!(lhs, LoraValue::Null);
            let r_null = matches!(rhs, LoraValue::Null);
            if l_null || r_null {
                // true OR null → true; null OR true → true
                if (!l_null && lhs.is_truthy()) || (!r_null && rhs.is_truthy()) {
                    LoraValue::Bool(true)
                } else {
                    LoraValue::Null
                }
            } else {
                LoraValue::Bool(lhs.is_truthy() || rhs.is_truthy())
            }
        }
        BinaryOp::Xor => {
            if matches!(lhs, LoraValue::Null) || matches!(rhs, LoraValue::Null) {
                LoraValue::Null
            } else {
                LoraValue::Bool(lhs.is_truthy() ^ rhs.is_truthy())
            }
        }

        // Lora null semantics: any comparison involving null returns null.
        BinaryOp::Eq => {
            if matches!(lhs, LoraValue::Null) || matches!(rhs, LoraValue::Null) {
                LoraValue::Null
            } else {
                LoraValue::Bool(value_eq(&lhs, &rhs))
            }
        }
        BinaryOp::Ne => {
            if matches!(lhs, LoraValue::Null) || matches!(rhs, LoraValue::Null) {
                LoraValue::Null
            } else {
                LoraValue::Bool(!value_eq(&lhs, &rhs))
            }
        }

        // Lora null semantics: comparisons with null return null.
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => {
            if matches!(lhs, LoraValue::Null) || matches!(rhs, LoraValue::Null) {
                return LoraValue::Null;
            }
            match op {
                BinaryOp::Lt => cmp_numeric_or_string(lhs, rhs, |a, b| a < b, |a, b| a < b),
                BinaryOp::Gt => cmp_numeric_or_string(lhs, rhs, |a, b| a > b, |a, b| a > b),
                BinaryOp::Le => cmp_numeric_or_string(lhs, rhs, |a, b| a <= b, |a, b| a <= b),
                BinaryOp::Ge => cmp_numeric_or_string(lhs, rhs, |a, b| a >= b, |a, b| a >= b),
                _ => unreachable!(),
            }
        }

        BinaryOp::Add => add_values(lhs, rhs),
        BinaryOp::Sub => sub_values(lhs, rhs),
        BinaryOp::Mul => mul_values(lhs, rhs),
        BinaryOp::Div => div_values(lhs, rhs),
        BinaryOp::Mod => mod_values(lhs, rhs),
        BinaryOp::Pow => pow_values(lhs, rhs),

        BinaryOp::In => {
            if matches!(lhs, LoraValue::Null) {
                return LoraValue::Null;
            }
            match rhs {
                LoraValue::List(values) => LoraValue::Bool(values.iter().any(|v| value_eq(&lhs, v))),
                LoraValue::Null => LoraValue::Null,
                _ => LoraValue::Bool(false),
            }
        }

        BinaryOp::StartsWith => match (lhs, rhs) {
            (LoraValue::Null, _) | (_, LoraValue::Null) => LoraValue::Null,
            (LoraValue::String(a), LoraValue::String(b)) => LoraValue::Bool(a.starts_with(&b)),
            _ => LoraValue::Bool(false),
        },

        BinaryOp::EndsWith => match (lhs, rhs) {
            (LoraValue::Null, _) | (_, LoraValue::Null) => LoraValue::Null,
            (LoraValue::String(a), LoraValue::String(b)) => LoraValue::Bool(a.ends_with(&b)),
            _ => LoraValue::Bool(false),
        },

        BinaryOp::Contains => match (lhs, rhs) {
            (LoraValue::Null, _) | (_, LoraValue::Null) => LoraValue::Null,
            (LoraValue::String(a), LoraValue::String(b)) => LoraValue::Bool(a.contains(&b)),
            (LoraValue::List(a), b) => LoraValue::Bool(a.iter().any(|v| value_eq(v, &b))),
            _ => LoraValue::Bool(false),
        },

        BinaryOp::IsNull => LoraValue::Bool(matches!(lhs, LoraValue::Null)),
        BinaryOp::IsNotNull => LoraValue::Bool(!matches!(lhs, LoraValue::Null)),

        BinaryOp::RegexMatch => match (lhs, rhs) {
            (LoraValue::Null, _) | (_, LoraValue::Null) => LoraValue::Null,
            (LoraValue::String(s), LoraValue::String(pattern)) => {
                match regex::Regex::new(&format!("^(?:{pattern})$")) {
                    Ok(re) => LoraValue::Bool(re.is_match(&s)),
                    Err(_) => LoraValue::Null,
                }
            }
            _ => LoraValue::Bool(false),
        },
    }
}

fn eval_function<S: GraphStorage + ?Sized>(
    name: &str,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    let fq = name.to_ascii_lowercase();

    match fq.as_str() {
        "id" => {
            if let Some(LoraValue::Node(id)) = args.first() {
                LoraValue::Int(*id as i64)
            } else if let Some(LoraValue::Relationship(id)) = args.first() {
                LoraValue::Int(*id as i64)
            } else {
                LoraValue::Null
            }
        }

        "tolower" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.to_ascii_lowercase()),
            _ => LoraValue::Null,
        },

        "toupper" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.to_ascii_uppercase()),
            _ => LoraValue::Null,
        },

        "coalesce" => {
            for arg in args {
                if !matches!(arg, LoraValue::Null) {
                    return arg.clone();
                }
            }
            LoraValue::Null
        }

        "type" => match args.first() {
            Some(LoraValue::Relationship(id)) => ctx
                .storage
                .relationship_ref(*id)
                .map(|r| LoraValue::String(r.rel_type.clone()))
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "labels" => match args.first() {
            Some(LoraValue::Node(id)) => ctx
                .storage
                .node_ref(*id)
                .map(|n| {
                    LoraValue::List(n.labels.iter().map(|s| LoraValue::String(s.clone())).collect())
                })
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "keys" => match args.first() {
            Some(LoraValue::Node(id)) => ctx
                .storage
                .node_ref(*id)
                .map(|n| {
                    LoraValue::List(
                        n.properties
                            .keys()
                            .map(|k| LoraValue::String(k.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Relationship(id)) => ctx
                .storage
                .relationship_ref(*id)
                .map(|r| {
                    LoraValue::List(
                        r.properties
                            .keys()
                            .map(|k| LoraValue::String(k.clone()))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Map(m)) => {
                LoraValue::List(m.keys().cloned().map(LoraValue::String).collect())
            }
            _ => LoraValue::Null,
        },

        "size" | "length" => match args.first() {
            Some(LoraValue::List(l)) => LoraValue::Int(l.len() as i64),
            Some(LoraValue::String(s)) => LoraValue::Int(s.len() as i64),
            Some(LoraValue::Path(p)) => LoraValue::Int(p.rels.len() as i64),
            _ => LoraValue::Null,
        },

        "nodes" => match args.first() {
            Some(LoraValue::Path(p)) => {
                LoraValue::List(p.nodes.iter().map(|id| LoraValue::Node(*id)).collect())
            }
            _ => LoraValue::Null,
        },

        "relationships" => match args.first() {
            Some(LoraValue::Path(p)) => {
                LoraValue::List(p.rels.iter().map(|id| LoraValue::Relationship(*id)).collect())
            }
            _ => LoraValue::Null,
        },

        "head" => match args.first() {
            Some(LoraValue::List(l)) => l.first().cloned().unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "tail" => match args.first() {
            Some(LoraValue::List(l)) => {
                if l.is_empty() {
                    LoraValue::Null
                } else {
                    LoraValue::List(l[1..].to_vec())
                }
            }
            _ => LoraValue::Null,
        },

        "tostring" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::String(i.to_string()),
            Some(LoraValue::Float(f)) => LoraValue::String(f.to_string()),
            Some(LoraValue::Bool(b)) => LoraValue::String(b.to_string()),
            Some(LoraValue::String(s)) => LoraValue::String(s.clone()),
            Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Date(d)) => LoraValue::String(d.to_string()),
            Some(LoraValue::DateTime(dt)) => LoraValue::String(dt.to_string()),
            Some(LoraValue::LocalDateTime(dt)) => LoraValue::String(dt.to_string()),
            Some(LoraValue::Time(t)) => LoraValue::String(t.to_string()),
            Some(LoraValue::LocalTime(t)) => LoraValue::String(t.to_string()),
            Some(LoraValue::Duration(dur)) => LoraValue::String(dur.to_string()),
            _ => LoraValue::Null,
        },

        "tointeger" | "toint" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            Some(LoraValue::Float(f)) => LoraValue::Int(*f as i64),
            Some(LoraValue::String(s)) => s.parse::<i64>().ok().map(LoraValue::Int).unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "tofloat" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Float(*f),
            Some(LoraValue::Int(i)) => LoraValue::Float(*i as f64),
            Some(LoraValue::String(s)) => s.parse::<f64>().ok().map(LoraValue::Float).unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        "abs" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::Int(i.abs()),
            Some(LoraValue::Float(f)) => LoraValue::Float(f.abs()),
            _ => LoraValue::Null,
        },

        // -- Math functions ------------------------------------------------

        "ceil" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Int(f.ceil() as i64),
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            _ => LoraValue::Null,
        },

        "floor" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Int(f.floor() as i64),
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            _ => LoraValue::Null,
        },

        "round" => match args.first() {
            Some(LoraValue::Float(f)) => LoraValue::Int(f.round() as i64),
            Some(LoraValue::Int(i)) => LoraValue::Int(*i),
            _ => LoraValue::Null,
        },

        "sqrt" => match args.first() {
            Some(LoraValue::Float(f)) => {
                if *f < 0.0 {
                    LoraValue::Null
                } else {
                    LoraValue::Float(f.sqrt())
                }
            }
            Some(LoraValue::Int(i)) => {
                if *i < 0 {
                    LoraValue::Null
                } else {
                    LoraValue::Float((*i as f64).sqrt())
                }
            }
            _ => LoraValue::Null,
        },

        "sign" => match args.first() {
            Some(LoraValue::Int(i)) => LoraValue::Int(i.signum()),
            Some(LoraValue::Float(f)) => {
                if f.is_nan() {
                    LoraValue::Null
                } else {
                    LoraValue::Int(f.signum() as i64)
                }
            }
            _ => LoraValue::Null,
        },

        // -- String functions -----------------------------------------------

        "trim" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.trim().to_string()),
            _ => LoraValue::Null,
        },

        "ltrim" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.trim_start().to_string()),
            _ => LoraValue::Null,
        },

        "rtrim" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.trim_end().to_string()),
            _ => LoraValue::Null,
        },

        "replace" => {
            match (args.first(), args.get(1), args.get(2)) {
                (Some(LoraValue::String(s)), Some(LoraValue::String(search)), Some(LoraValue::String(replacement))) => {
                    LoraValue::String(s.replace(search.as_str(), replacement.as_str()))
                }
                _ => LoraValue::Null,
            }
        }

        "split" => {
            match (args.first(), args.get(1)) {
                (Some(LoraValue::String(s)), Some(LoraValue::String(delimiter))) => {
                    LoraValue::List(
                        s.split(delimiter.as_str())
                            .map(|part| LoraValue::String(part.to_string()))
                            .collect(),
                    )
                }
                _ => LoraValue::Null,
            }
        }

        "substring" => {
            match args.first() {
                Some(LoraValue::String(s)) => {
                    let start = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0) as usize;
                    if start > s.len() {
                        return LoraValue::String(String::new());
                    }
                    match args.get(2).and_then(|v| v.as_i64()) {
                        Some(len) => {
                            let len = len.max(0) as usize;
                            let end = (start + len).min(s.len());
                            LoraValue::String(s[start..end].to_string())
                        }
                        None => {
                            // Two-argument form: substring(s, start) — rest of string
                            LoraValue::String(s[start..].to_string())
                        }
                    }
                }
                _ => LoraValue::Null,
            }
        }

        "reverse" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::String(s.chars().rev().collect()),
            Some(LoraValue::List(l)) => LoraValue::List(l.iter().rev().cloned().collect()),
            _ => LoraValue::Null,
        },

        "left" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(s)), Some(LoraValue::Int(n))) => {
                let n = (*n).max(0) as usize;
                LoraValue::String(s.chars().take(n).collect())
            }
            _ => LoraValue::Null,
        },

        "right" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(s)), Some(LoraValue::Int(n))) => {
                let n = (*n).max(0) as usize;
                let char_count = s.chars().count();
                let skip = char_count.saturating_sub(n);
                LoraValue::String(s.chars().skip(skip).collect())
            }
            _ => LoraValue::Null,
        },

        "properties" => match args.first() {
            Some(LoraValue::Node(id)) => ctx
                .storage
                .node_ref(*id)
                .map(|n| {
                    LoraValue::Map(
                        n.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Relationship(id)) => ctx
                .storage
                .relationship_ref(*id)
                .map(|r| {
                    LoraValue::Map(
                        r.properties
                            .iter()
                            .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                            .collect(),
                    )
                })
                .unwrap_or(LoraValue::Null),
            Some(LoraValue::Map(m)) => LoraValue::Map(m.clone()),
            _ => LoraValue::Null,
        },

        "timestamp" => {
            use std::time::{SystemTime, UNIX_EPOCH};
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            LoraValue::Int(millis)
        }

        "range" => {
            let start = args.first().and_then(|v| v.as_i64()).unwrap_or(0);
            let end = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
            let step = args.get(2).and_then(|v| v.as_i64()).unwrap_or(1);
            if step == 0 {
                return LoraValue::Null;
            }
            let mut result = Vec::new();
            let mut i = start;
            if step > 0 {
                while i <= end {
                    result.push(LoraValue::Int(i));
                    i += step;
                }
            } else {
                while i >= end {
                    result.push(LoraValue::Int(i));
                    i += step;
                }
            }
            LoraValue::List(result)
        }

        // -- Last (list) -----------------------------------------------------

        "last" => match args.first() {
            Some(LoraValue::List(l)) => l.last().cloned().unwrap_or(LoraValue::Null),
            _ => LoraValue::Null,
        },

        // -- String padding / char_length / normalize -------------------------

        "lpad" => {
            match (args.first(), args.get(1), args.get(2)) {
                (Some(LoraValue::String(s)), Some(len_val), Some(LoraValue::String(pad))) => {
                    let target_len = len_val.as_i64().unwrap_or(0).max(0) as usize;
                    let current_len = s.chars().count();
                    if current_len >= target_len {
                        LoraValue::String(s.clone())
                    } else {
                        let pad_needed = target_len - current_len;
                        let pad_chars: String = pad.chars().cycle().take(pad_needed).collect();
                        LoraValue::String(format!("{}{}", pad_chars, s))
                    }
                }
                _ => LoraValue::Null,
            }
        }

        "rpad" => {
            match (args.first(), args.get(1), args.get(2)) {
                (Some(LoraValue::String(s)), Some(len_val), Some(LoraValue::String(pad))) => {
                    let target_len = len_val.as_i64().unwrap_or(0).max(0) as usize;
                    let current_len = s.chars().count();
                    if current_len >= target_len {
                        LoraValue::String(s.clone())
                    } else {
                        let pad_needed = target_len - current_len;
                        let pad_chars: String = pad.chars().cycle().take(pad_needed).collect();
                        LoraValue::String(format!("{}{}", s, pad_chars))
                    }
                }
                _ => LoraValue::Null,
            }
        }

        "char_length" => match args.first() {
            Some(LoraValue::String(s)) => LoraValue::Int(s.chars().count() as i64),
            _ => LoraValue::Null,
        },

        "normalize" => match args.first() {
            // Basic NFC normalization — for ASCII input, returns as-is
            Some(LoraValue::String(s)) => LoraValue::String(s.clone()),
            _ => LoraValue::Null,
        },

        // -- toBoolean --------------------------------------------------------

        "toboolean" | "tobooleanornull" => match args.first() {
            Some(LoraValue::Bool(b)) => LoraValue::Bool(*b),
            Some(LoraValue::String(s)) => match s.to_ascii_lowercase().as_str() {
                "true" => LoraValue::Bool(true),
                "false" => LoraValue::Bool(false),
                _ => LoraValue::Null,
            },
            Some(LoraValue::Int(i)) => match *i {
                0 => LoraValue::Bool(false),
                _ => LoraValue::Bool(true),
            },
            Some(LoraValue::Null) => LoraValue::Null,
            _ => LoraValue::Null,
        },

        // -- valueType --------------------------------------------------------

        "valuetype" => match args.first() {
            Some(LoraValue::Null) => LoraValue::String("NULL".to_string()),
            Some(LoraValue::Bool(_)) => LoraValue::String("BOOLEAN".to_string()),
            Some(LoraValue::Int(_)) => LoraValue::String("INTEGER".to_string()),
            Some(LoraValue::Float(_)) => LoraValue::String("FLOAT".to_string()),
            Some(LoraValue::String(_)) => LoraValue::String("STRING".to_string()),
            Some(LoraValue::List(items)) => {
                // Determine element type for homogeneous lists
                let elem_type = if items.is_empty() {
                    "ANY"
                } else {
                    let first_type = match &items[0] {
                        LoraValue::Int(_) => "INTEGER",
                        LoraValue::Float(_) => "FLOAT",
                        LoraValue::String(_) => "STRING",
                        LoraValue::Bool(_) => "BOOLEAN",
                        LoraValue::Null => "ANY",
                        _ => "ANY",
                    };
                    let homogeneous = items.iter().all(|v| {
                        matches!(
                            (v, first_type),
                            (LoraValue::Int(_), "INTEGER")
                                | (LoraValue::Float(_), "FLOAT")
                                | (LoraValue::String(_), "STRING")
                                | (LoraValue::Bool(_), "BOOLEAN")
                        )
                    });
                    if homogeneous { first_type } else { "ANY" }
                };
                LoraValue::String(format!("LIST<{elem_type}>"))
            }
            Some(LoraValue::Map(_)) => LoraValue::String("MAP".to_string()),
            Some(LoraValue::Node(_)) => LoraValue::String("NODE".to_string()),
            Some(LoraValue::Relationship(_)) => LoraValue::String("RELATIONSHIP".to_string()),
            Some(LoraValue::Path(_)) => LoraValue::String("PATH".to_string()),
            Some(LoraValue::Date(_)) => LoraValue::String("DATE".to_string()),
            Some(LoraValue::DateTime(_)) => LoraValue::String("DATE_TIME".to_string()),
            Some(LoraValue::LocalDateTime(_)) => LoraValue::String("LOCAL_DATE_TIME".to_string()),
            Some(LoraValue::Time(_)) => LoraValue::String("TIME".to_string()),
            Some(LoraValue::LocalTime(_)) => LoraValue::String("LOCAL_TIME".to_string()),
            Some(LoraValue::Duration(_)) => LoraValue::String("DURATION".to_string()),
            Some(LoraValue::Point(_)) => LoraValue::String("POINT".to_string()),
            None => LoraValue::Null,
        },

        // -- Trigonometric / logarithmic / constants --------------------------

        "log" | "ln" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if f > 0.0 => LoraValue::Float(f.ln()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "log10" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if f > 0.0 => LoraValue::Float(f.log10()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "exp" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.exp()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "sin" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.sin()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "cos" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.cos()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "tan" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.tan()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "asin" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if (-1.0..=1.0).contains(&f) => LoraValue::Float(f.asin()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "acos" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) if (-1.0..=1.0).contains(&f) => LoraValue::Float(f.acos()),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "atan" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.atan()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "atan2" => match (args.first(), args.get(1)) {
            (Some(y_val), Some(x_val)) => match (y_val.as_f64(), x_val.as_f64()) {
                (Some(y), Some(x)) => LoraValue::Float(y.atan2(x)),
                _ => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "pi" => LoraValue::Float(std::f64::consts::PI),

        "e" => LoraValue::Float(std::f64::consts::E),

        "rand" => {
            use std::time::{SystemTime, UNIX_EPOCH};
            // Simple pseudo-random using system time nanoseconds
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            // Use a simple hash to get pseudo-random distribution
            let hash = ((nanos as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407)) as f64;
            LoraValue::Float((hash / u64::MAX as f64).abs())
        }

        "degrees" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.to_degrees()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        "radians" => match args.first() {
            Some(v) => match v.as_f64() {
                Some(f) => LoraValue::Float(f.to_radians()),
                None => LoraValue::Null,
            },
            _ => LoraValue::Null,
        },

        // -- Temporal constructors -------------------------------------------

        "date" => match args.first() {
            None => LoraValue::Date(LoraDate::today()),
            Some(LoraValue::String(s)) => match LoraDate::parse(s) {
                Ok(d) => LoraValue::Date(d),
                Err(e) => { set_eval_error(e); LoraValue::Null }
            },
            Some(LoraValue::Map(m)) => {
                let year = m.get("year").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let month = m.get("month").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                let day = m.get("day").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                match LoraDate::new(year, month, day) {
                    Ok(d) => LoraValue::Date(d),
                    Err(e) => { set_eval_error(e); LoraValue::Null }
                }
            }
            // Roundtrip: date(date) -> date
            Some(LoraValue::Date(d)) => LoraValue::Date(d.clone()),
            _ => LoraValue::Null,
        },

        "datetime" => match args.first() {
            None => LoraValue::DateTime(LoraDateTime::now()),
            Some(LoraValue::String(s)) => match LoraDateTime::parse(s) {
                Ok(dt) => LoraValue::DateTime(dt),
                Err(e) => { set_eval_error(e); LoraValue::Null }
            },
            Some(LoraValue::Map(m)) => {
                let year = m.get("year").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let month = m.get("month").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                let day = m.get("day").and_then(|v| v.as_i64()).unwrap_or(1) as u32;
                let hour = m.get("hour").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let minute = m.get("minute").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let second = m.get("second").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let ms = m.get("millisecond").and_then(|v| v.as_i64()).unwrap_or(0) as u32;
                let offset = if let Some(LoraValue::String(tz)) = m.get("timezone") {
                    // Simple named timezone handling: map common names to offsets
                    timezone_name_to_offset(tz)
                } else {
                    0
                };
                match LoraDateTime::new(year, month, day, hour, minute, second, ms * 1_000_000, offset) {
                    Ok(dt) => LoraValue::DateTime(dt),
                    Err(e) => { set_eval_error(e); LoraValue::Null }
                }
            }
            _ => LoraValue::Null,
        },

        "time" => match args.first() {
            None => LoraValue::Time(LoraTime::now()),
            Some(LoraValue::String(s)) => match LoraTime::parse(s) {
                Ok(t) => LoraValue::Time(t),
                Err(e) => { set_eval_error(e); LoraValue::Null }
            },
            _ => LoraValue::Null,
        },

        "localtime" => match args.first() {
            None => LoraValue::LocalTime(LoraLocalTime::now()),
            Some(LoraValue::String(s)) => match LoraLocalTime::parse(s) {
                Ok(t) => LoraValue::LocalTime(t),
                Err(e) => { set_eval_error(e); LoraValue::Null }
            },
            _ => LoraValue::Null,
        },

        "localdatetime" => match args.first() {
            None => LoraValue::LocalDateTime(LoraLocalDateTime::now()),
            Some(LoraValue::String(s)) => match LoraLocalDateTime::parse(s) {
                Ok(dt) => LoraValue::LocalDateTime(dt),
                Err(e) => { set_eval_error(e); LoraValue::Null }
            },
            _ => LoraValue::Null,
        },

        "duration" => match args.first() {
            Some(LoraValue::String(s)) => match LoraDuration::parse(s) {
                Ok(d) => LoraValue::Duration(d),
                Err(e) => { set_eval_error(e); LoraValue::Null }
            },
            Some(LoraValue::Map(m)) => {
                let years = m.get("years").and_then(|v| v.as_i64()).unwrap_or(0);
                let months = m.get("months").and_then(|v| v.as_i64()).unwrap_or(0);
                let days = m.get("days").and_then(|v| v.as_i64()).unwrap_or(0);
                let hours = m.get("hours").and_then(|v| v.as_i64()).unwrap_or(0);
                let minutes = m.get("minutes").and_then(|v| v.as_i64()).unwrap_or(0);
                let seconds = m.get("seconds").and_then(|v| v.as_i64()).unwrap_or(0);
                LoraValue::Duration(LoraDuration {
                    months: years * 12 + months,
                    days,
                    seconds: hours * 3600 + minutes * 60 + seconds,
                    nanoseconds: 0,
                })
            }
            _ => LoraValue::Null,
        },

        // -- Temporal namespace functions -----------------------------------

        "date.truncate" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(unit)), Some(LoraValue::Date(d))) => {
                match unit.as_str() {
                    "month" => LoraValue::Date(d.truncate_to_month()),
                    "year" => LoraValue::Date(LoraDate { year: d.year, month: 1, day: 1 }),
                    _ => LoraValue::Date(d.clone()),
                }
            }
            _ => LoraValue::Null,
        },

        "datetime.truncate" => match (args.first(), args.get(1)) {
            (Some(LoraValue::String(unit)), Some(LoraValue::DateTime(dt))) => {
                match unit.as_str() {
                    "day" => LoraValue::DateTime(dt.truncate_to_day()),
                    "hour" => LoraValue::DateTime(dt.truncate_to_hour()),
                    "month" => LoraValue::DateTime(LoraDateTime {
                        year: dt.year, month: dt.month, day: 1,
                        hour: 0, minute: 0, second: 0, nanosecond: 0,
                        offset_seconds: dt.offset_seconds,
                    }),
                    _ => LoraValue::DateTime(dt.clone()),
                }
            }
            _ => LoraValue::Null,
        },

        "duration.between" => match (args.first(), args.get(1)) {
            (Some(LoraValue::Date(d1)), Some(LoraValue::Date(d2))) => {
                LoraValue::Duration(LoraDuration::between_dates(d1, d2))
            }
            (Some(LoraValue::DateTime(dt1)), Some(LoraValue::DateTime(dt2))) => {
                LoraValue::Duration(LoraDuration::between_datetimes(dt1, dt2))
            }
            _ => LoraValue::Null,
        },

        "duration.indays" => match (args.first(), args.get(1)) {
            (Some(LoraValue::Date(d1)), Some(LoraValue::Date(d2))) => {
                LoraValue::Duration(LoraDuration::in_days(d1, d2))
            }
            _ => LoraValue::Null,
        },

        // -- Spatial functions -----------------------------------------------

        "point" => match args.first() {
            None | Some(LoraValue::Null) => LoraValue::Null,
            Some(LoraValue::Map(m)) => match build_point_from_map(m) {
                Ok(Some(p)) => LoraValue::Point(p),
                Ok(None) => LoraValue::Null,
                Err(msg) => {
                    set_eval_error(msg);
                    LoraValue::Null
                }
            },
            Some(_) => {
                set_eval_error("point() requires a map argument".to_string());
                LoraValue::Null
            }
        },

        "distance" => match (args.first(), args.get(1)) {
            (Some(LoraValue::Point(a)), Some(LoraValue::Point(b))) => {
                match point_distance(a, b) {
                    Some(d) => LoraValue::Float(d),
                    None => {
                        set_eval_error("Cannot compute distance between points with different SRIDs".to_string());
                        LoraValue::Null
                    }
                }
            }
            _ => LoraValue::Null,
        },

        _ => LoraValue::Null,
    }
}

thread_local! {
    static EVAL_ERROR: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}

fn set_eval_error(msg: String) {
    EVAL_ERROR.with(|e| *e.borrow_mut() = Some(msg));
}

pub fn take_eval_error() -> Option<String> {
    EVAL_ERROR.with(|e| e.borrow_mut().take())
}

/// Parse a `point(map)` argument into a `LoraPoint`.
///
/// - `Ok(Some(p))` → construction succeeded.
/// - `Ok(None)`    → null propagation: the map contained a null on one of the
///                   recognised coordinate/crs/srid keys, so the call should
///                   return `null` *without* signalling an error.
/// - `Err(msg)`    → validation failure (unknown key, bad type, conflicting
///                   crs/srid, dimensionality mismatch, missing coords, …).
fn build_point_from_map(
    map: &BTreeMap<String, LoraValue>,
) -> Result<Option<LoraPoint>, String> {
    const KNOWN_KEYS: &[&str] = &[
        "x", "y", "z", "longitude", "latitude", "height", "crs", "srid",
    ];

    // Reject unknown keys up front — strictness is preferred over silently
    // ignoring typos like `{lon: 4, lat: 52}`.
    for k in map.keys() {
        if !KNOWN_KEYS.iter().any(|known| known.eq_ignore_ascii_case(k)) {
            return Err(format!("point() got unknown key '{k}'"));
        }
    }

    // Pull every recognised coordinate slot. `Some(None)` means the key was
    // present but held a null (→ null propagation); `None` means absent.
    let x = take_numeric(map, "x")?;
    let y = take_numeric(map, "y")?;
    let z = take_numeric(map, "z")?;
    let longitude = take_numeric(map, "longitude")?;
    let latitude = take_numeric(map, "latitude")?;
    let height = take_numeric(map, "height")?;
    let crs = take_string(map, "crs")?;
    let srid = take_integer(map, "srid")?;

    // Null propagation: any null on a recognised key → return null.
    if matches!(x, Some(None))
        || matches!(y, Some(None))
        || matches!(z, Some(None))
        || matches!(longitude, Some(None))
        || matches!(latitude, Some(None))
        || matches!(height, Some(None))
        || matches!(crs, Some(None))
        || matches!(srid, Some(None))
    {
        return Ok(None);
    }

    // Flatten `Option<Option<T>>` now that null-propagation is resolved.
    let x = x.and_then(|v| v);
    let y = y.and_then(|v| v);
    let z = z.and_then(|v| v);
    let longitude = longitude.and_then(|v| v);
    let latitude = latitude.and_then(|v| v);
    let height = height.and_then(|v| v);
    let crs = crs.and_then(|v| v);
    let srid = srid.and_then(|v| v);

    // Detect coordinate family. Mixing x/y with longitude/latitude is
    // ambiguous and rejected.
    let has_cartesian = x.is_some() || y.is_some();
    let has_geographic = longitude.is_some() || latitude.is_some();
    if has_cartesian && has_geographic {
        return Err(
            "point() cannot mix cartesian (x/y) and geographic (longitude/latitude) keys"
                .to_string(),
        );
    }

    let (family, first, second) = if has_geographic {
        (
            PointKeyFamily::Geographic,
            longitude
                .ok_or_else(|| "point() is missing longitude".to_string())?,
            latitude
                .ok_or_else(|| "point() is missing latitude".to_string())?,
        )
    } else if has_cartesian {
        (
            PointKeyFamily::Cartesian,
            x.ok_or_else(|| "point() is missing x".to_string())?,
            y.ok_or_else(|| "point() is missing y".to_string())?,
        )
    } else {
        return Err(
            "point() requires coordinates — either {x, y} or {longitude, latitude}"
                .to_string(),
        );
    };

    // Third dimension. `z` and `height` are aliases; specifying both is an
    // error even if they agree, to keep the input unambiguous.
    let third = match (z, height) {
        (Some(_), Some(_)) => {
            return Err(
                "point() cannot specify both 'z' and 'height' — they are aliases".to_string(),
            );
        }
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    };

    let is_3d = third.is_some();
    let final_srid = resolve_srid(crs.as_deref(), srid, family, is_3d)?;

    // Construct — we use the raw struct to avoid re-deriving 2D-vs-3D via
    // the convenience constructors once we already have the resolved SRID.
    let point = LoraPoint {
        x: first,
        y: second,
        z: if srid_is_3d(final_srid) { third } else { None },
        srid: final_srid,
    };

    Ok(Some(point))
}

/// Fetch a numeric slot from a `point()` map.
///
/// Returns:
/// - `Ok(None)`              → key absent.
/// - `Ok(Some(None))`        → key present with `null` (null-propagate).
/// - `Ok(Some(Some(n)))`     → numeric value; `Int`s are coerced to `f64`.
/// - `Err(msg)`              → present but not numeric / not null.
fn take_numeric(
    map: &BTreeMap<String, LoraValue>,
    key: &str,
) -> Result<Option<Option<f64>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(LoraValue::Null) => Ok(Some(None)),
        Some(LoraValue::Int(v)) => Ok(Some(Some(*v as f64))),
        Some(LoraValue::Float(v)) => Ok(Some(Some(*v))),
        Some(other) => Err(format!(
            "point() field '{key}' must be numeric, got {}",
            crate::errors::value_kind(other)
        )),
    }
}

fn take_string(
    map: &BTreeMap<String, LoraValue>,
    key: &str,
) -> Result<Option<Option<String>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(LoraValue::Null) => Ok(Some(None)),
        Some(LoraValue::String(s)) => Ok(Some(Some(s.clone()))),
        Some(other) => Err(format!(
            "point() field '{key}' must be a string, got {}",
            crate::errors::value_kind(other)
        )),
    }
}

fn take_integer(
    map: &BTreeMap<String, LoraValue>,
    key: &str,
) -> Result<Option<Option<i64>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(LoraValue::Null) => Ok(Some(None)),
        Some(LoraValue::Int(v)) => Ok(Some(Some(*v))),
        Some(other) => Err(format!(
            "point() field '{key}' must be an integer, got {}",
            crate::errors::value_kind(other)
        )),
    }
}

/// Simple named timezone to offset mapping for common zones.
fn timezone_name_to_offset(name: &str) -> i32 {
    // This is a simplified mapping; a full implementation would use a timezone database.
    match name {
        "UTC" | "GMT" | "Z" => 0,
        "Europe/London" => 0,      // Ignoring DST for simplicity
        "Europe/Amsterdam" | "Europe/Berlin" | "Europe/Paris"
        | "CET" => 3600,           // +01:00 (ignoring DST)
        "Europe/Moscow" => 10800,  // +03:00
        "US/Eastern" | "America/New_York" | "EST" => -18000, // -05:00
        "US/Central" | "America/Chicago" | "CST" => -21600,  // -06:00
        "US/Mountain" | "America/Denver" | "MST" => -25200,  // -07:00
        "US/Pacific" | "America/Los_Angeles" | "PST" => -28800, // -08:00
        "Asia/Tokyo" | "JST" => 32400, // +09:00
        "Asia/Shanghai" | "Asia/Hong_Kong" => 28800, // +08:00
        _ => 0, // Default to UTC for unknown timezones
    }
}

fn value_eq(a: &LoraValue, b: &LoraValue) -> bool {
    match (a, b) {
        (LoraValue::Null, LoraValue::Null) => true,
        (LoraValue::Bool(x), LoraValue::Bool(y)) => x == y,
        (LoraValue::Int(x), LoraValue::Int(y)) => x == y,
        (LoraValue::Float(x), LoraValue::Float(y)) => x == y,
        (LoraValue::Int(x), LoraValue::Float(y)) => (*x as f64) == *y,
        (LoraValue::Float(x), LoraValue::Int(y)) => *x == (*y as f64),
        (LoraValue::String(x), LoraValue::String(y)) => x == y,
        (LoraValue::Node(x), LoraValue::Node(y)) => x == y,
        (LoraValue::Relationship(x), LoraValue::Relationship(y)) => x == y,
        (LoraValue::List(x), LoraValue::List(y)) => x == y,
        (LoraValue::Map(x), LoraValue::Map(y)) => x == y,
        (LoraValue::Date(x), LoraValue::Date(y)) => x == y,
        (LoraValue::DateTime(x), LoraValue::DateTime(y)) => x == y,
        (LoraValue::LocalDateTime(x), LoraValue::LocalDateTime(y)) => x == y,
        (LoraValue::Time(x), LoraValue::Time(y)) => x == y,
        (LoraValue::LocalTime(x), LoraValue::LocalTime(y)) => x == y,
        (LoraValue::Duration(x), LoraValue::Duration(y)) => x == y,
        (LoraValue::Point(x), LoraValue::Point(y)) => x == y,
        _ => false,
    }
}

fn cmp_numeric_or_string(
    lhs: LoraValue,
    rhs: LoraValue,
    num_cmp: impl Fn(f64, f64) -> bool,
    str_cmp: impl Fn(&str, &str) -> bool,
) -> LoraValue {
    match (&lhs, &rhs) {
        (LoraValue::String(a), LoraValue::String(b)) => LoraValue::Bool(str_cmp(a, b)),
        (LoraValue::Date(a), LoraValue::Date(b)) => {
            LoraValue::Bool(num_cmp(a.to_epoch_days() as f64, b.to_epoch_days() as f64))
        }
        (LoraValue::DateTime(a), LoraValue::DateTime(b)) => {
            LoraValue::Bool(num_cmp(a.to_epoch_millis() as f64, b.to_epoch_millis() as f64))
        }
        (LoraValue::Duration(a), LoraValue::Duration(b)) => {
            LoraValue::Bool(num_cmp(a.total_seconds_approx(), b.total_seconds_approx()))
        }
        _ => match (lhs.as_f64(), rhs.as_f64()) {
            (Some(a), Some(b)) => LoraValue::Bool(num_cmp(a, b)),
            _ => LoraValue::Bool(false),
        },
    }
}

fn add_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Int(a), LoraValue::Int(b)) => LoraValue::Int(a + b),
        (LoraValue::String(a), LoraValue::String(b)) => LoraValue::String(a + &b),
        (LoraValue::List(mut a), LoraValue::List(b)) => {
            a.extend(b);
            LoraValue::List(a)
        }
        // Temporal + Duration
        (LoraValue::Date(d), LoraValue::Duration(dur)) => LoraValue::Date(d.add_duration(&dur)),
        (LoraValue::Duration(dur), LoraValue::Date(d)) => LoraValue::Date(d.add_duration(&dur)),
        (LoraValue::DateTime(dt), LoraValue::Duration(dur)) => LoraValue::DateTime(dt.add_duration(&dur)),
        (LoraValue::Duration(dur), LoraValue::DateTime(dt)) => LoraValue::DateTime(dt.add_duration(&dur)),
        (LoraValue::Duration(a), LoraValue::Duration(b)) => LoraValue::Duration(a.add(&b)),
        // Type errors for temporal + non-duration
        (LoraValue::Date(_), _) | (_, LoraValue::Date(_)) => {
            set_eval_error("Cannot add non-duration to date".to_string());
            LoraValue::Null
        }
        (LoraValue::DateTime(_), _) | (_, LoraValue::DateTime(_)) => {
            set_eval_error("Cannot add non-duration to datetime".to_string());
            LoraValue::Null
        }
        (a, b) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => LoraValue::Float(a + b),
            _ => LoraValue::Null,
        },
    }
}

fn sub_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Int(a), LoraValue::Int(b)) => LoraValue::Int(a - b),
        // Temporal - Duration
        (LoraValue::Date(d), LoraValue::Duration(dur)) => LoraValue::Date(d.sub_duration(&dur)),
        (LoraValue::DateTime(dt), LoraValue::Duration(dur)) => {
            LoraValue::DateTime(dt.add_duration(&dur.negate()))
        }
        // Temporal - Temporal -> Duration
        (LoraValue::Date(d1), LoraValue::Date(d2)) => {
            LoraValue::Duration(LoraDuration::in_days(&d2, &d1))
        }
        (LoraValue::DateTime(dt1), LoraValue::DateTime(dt2)) => {
            LoraValue::Duration(LoraDuration::between_datetimes(&dt2, &dt1))
        }
        // Duration - Duration
        (LoraValue::Duration(a), LoraValue::Duration(b)) => LoraValue::Duration(a.add(&b.negate())),
        (a, b) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => LoraValue::Float(a - b),
            _ => LoraValue::Null,
        },
    }
}

fn mul_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Int(a), LoraValue::Int(b)) => LoraValue::Int(a * b),
        (LoraValue::Duration(d), LoraValue::Int(n)) => LoraValue::Duration(d.mul_int(n)),
        (LoraValue::Int(n), LoraValue::Duration(d)) => LoraValue::Duration(d.mul_int(n)),
        (a, b) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => LoraValue::Float(a * b),
            _ => LoraValue::Null,
        },
    }
}

fn div_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (&lhs, &rhs) {
        (LoraValue::Duration(d), LoraValue::Int(n)) if *n != 0 => {
            return LoraValue::Duration(d.div_int(*n));
        }
        _ => {}
    }
    match (lhs.as_f64(), rhs.as_f64()) {
        (Some(_), Some(0.0)) => LoraValue::Null,
        (Some(a), Some(b)) => LoraValue::Float(a / b),
        _ => LoraValue::Null,
    }
}

fn mod_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Int(_), LoraValue::Int(0)) => LoraValue::Null,
        (LoraValue::Int(a), LoraValue::Int(b)) => LoraValue::Int(a % b),
        _ => LoraValue::Null,
    }
}

fn pow_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs.as_f64(), rhs.as_f64()) {
        (Some(a), Some(b)) => {
            let out = a.powf(b);
            if out.fract() == 0.0 {
                LoraValue::Int(out as i64)
            } else {
                LoraValue::Float(out)
            }
        }
        _ => LoraValue::Null,
    }
}