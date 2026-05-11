//! Post-processing for `SHOW INDEXES` / `SHOW CONSTRAINTS` tails.
//!
//! Neo4j allows a YIELD-anchored pipeline on these DDL commands:
//!
//! ```cypher
//! SHOW INDEXES
//!   YIELD name, type [ORDER BY ...] [SKIP ...] [LIMIT ...]
//!   [WHERE expr]
//!   [RETURN items [ORDER BY ...] [SKIP ...] [LIMIT ...]]
//! ```
//!
//! Catalog rows are flat scalars (plus `LIST<STRING>` for
//! `labelsOrTypes` and `properties`), so we evaluate the pipeline
//! directly here rather than routing through the analyzer/compiler.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use lora_ast::{
    BinaryOp, Expr, ProjectionItem, ShowPipeline, ShowReturn, ShowYield, SortDirection, SortItem,
    UnaryOp,
};
use lora_executor::{LoraValue, Row};

use super::row_projection::{
    lookup_column, project_yield_items, row_from_columns, ColumnLookupContext, NamedColumn,
};

pub(crate) fn apply_pipeline(
    rows: Vec<Row>,
    pipeline: &ShowPipeline,
    params: &BTreeMap<String, LoraValue>,
) -> Result<Vec<Row>> {
    let mut rows = apply_yield(rows, &pipeline.yield_part)?;

    if let Some(where_expr) = &pipeline.where_ {
        rows = filter_rows(rows, where_expr, params)?;
    }

    rows = apply_sort_skip_limit(
        rows,
        &pipeline.yield_part.order,
        pipeline.yield_part.skip.as_ref(),
        pipeline.yield_part.limit.as_ref(),
        params,
    )?;

    if let Some(ret) = &pipeline.return_part {
        rows = apply_return(rows, ret, params)?;
    }

    Ok(rows)
}

fn apply_yield(rows: Vec<Row>, y: &ShowYield) -> Result<Vec<Row>> {
    project_yield_items(rows, &y.items, y.star, ColumnLookupContext::ShowPipeline)
}

fn apply_return(
    rows: Vec<Row>,
    ret: &ShowReturn,
    params: &BTreeMap<String, LoraValue>,
) -> Result<Vec<Row>> {
    let projected = project_rows(rows, &ret.items, params)?;
    apply_sort_skip_limit(
        projected,
        &ret.order,
        ret.skip.as_ref(),
        ret.limit.as_ref(),
        params,
    )
}

fn project_rows(
    rows: Vec<Row>,
    items: &[ProjectionItem],
    params: &BTreeMap<String, LoraValue>,
) -> Result<Vec<Row>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut columns = Vec::new();
        for item in items {
            match item {
                ProjectionItem::Star { .. } => {
                    for (_, name, value) in row.iter_named() {
                        columns.push(NamedColumn::new(name.into_owned(), value.clone()));
                    }
                }
                ProjectionItem::Expr { expr, alias, .. } => {
                    let value = eval_expr(expr, &row, params)?;
                    let name = match alias {
                        Some(v) => v.name.clone(),
                        None => render_expr_label(expr),
                    };
                    columns.push(NamedColumn::new(name, value));
                }
            }
        }
        out.push(row_from_columns(columns));
    }
    Ok(out)
}

fn filter_rows(
    rows: Vec<Row>,
    where_expr: &Expr,
    params: &BTreeMap<String, LoraValue>,
) -> Result<Vec<Row>> {
    let mut out = Vec::new();
    for row in rows {
        let v = eval_expr(where_expr, &row, params)?;
        if v.is_truthy() {
            out.push(row);
        }
    }
    Ok(out)
}

fn apply_sort_skip_limit(
    mut rows: Vec<Row>,
    order: &[SortItem],
    skip: Option<&Expr>,
    limit: Option<&Expr>,
    params: &BTreeMap<String, LoraValue>,
) -> Result<Vec<Row>> {
    if !order.is_empty() {
        // Pre-compute keys to avoid re-evaluating expressions on every
        // comparator call.
        let mut keyed: Vec<(Vec<LoraValue>, Row)> = rows
            .into_iter()
            .map(|row| {
                let keys = order
                    .iter()
                    .map(|si| eval_expr(&si.expr, &row, params))
                    .collect::<Result<Vec<_>>>()?;
                Ok::<_, anyhow::Error>((keys, row))
            })
            .collect::<Result<_>>()?;
        keyed.sort_by(|a, b| {
            for (i, dir) in order.iter().map(|si| si.direction).enumerate() {
                let ord = compare_values(&a.0[i], &b.0[i]);
                if ord != Ordering::Equal {
                    return match dir {
                        SortDirection::Asc => ord,
                        SortDirection::Desc => ord.reverse(),
                    };
                }
            }
            Ordering::Equal
        });
        rows = keyed.into_iter().map(|(_, r)| r).collect();
    }

    let skip_n = match skip {
        Some(e) => eval_usize(e, params, "SKIP")?,
        None => 0,
    };
    let limit_n = match limit {
        Some(e) => Some(eval_usize(e, params, "LIMIT")?),
        None => None,
    };

    if skip_n >= rows.len() {
        return Ok(Vec::new());
    }
    let mut iter = rows.into_iter().skip(skip_n);
    let result: Vec<Row> = if let Some(n) = limit_n {
        iter.by_ref().take(n).collect()
    } else {
        iter.collect()
    };
    Ok(result)
}

fn eval_usize(expr: &Expr, params: &BTreeMap<String, LoraValue>, label: &str) -> Result<usize> {
    let row = Row::new();
    let v = eval_expr(expr, &row, params)?;
    match v {
        LoraValue::Int(n) if n >= 0 => Ok(n as usize),
        other => Err(anyhow!(
            "{label} must evaluate to a non-negative integer, got {other:?}"
        )),
    }
}

// ---------- Expression evaluation against a flat catalog row ----------

fn eval_expr(expr: &Expr, row: &Row, params: &BTreeMap<String, LoraValue>) -> Result<LoraValue> {
    match expr {
        Expr::Variable(v) => lookup_column(row, &v.name, ColumnLookupContext::ShowPipeline),
        Expr::Integer(n, _) => Ok(LoraValue::Int(*n)),
        Expr::Float(f, _) => Ok(LoraValue::Float(*f)),
        Expr::String(s, _) => Ok(LoraValue::String(s.clone())),
        Expr::Bool(b, _) => Ok(LoraValue::Bool(*b)),
        Expr::Null(_) => Ok(LoraValue::Null),
        Expr::Parameter(name, _) => params
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("parameter `${name}` not supplied")),
        Expr::List(items, _) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(eval_expr(item, row, params)?);
            }
            Ok(LoraValue::List(out))
        }
        Expr::Property {
            expr: inner, key, ..
        } => {
            let base = eval_expr(inner, row, params)?;
            Ok(match base {
                LoraValue::Map(map) => map.get(key).cloned().unwrap_or(LoraValue::Null),
                LoraValue::Null => LoraValue::Null,
                _ => LoraValue::Null,
            })
        }
        Expr::Unary {
            op, expr: inner, ..
        } => {
            let v = eval_expr(inner, row, params)?;
            Ok(apply_unary(*op, v))
        }
        Expr::Binary { lhs, op, rhs, .. } => {
            // IsNull / IsNotNull only inspect the lhs operand.
            match op {
                BinaryOp::IsNull => {
                    let v = eval_expr(lhs, row, params)?;
                    return Ok(LoraValue::Bool(matches!(v, LoraValue::Null)));
                }
                BinaryOp::IsNotNull => {
                    let v = eval_expr(lhs, row, params)?;
                    return Ok(LoraValue::Bool(!matches!(v, LoraValue::Null)));
                }
                BinaryOp::And => {
                    let l = eval_expr(lhs, row, params)?;
                    if !l.is_truthy() {
                        return Ok(LoraValue::Bool(false));
                    }
                    let r = eval_expr(rhs, row, params)?;
                    return Ok(LoraValue::Bool(r.is_truthy()));
                }
                BinaryOp::Or => {
                    let l = eval_expr(lhs, row, params)?;
                    if l.is_truthy() {
                        return Ok(LoraValue::Bool(true));
                    }
                    let r = eval_expr(rhs, row, params)?;
                    return Ok(LoraValue::Bool(r.is_truthy()));
                }
                _ => {}
            }
            let l = eval_expr(lhs, row, params)?;
            let r = eval_expr(rhs, row, params)?;
            Ok(apply_binary(*op, l, r))
        }
        // Anything else is a hard error rather than a silent null so
        // we don't paper over genuinely unsupported SHOW expressions.
        other => Err(anyhow!(
            "expression form not supported in SHOW pipeline: {other:?}"
        )),
    }
}

fn apply_unary(op: UnaryOp, v: LoraValue) -> LoraValue {
    match op {
        UnaryOp::Not => match v {
            LoraValue::Null => LoraValue::Null,
            other => LoraValue::Bool(!other.is_truthy()),
        },
        UnaryOp::Neg => match v {
            LoraValue::Int(n) => LoraValue::Int(-n),
            LoraValue::Float(f) => LoraValue::Float(-f),
            _ => LoraValue::Null,
        },
        UnaryOp::Pos => v,
    }
}

fn apply_binary(op: BinaryOp, l: LoraValue, r: LoraValue) -> LoraValue {
    use BinaryOp::*;
    match op {
        Eq => LoraValue::Bool(values_equal(&l, &r)),
        Ne => LoraValue::Bool(!values_equal(&l, &r)),
        Lt | Gt | Le | Ge => match compare_values(&l, &r) {
            Ordering::Less => LoraValue::Bool(matches!(op, Lt | Le)),
            Ordering::Greater => LoraValue::Bool(matches!(op, Gt | Ge)),
            Ordering::Equal => LoraValue::Bool(matches!(op, Le | Ge)),
        },
        In => match r {
            LoraValue::List(items) => {
                LoraValue::Bool(items.iter().any(|item| values_equal(item, &l)))
            }
            LoraValue::Null => LoraValue::Null,
            _ => LoraValue::Bool(false),
        },
        StartsWith => string_pred(&l, &r, |a, b| a.starts_with(b)),
        EndsWith => string_pred(&l, &r, |a, b| a.ends_with(b)),
        Contains => string_pred(&l, &r, |a, b| a.contains(b)),
        Add | Sub | Mul | Div | Mod | Pow => numeric_op(op, l, r),
        Xor => LoraValue::Bool(l.is_truthy() ^ r.is_truthy()),
        // Short-circuited above; reach only via direct call with raw
        // values, in which case we fall back to standard truthiness.
        And => LoraValue::Bool(l.is_truthy() && r.is_truthy()),
        Or => LoraValue::Bool(l.is_truthy() || r.is_truthy()),
        IsNull | IsNotNull => LoraValue::Null, // handled in eval_expr
        RegexMatch => LoraValue::Null,
    }
}

fn string_pred(l: &LoraValue, r: &LoraValue, f: impl Fn(&str, &str) -> bool) -> LoraValue {
    match (l, r) {
        (LoraValue::String(a), LoraValue::String(b)) => LoraValue::Bool(f(a, b)),
        _ => LoraValue::Null,
    }
}

fn numeric_op(op: BinaryOp, l: LoraValue, r: LoraValue) -> LoraValue {
    use BinaryOp::*;
    let l = l.as_f64();
    let r = r.as_f64();
    match (l, r) {
        (Some(a), Some(b)) => LoraValue::Float(match op {
            Add => a + b,
            Sub => a - b,
            Mul => a * b,
            Div => a / b,
            Mod => a % b,
            Pow => a.powf(b),
            _ => return LoraValue::Null,
        }),
        _ => LoraValue::Null,
    }
}

fn values_equal(l: &LoraValue, r: &LoraValue) -> bool {
    match (l, r) {
        (LoraValue::Null, _) | (_, LoraValue::Null) => false,
        (LoraValue::Int(a), LoraValue::Int(b)) => a == b,
        (LoraValue::Float(a), LoraValue::Float(b)) => a == b,
        (LoraValue::Int(a), LoraValue::Float(b)) | (LoraValue::Float(b), LoraValue::Int(a)) => {
            (*a as f64) == *b
        }
        (LoraValue::String(a), LoraValue::String(b)) => a == b,
        (LoraValue::Bool(a), LoraValue::Bool(b)) => a == b,
        (LoraValue::List(a), LoraValue::List(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| values_equal(x, y))
        }
        _ => l == r,
    }
}

fn compare_values(l: &LoraValue, r: &LoraValue) -> Ordering {
    // Cypher's "null sorts last" rule.
    match (l, r) {
        (LoraValue::Null, LoraValue::Null) => Ordering::Equal,
        (LoraValue::Null, _) => Ordering::Greater,
        (_, LoraValue::Null) => Ordering::Less,
        (LoraValue::Int(a), LoraValue::Int(b)) => a.cmp(b),
        (LoraValue::Float(a), LoraValue::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (LoraValue::Int(a), LoraValue::Float(b)) => {
            (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal)
        }
        (LoraValue::Float(a), LoraValue::Int(b)) => {
            a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal)
        }
        (LoraValue::String(a), LoraValue::String(b)) => a.cmp(b),
        (LoraValue::Bool(a), LoraValue::Bool(b)) => a.cmp(b),
        (LoraValue::List(a), LoraValue::List(b)) => {
            for (x, y) in a.iter().zip(b.iter()) {
                let ord = compare_values(x, y);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.len().cmp(&b.len())
        }
        _ => Ordering::Equal,
    }
}

fn render_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Variable(v) => v.name.clone(),
        Expr::Property {
            expr: inner, key, ..
        } => {
            format!("{}.{}", render_expr_label(inner), key)
        }
        _ => "_expr".to_string(),
    }
}
