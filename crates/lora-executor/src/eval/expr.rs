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
use lora_analyzer::symbols::VarId;
use lora_analyzer::{LiteralValue, PatternSeek, ResolvedExpr, ResolvedMapSelector};
use lora_ast::{BinaryOp, ListPredicateKind};
use lora_store::GraphStorage;
use std::borrow::Cow;
use std::collections::BTreeMap;

use super::binops::{eval_binary, eval_comparison, eval_in, eval_unary, value_eq};
use super::errors::{clear_eval_error, take_eval_error};
use super::functions::eval_function;

/// The message [`eval_expr_result`] fails with when an expression stopped
/// because the query's deadline passed or the query was cancelled.
/// [`crate::ExecutorError`] maps it back to `QueryTimeout`.
pub const DEADLINE_EXCEEDED: &str = "query deadline exceeded";

use crate::cancel::eval_deadline_hit;

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
            built_list(items.iter().map(|e| eval_expr(e, row, ctx)).collect())
        }

        ResolvedExpr::Map(items) => {
            let mut map = BTreeMap::new();
            for (k, v) in items {
                map.insert(k.clone(), eval_expr(v, row, ctx));
            }
            built_map(map)
        }

        ResolvedExpr::Property { expr, property } => {
            let base = eval_ref(expr, row, ctx);
            eval_property(&base, property, ctx)
        }

        ResolvedExpr::Binary {
            lhs,
            op: BinaryOp::In,
            rhs,
        } => {
            let l = eval_expr(lhs, row, ctx);
            let r = eval_ref(rhs, row, ctx);
            eval_in(&l, &r)
        }

        // `AND` / `OR` short-circuit: a left side that decides the result
        // (`false AND …`, `true OR …`) leaves the right side unevaluated,
        // so its errors cannot fail the query and a guard such as
        // `type.of(x) = 'DATE' AND x >= date(…)` protects the comparison.
        // A `null` left side decides nothing: three-valued logic needs the
        // right side (`null AND false` is `false`).
        ResolvedExpr::Binary {
            lhs,
            op: op @ (BinaryOp::And | BinaryOp::Or),
            rhs,
        } => {
            let l = eval_expr(lhs, row, ctx);
            let decides = match (&l, op) {
                (LoraValue::Null, _) => false,
                (l, BinaryOp::And) => !l.is_truthy(),
                (l, _) => l.is_truthy(),
            };
            if decides {
                return LoraValue::Bool(matches!(op, BinaryOp::Or));
            }
            let r = eval_expr(rhs, row, ctx);
            eval_binary(op, l, r)
        }

        ResolvedExpr::Binary {
            lhs,
            op:
                op @ (BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Gt
                | BinaryOp::Le
                | BinaryOp::Ge),
            rhs,
        } if reads_variable(lhs) || reads_variable(rhs) => compare_in_place(lhs, op, rhs, row, ctx),

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
        } => eval_call(*function, args, row, ctx),

        ResolvedExpr::Parameter(name) => ctx.params.get(name).cloned().unwrap_or(LoraValue::Null),

        ResolvedExpr::ListPredicate {
            kind,
            variable,
            list,
            predicate,
        } => {
            match list_items(list, row, ctx) {
                ListItems::Items(items) => {
                    let mut total = items.len();
                    // Clone the row once, then rebind `variable` per iteration
                    // instead of cloning the entire row + item per element.
                    let mut inner_row = row.clone();
                    let mut count = 0usize;
                    for item in items {
                        if eval_deadline_hit() {
                            return LoraValue::Null;
                        }
                        inner_row.insert_inline(*variable, item);
                        if eval_expr(predicate, &inner_row, ctx).is_truthy() {
                            count += 1;
                            // One match settles `any` and `none`, a second
                            // settles `single`: the rest of a long list is
                            // not walked.
                            match kind {
                                ListPredicateKind::Any | ListPredicateKind::None => break,
                                ListPredicateKind::Single if count > 1 => break,
                                _ => {}
                            }
                        } else if matches!(kind, ListPredicateKind::All) {
                            // One miss settles `all`.
                            total = usize::MAX;
                            break;
                        }
                    }
                    match kind {
                        ListPredicateKind::Any => LoraValue::Bool(count > 0),
                        ListPredicateKind::All => LoraValue::Bool(count == total),
                        ListPredicateKind::None => LoraValue::Bool(count == 0),
                        ListPredicateKind::Single => LoraValue::Bool(count == 1),
                    }
                }
                ListItems::Null => LoraValue::Null,
                ListItems::NotAList => LoraValue::Bool(false),
            }
        }

        ResolvedExpr::ListComprehension {
            variable,
            list,
            filter,
            map_expr,
        } => {
            match list_items(list, row, ctx) {
                ListItems::Items(items) => {
                    let mut result = Vec::with_capacity(items.len());
                    let mut inner_row = row.clone();
                    for item in items {
                        if eval_deadline_hit() {
                            return LoraValue::Null;
                        }
                        // When no map_expr, we need the item after filter —
                        // stash it in the row binding and read it back if kept.
                        inner_row.insert_inline(*variable, item);
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
                    built_list(result)
                }
                ListItems::Null | ListItems::NotAList => LoraValue::Null,
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
            match list_items(list, row, ctx) {
                ListItems::Items(items) => {
                    // One cloned row reused across all iterations; accumulator
                    // and variable rebind per item.
                    let mut inner_row = row.clone();
                    let mut acc = init_val;
                    for item in items {
                        if eval_deadline_hit() {
                            return LoraValue::Null;
                        }
                        inner_row.insert_inline(*accumulator, acc);
                        inner_row.insert_inline(*variable, item);
                        acc = eval_expr(expr, &inner_row, ctx);
                    }
                    acc
                }
                ListItems::Null | ListItems::NotAList => LoraValue::Null,
            }
        }

        ResolvedExpr::Index { expr, index } => {
            let base = eval_ref(expr, row, ctx);
            let idx = eval_expr(index, row, ctx);
            match (&*base, idx) {
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
                // `n[k]` reads the property `n.k` would.
                (LoraValue::Node(_) | LoraValue::Relationship(_), LoraValue::String(key)) => {
                    eval_property(&base, &key, ctx)
                }
                _ => LoraValue::Null,
            }
        }

        ResolvedExpr::Slice { expr, from, to } => {
            let base = eval_ref(expr, row, ctx);
            match &*base {
                LoraValue::List(items) => {
                    let len = items.len() as i64;
                    // A negative bound counts from the end (`[..-1]` drops the
                    // last element); a null bound makes the slice null.
                    let bound = |e: &Option<Box<ResolvedExpr>>, default: i64| match e {
                        None => Some(default),
                        Some(e) => match eval_expr(e, row, ctx) {
                            LoraValue::Null => None,
                            v => {
                                let b = v.as_i64().unwrap_or(default);
                                Some(if b < 0 { len.saturating_add(b) } else { b }.clamp(0, len))
                            }
                        },
                    };
                    let (Some(start), Some(end)) = (bound(from, 0), bound(to, len)) else {
                        return LoraValue::Null;
                    };
                    let (start, end) = (start as usize, end as usize);
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
            let base_val = eval_ref(base, row, ctx);
            // A projection of null is null (openCypher): an OPTIONAL MATCH
            // miss or `head([])` projects as nothing, not as a map of nulls.
            if matches!(&*base_val, LoraValue::Null) {
                return LoraValue::Null;
            }
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
                        match &*base_val {
                            LoraValue::Node(id) => {
                                ctx.storage.with_node(*id, |node| {
                                    for (k, v) in &node.properties() {
                                        result.insert(k.to_string(), LoraValue::from(v));
                                    }
                                });
                            }
                            LoraValue::Relationship(id) => {
                                ctx.storage.with_relationship(*id, |rel| {
                                    for (k, v) in &rel.properties() {
                                        result.insert(k.to_string(), LoraValue::from(v));
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

            built_map(result)
        }

        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => match case_branch(
            input.as_deref(),
            alternatives,
            else_expr.as_deref(),
            row,
            ctx,
        ) {
            Some(branch) => eval_expr(branch, row, ctx),
            None => LoraValue::Null,
        },

        ResolvedExpr::ExistsSubquery {
            pattern,
            where_,
            reads,
            seeks,
        } => eval_exists_subquery(pattern, where_.as_deref(), reads, seeks, row, ctx),

        ResolvedExpr::PatternComprehension {
            pattern,
            where_,
            map_expr,
            reads,
            seeks,
        } => {
            eval_pattern_comprehension(pattern, where_.as_deref(), map_expr, reads, seeks, row, ctx)
        }
    }
}

/// `items` as a new list value, or null with an error when the list would
/// break a value limit (see [`crate::limits`]).
#[inline(never)]
pub(super) fn built_list(items: Vec<LoraValue>) -> LoraValue {
    match crate::limits::check_built_list(&items) {
        Ok(()) => LoraValue::List(items),
        Err(message) => {
            super::errors::set_eval_error(message);
            LoraValue::Null
        }
    }
}

/// [`built_list`] for a map.
#[inline(never)]
pub(super) fn built_map(map: BTreeMap<String, LoraValue>) -> LoraValue {
    match crate::limits::check_built(map.values()) {
        Ok(()) => LoraValue::Map(map),
        Err(message) => {
            super::errors::set_eval_error(message);
            LoraValue::Null
        }
    }
}

/// The branch a `CASE` takes, or `None` when no alternative matches and
/// there is no `ELSE`.
fn case_branch<'e, S: GraphStorage>(
    input: Option<&ResolvedExpr>,
    alternatives: &'e [(ResolvedExpr, ResolvedExpr)],
    else_expr: Option<&'e ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Option<&'e ResolvedExpr> {
    let input = input.map(|input| eval_ref(input, row, ctx));
    for (when, then) in alternatives {
        let when = eval_ref(when, row, ctx);
        let taken = match &input {
            Some(input) => value_eq(input, &when),
            None => when.is_truthy(),
        };
        if taken {
            return Some(then);
        }
    }
    else_expr
}

/// Where an expression's value comes from: a variable the row holds (or a
/// parameter, with no variable), whose value can be read or shared in
/// place, or a value computed for this call.
enum Source<'r> {
    Var(Option<VarId>, &'r LoraValue),
    Value(LoraValue),
}

/// `expr`'s [`Source`]. A bare variable, and a `coalesce` or `CASE` that
/// hands one back unchanged (`coalesce(items, [])`), yield the variable:
/// the callers below then read or share a carried list instead of copying
/// it once per row.
#[inline(never)]
fn eval_source<'r, S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &'r Row,
    ctx: &EvalContext<'r, S>,
) -> Source<'r> {
    match expr {
        ResolvedExpr::Variable(var) => match row.get(*var) {
            Some(value) => Source::Var(Some(*var), value),
            None => Source::Value(LoraValue::Null),
        },
        // A parameter is read in place too: `x IN $list` per row must not
        // copy the list each time.
        ResolvedExpr::Parameter(name) => Source::Var(None, ctx.params.get(name).unwrap_or(&NULL)),
        ResolvedExpr::Function { function, args, .. } if is_coalesce(*function) => {
            let mut args = args.iter();
            let mut first = Source::Value(LoraValue::Null);
            for arg in args.by_ref() {
                let source = eval_source(arg, row, ctx);
                let is_null = match &source {
                    Source::Var(_, value) => matches!(value, LoraValue::Null),
                    Source::Value(value) => matches!(value, LoraValue::Null),
                };
                if !is_null {
                    first = source;
                    break;
                }
            }
            // The arguments after the first non-null one decide nothing,
            // but an error in one still fails the query, as it always has.
            for arg in args {
                if !matches!(arg, ResolvedExpr::Variable(_) | ResolvedExpr::Literal(_)) {
                    eval_expr(arg, row, ctx);
                }
            }
            first
        }
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => match case_branch(
            input.as_deref(),
            alternatives,
            else_expr.as_deref(),
            row,
            ctx,
        ) {
            Some(branch) => eval_source(branch, row, ctx),
            None => Source::Value(LoraValue::Null),
        },
        _ => Source::Value(eval_expr(expr, row, ctx)),
    }
}

fn is_coalesce(function: lora_analyzer::FunctionId) -> bool {
    matches!(function.name(), "value.coalesce" | "value.first_non_null")
}

/// `expr`'s value, borrowed from the row when it is a variable's (see
/// [`eval_source`]). Reading one element, slice, key or property of a
/// carried list or map, comparing it, or testing `x IN list`, must not copy
/// the whole value: a pattern subquery evaluates its WHERE once per
/// candidate.
#[inline]
pub(crate) fn eval_ref<'r, S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &'r Row,
    ctx: &EvalContext<'r, S>,
) -> Cow<'r, LoraValue> {
    match expr {
        ResolvedExpr::Variable(var) => Cow::Borrowed(row.get(*var).unwrap_or(&NULL)),
        ResolvedExpr::Parameter(name) => Cow::Borrowed(ctx.params.get(name).unwrap_or(&NULL)),
        _ if passes_through(expr) => match eval_source(expr, row, ctx) {
            Source::Var(_, value) => Cow::Borrowed(value),
            Source::Value(value) => Cow::Owned(value),
        },
        _ => Cow::Owned(eval_expr(expr, row, ctx)),
    }
}

static NULL: LoraValue = LoraValue::Null;

/// Whether `expr`'s value can be a variable's, read in place.
#[inline]
fn reads_variable(expr: &ResolvedExpr) -> bool {
    matches!(expr, ResolvedExpr::Variable(_)) || passes_through(expr)
}

/// A comparison with an operand that is a variable's value: a carried list
/// is compared where it sits, not copied first.
#[inline(never)]
fn compare_in_place<S: GraphStorage>(
    lhs: &ResolvedExpr,
    op: &BinaryOp,
    rhs: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    let l = eval_ref(lhs, row, ctx);
    let r = eval_ref(rhs, row, ctx);
    eval_comparison(op, &l, &r)
}

/// Whether `expr` is a `coalesce` or a `CASE`, which can hand a variable's
/// value back unchanged. Everything else, the hot path of every read, is
/// evaluated straight.
#[inline]
pub fn passes_through(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Case { .. } => true,
        ResolvedExpr::Function { function, .. } => is_coalesce(*function),
        _ => false,
    }
}

/// What a projection item evaluates to: the row's own value for a variable,
/// which the projected row can share, or a new value.
pub enum Projected {
    Var(VarId),
    Value(LoraValue),
}

/// [`eval_expr_result`] for a projection item that [`passes_through`],
/// naming the variable instead of copying its value when the item hands one
/// back unchanged.
#[inline(never)]
pub fn eval_projected_result<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Result<Projected, String> {
    clear_eval_error();
    let projected = match eval_source(expr, row, ctx) {
        Source::Var(Some(var), _) => Projected::Var(var),
        Source::Var(None, value) => Projected::Value(value.clone()),
        Source::Value(value) => Projected::Value(value),
    };
    if crate::cancel::eval_tripped() {
        clear_eval_error();
        return Err(DEADLINE_EXCEEDED.to_string());
    }
    match take_eval_error() {
        Some(err) => Err(err),
        None => Ok(projected),
    }
}

/// The elements of a list expression, for the constructs that bind each to
/// a variable. A carried list is walked in place, its elements cloned one
/// at a time; a list built for this call is consumed.
enum ListItems<'r> {
    Items(ItemIter<'r>),
    Null,
    NotAList,
}

enum ItemIter<'r> {
    Borrowed(std::slice::Iter<'r, LoraValue>),
    Owned(std::vec::IntoIter<LoraValue>),
}

impl Iterator for ItemIter<'_> {
    type Item = LoraValue;

    #[inline]
    fn next(&mut self) -> Option<LoraValue> {
        match self {
            ItemIter::Borrowed(items) => items.next().cloned(),
            ItemIter::Owned(items) => items.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            ItemIter::Borrowed(items) => items.size_hint(),
            ItemIter::Owned(items) => items.size_hint(),
        }
    }
}

impl ExactSizeIterator for ItemIter<'_> {}

#[inline(never)]
fn list_items<'r, S: GraphStorage>(
    list: &ResolvedExpr,
    row: &'r Row,
    ctx: &EvalContext<'r, S>,
) -> ListItems<'r> {
    match eval_source(list, row, ctx) {
        Source::Var(_, LoraValue::List(items)) => {
            ListItems::Items(ItemIter::Borrowed(items.iter()))
        }
        Source::Value(LoraValue::List(items)) => {
            ListItems::Items(ItemIter::Owned(items.into_iter()))
        }
        Source::Var(_, LoraValue::Null) | Source::Value(LoraValue::Null) => ListItems::Null,
        _ => ListItems::NotAList,
    }
}

/// A function call. A lone argument that is a variable's value
/// (`size(big)`, `head(coalesce(list, []))`) is read in place instead of
/// copied per call.
fn eval_call<S: GraphStorage>(
    function: lora_analyzer::FunctionId,
    args: &[ResolvedExpr],
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    if let [arg] = args {
        let value = eval_ref(arg, row, ctx);
        return eval_function(function, std::slice::from_ref(&*value), ctx);
    }
    let args: Vec<LoraValue> = args.iter().map(|a| eval_expr(a, row, ctx)).collect();
    eval_function(function, &args, ctx)
}

fn eval_exists_subquery<S: GraphStorage>(
    pattern: &lora_analyzer::ResolvedPattern,
    where_: Option<&ResolvedExpr>,
    reads: &[VarId],
    seeks: &[PatternSeek],
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    if pattern.parts.is_empty() {
        return LoraValue::Bool(exists_candidate_matches(where_, row, ctx));
    }

    // Process each pattern part. The final part short-circuits because EXISTS
    // only cares whether at least one complete binding survives the WHERE.
    // Match on a row holding only what the subquery reads: every candidate
    // clones it, and the outer row may carry large values (collected lists).
    let mut candidate_rows = vec![scoped_row(row, reads)];
    let last_part = pattern.parts.len() - 1;

    for (part_idx, part) in pattern.parts.iter().enumerate() {
        let mut next_rows = Vec::new();
        let mut found = false;
        for current_row in &candidate_rows {
            if eval_deadline_hit() {
                return LoraValue::Null;
            }
            match_pattern_part(part, current_row, seeks, ctx, &mut |rows| {
                if part_idx == last_part {
                    found = rows
                        .iter()
                        .any(|r| exists_candidate_matches(where_, r, ctx));
                    !found
                } else {
                    next_rows.extend(rows);
                    true
                }
            });
            if found {
                return LoraValue::Bool(true);
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
        .map(|where_expr| !eval_deadline_hit() && eval_expr(where_expr, row, ctx).is_truthy())
        .unwrap_or(true)
}

fn eval_pattern_comprehension<S: GraphStorage>(
    pattern: &lora_analyzer::ResolvedPattern,
    where_: Option<&ResolvedExpr>,
    map_expr: &ResolvedExpr,
    reads: &[VarId],
    seeks: &[PatternSeek],
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> LoraValue {
    // Reuse the same pattern matching as EXISTS, on a row holding only what
    // the comprehension reads: every candidate and every expansion clones it,
    // and the outer row may carry large values (the lists earlier CALLs
    // collected), which made the cost proportional to them.
    let mut candidate_rows = vec![scoped_row(row, reads)];

    for part in &pattern.parts {
        let mut next_rows = Vec::new();
        for current_row in &candidate_rows {
            if eval_deadline_hit() {
                return LoraValue::Null;
            }
            match_pattern_part(part, current_row, seeks, ctx, &mut |rows| {
                next_rows.extend(rows);
                true
            });
        }
        candidate_rows = next_rows;
    }

    if let Some(where_expr) = where_ {
        candidate_rows
            .retain(|r| !eval_deadline_hit() && eval_expr(where_expr, r, ctx).is_truthy());
    }
    if crate::cancel::eval_tripped() {
        return LoraValue::Null;
    }

    // Map each matched row through the map expression
    let mut out = Vec::with_capacity(candidate_rows.len());
    for r in &candidate_rows {
        if eval_deadline_hit() {
            return LoraValue::Null;
        }
        out.push(eval_expr(map_expr, r, ctx));
    }
    built_list(out)
}

/// A copy of `row` holding only `reads`, the variables a pattern subquery
/// uses (computed once by the analyzer). Every candidate and every expansion
/// step clones this row, so values the subquery never reads (a list collected
/// by an earlier CALL) must not ride along. A row holding nothing but what
/// the subquery reads is cloned as is: scoping it would save nothing.
fn scoped_row(row: &Row, reads: &[VarId]) -> Row {
    let read_bound = reads.iter().filter(|&&var| row.contains_key(var)).count();
    if read_bound == row.len() {
        return row.clone();
    }
    let mut scoped = Row::new();
    for &var in reads {
        if let Some(value) = row.get(var) {
            scoped.insert(var, value.clone());
        }
    }
    scoped
}

/// What a row says about a variable a pattern subquery names.
enum Binding<T> {
    /// Nothing yet: the pattern binds it.
    Free,
    /// An entity the pattern must match on.
    Bound(T),
    /// `null` (an OPTIONAL MATCH that found nothing, `head([])`), or a
    /// value that is no node or relationship: the pattern matches nothing,
    /// as in openCypher. It must not read as free, which would match the
    /// pattern for every node there is.
    Nothing,
}

fn node_binding(row: &Row, var: Option<VarId>) -> Binding<lora_store::NodeId> {
    match var.and_then(|var| row.get(var)) {
        None => Binding::Free,
        Some(LoraValue::Node(id)) => Binding::Bound(*id),
        Some(_) => Binding::Nothing,
    }
}

fn relationship_binding(row: &Row, var: Option<VarId>) -> Binding<lora_store::RelationshipId> {
    match var.and_then(|var| row.get(var)) {
        None => Binding::Free,
        Some(LoraValue::Relationship(id)) => Binding::Bound(*id),
        Some(_) => Binding::Nothing,
    }
}

/// As [`relationship_binding`], for the list a variable-length
/// relationship binds.
fn relationship_list_binding(
    row: &Row,
    var: Option<VarId>,
) -> Binding<Vec<lora_store::RelationshipId>> {
    match var.and_then(|var| row.get(var)) {
        None => Binding::Free,
        Some(LoraValue::List(items)) => items
            .iter()
            .map(|item| match item {
                LoraValue::Relationship(id) => Some(*id),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map_or(Binding::Nothing, Binding::Bound),
        Some(_) => Binding::Nothing,
    }
}

/// One hop of a pattern subquery's chain, in the order it is walked.
struct WalkStep<'q> {
    /// The variable of the node the hop starts from.
    src: Option<VarId>,
    rel: &'q lora_analyzer::ResolvedRel,
    /// `rel`'s direction as seen from `src`.
    direction: lora_ast::Direction,
    node: &'q lora_analyzer::ResolvedNode,
}

/// The order to walk `head` and `chain` in for `row`: as written, or from
/// the last node when that one is the cheaper place to start (the row binds
/// it, or a property pins it, and the head has neither), as `MATCH` plans
/// it. `[(g:G)<-[:LIKES]-(p:P) WHERE p.key = $key | g]` then seeks `p` and
/// follows its relationships instead of scanning `:G`.
fn chain_walk<'q>(
    head: &'q lora_analyzer::ResolvedNode,
    chain: &'q [lora_analyzer::ResolvedChain],
    reversible: bool,
    row: &Row,
    seeks: &[PatternSeek],
) -> (&'q lora_analyzer::ResolvedNode, Vec<WalkStep<'q>>) {
    let start_cost = |node: &lora_analyzer::ResolvedNode| match node.var {
        Some(var) if row.contains_key(var) => 0,
        var if node.properties.is_some() || seeks.iter().any(|seek| Some(seek.var) == var) => 1,
        _ => 2,
    };
    let from_tail = reversible
        && chain.last().is_some_and(|last| {
            start_cost(&last.node) < start_cost(head) && chain_reads_no_own_variable(head, chain)
        });

    if !from_tail {
        let mut src = head.var;
        let steps = chain
            .iter()
            .map(|step| {
                let walk = WalkStep {
                    src,
                    rel: &step.rel,
                    direction: step.rel.direction,
                    node: &step.node,
                };
                src = step.node.var;
                walk
            })
            .collect();
        return (head, steps);
    }

    let node_at = |i: usize| if i == 0 { head } else { &chain[i - 1].node };
    let steps = (0..chain.len())
        .rev()
        .map(|i| WalkStep {
            src: node_at(i + 1).var,
            rel: &chain[i].rel,
            direction: match chain[i].rel.direction {
                lora_ast::Direction::Left => lora_ast::Direction::Right,
                lora_ast::Direction::Right => lora_ast::Direction::Left,
                other => other,
            },
            node: node_at(i),
        })
        .collect();
    (node_at(chain.len()), steps)
}

/// Whether a chain can be walked from either end: no variable-length hop
/// (its list is bound in traversal order), and no property map reading a
/// variable of the chain itself, which is only bound once the walk in
/// written order reaches it.
fn chain_reads_no_own_variable(
    head: &lora_analyzer::ResolvedNode,
    chain: &[lora_analyzer::ResolvedChain],
) -> bool {
    if chain.iter().any(|step| step.rel.range.is_some()) {
        return false;
    }
    let maps = || {
        std::iter::once(&head.properties)
            .chain(
                chain
                    .iter()
                    .flat_map(|step| [&step.rel.properties, &step.node.properties]),
            )
            .flatten()
    };
    if maps().next().is_none() {
        return true;
    }
    let mut reads = std::collections::BTreeSet::new();
    maps().for_each(|map| map.collect_vars(&mut reads));
    let mut own = head.var.iter().chain(
        chain
            .iter()
            .flat_map(|step| step.rel.var.iter().chain(step.node.var.iter())),
    );
    !own.any(|var| reads.contains(var))
}

/// Match one part of a pattern subquery from `row`, handing `sink` the
/// complete matches found from each starting node in turn. `sink` returns
/// whether to go on: EXISTS stops at the first match that passes its WHERE.
fn match_pattern_part<S: GraphStorage>(
    part: &lora_analyzer::ResolvedPatternPart,
    row: &Row,
    seeks: &[PatternSeek],
    ctx: &EvalContext<'_, S>,
    sink: &mut dyn FnMut(Vec<Row>) -> bool,
) {
    use lora_analyzer::ResolvedPatternElement;

    let (head, chain, reversible) = match &part.element {
        ResolvedPatternElement::Node {
            var,
            labels,
            properties,
        } => {
            sink(match_node_pattern(
                *var, labels, properties, row, seeks, ctx,
            ));
            return;
        }
        ResolvedPatternElement::NodeChain { head, chain } => (head, chain, true),
        ResolvedPatternElement::ShortestPath { head, chain, .. } => (head, chain, false),
    };

    let (start, steps) = chain_walk(head, chain, reversible, row, seeks);
    let start_rows =
        match_node_pattern(start.var, &start.labels, &start.properties, row, seeks, ctx);
    for start_row in start_rows {
        let mut frontier = vec![start_row];
        for step in &steps {
            let mut step_rows = Vec::new();
            for fr in &frontier {
                expand_walk_step(step, fr, ctx, &mut step_rows);
            }
            frontier = step_rows;
            if frontier.is_empty() {
                break;
            }
        }
        if !sink(frontier) {
            return;
        }
    }
}

/// Extend `row` over one hop: every relationship of the step's types and
/// direction from the node `row` binds the hop's source to, ending on a node
/// that matches the step's node pattern. A destination or relationship the
/// row already binds (from the outer row, or earlier in this pattern)
/// restricts the hop to it; one bound to `null` leaves no match.
fn expand_walk_step<S: GraphStorage>(
    step: &WalkStep<'_>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
    out: &mut Vec<Row>,
) {
    let Binding::Bound(src) = node_binding(row, step.src) else {
        return;
    };
    let dst_bound = match node_binding(row, step.node.var) {
        Binding::Free => None,
        Binding::Bound(id) => Some(id),
        Binding::Nothing => return,
    };

    if let Some(range) = &step.rel.range {
        let rels_bound = match relationship_list_binding(row, step.rel.var) {
            Binding::Free => None,
            Binding::Bound(ids) => Some(ids),
            Binding::Nothing => return,
        };
        let (min_hops, max_hops) = crate::executor::resolve_range(range);
        let walk = VarLengthWalk {
            step,
            row,
            ctx,
            min_hops,
            max_hops,
            dst_bound,
            rels_bound,
        };
        walk.walk(src, out);
        return;
    }

    let rel_bound = match relationship_binding(row, step.rel.var) {
        Binding::Free => None,
        Binding::Bound(id) => Some(id),
        Binding::Nothing => return,
    };
    let _ = ctx.storage.try_for_each_expand_id(
        src,
        step.direction,
        &step.rel.types,
        |rel_id, dst_id| {
            if eval_deadline_hit() {
                return Err(());
            }
            if dst_bound.is_some_and(|bound| bound != dst_id)
                || rel_bound.is_some_and(|bound| bound != rel_id)
            {
                return Ok::<(), ()>(());
            }
            let matched = ctx
                .storage
                .with_node(dst_id, |dst| {
                    crate::executor::node_matches_label_groups(dst.labels(), &step.node.labels)
                        && node_matches_properties(
                            dst.properties(),
                            &step.node.properties,
                            row,
                            ctx,
                        )
                })
                .unwrap_or(false);
            if !matched || !rel_matches_properties(rel_id, &step.rel.properties, row, ctx) {
                return Ok(());
            }
            let mut r = row.clone();
            if let Some(rv) = step.rel.var {
                r.insert(rv, LoraValue::Relationship(rel_id));
            }
            if let Some(nv) = step.node.var {
                r.insert(nv, LoraValue::Node(dst_id));
            }
            out.push(r);
            Ok(())
        },
    );
}

/// Rows binding the node pattern `(var:labels {properties})` from `row`.
fn match_node_pattern<S: GraphStorage>(
    var: Option<VarId>,
    labels: &[Vec<String>],
    properties: &Option<ResolvedExpr>,
    row: &Row,
    seeks: &[PatternSeek],
    ctx: &EvalContext<'_, S>,
) -> Vec<Row> {
    let node_matches = |id: lora_store::NodeId| {
        ctx.storage
            .with_node(id, |n| {
                crate::executor::node_matches_label_groups(n.labels(), labels)
                    && node_matches_properties(n.properties(), properties, row, ctx)
            })
            .unwrap_or(false)
    };

    // If the variable is already bound in the row, check the existing binding
    // without cloning the whole record.
    match node_binding(row, var) {
        Binding::Bound(id) if node_matches(id) => return vec![row.clone()],
        Binding::Bound(_) | Binding::Nothing => return Vec::new(),
        Binding::Free => {}
    }

    // Candidate discovery only needs IDs — defer record lookup until after
    // label/property filtering so we can borrow once per matching candidate.
    // An inline property map (`(v:Person {subject: $s})`) seeks one of its
    // properties, as `MATCH` does, instead of scanning the label: a pattern
    // subquery runs once per outer row. So does a WHERE conjunct that pins
    // one (`WHERE v.subject = $s`), which the analyzer recorded in `seeks`;
    // of several, the one that leaves the fewest nodes.
    let inline_seek =
        properties
            .as_ref()
            .and_then(|props_expr| match eval_expr(props_expr, row, ctx) {
                LoraValue::Map(exp) => exp.iter().next().map(|(key, value)| {
                    crate::executor::indexed_node_property_candidates(
                        ctx.storage,
                        labels,
                        key,
                        value,
                    )
                    .ids
                }),
                _ => None,
            });
    let seek = inline_seek.or_else(|| {
        seeks
            .iter()
            .filter(|seek| Some(seek.var) == var)
            .filter_map(|seek| where_seek_candidates(seek, labels, row, ctx))
            .min_by_key(|ids| ids.len())
            .map(|mut ids| {
                // The order a label scan yields them in.
                ids.sort_unstable();
                ids.dedup();
                ids
            })
    });
    let first_label = labels.iter().flat_map(|g| g.iter()).next();
    let candidate_ids: Vec<lora_store::NodeId> = match (seek, first_label) {
        (Some(ids), _) => ids,
        (None, Some(label)) => ctx.storage.node_ids_by_label(label),
        (None, None) => ctx.storage.all_node_ids(),
    };

    let mut out = Vec::new();
    for id in candidate_ids {
        if eval_deadline_hit() {
            break;
        }
        if !node_matches(id) {
            continue;
        }
        let mut r = row.clone();
        if let Some(v) = var {
            r.insert(v, LoraValue::Node(id));
        }
        out.push(r);
    }
    out
}

/// The nodes a WHERE conjunct of a pattern subquery can leave for the node
/// it pins, found through the property's index. A superset is fine: the
/// WHERE still judges every match. `None` when this conjunct cannot narrow the scan (no index
/// answers it, or its value is not of a kind an index holds).
fn where_seek_candidates<S: GraphStorage>(
    seek: &PatternSeek,
    labels: &[Vec<String>],
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Option<Vec<lora_store::NodeId>> {
    use lora_analyzer::PatternSeekKind;

    let value = eval_expr(&seek.value, row, ctx);
    // `x = null`, `x IN null` and `x STARTS WITH null` are never true.
    if matches!(value, LoraValue::Null) {
        return Some(Vec::new());
    }
    let ids = match seek.kind {
        PatternSeekKind::Equals => {
            crate::executor::indexed_node_property_candidates(
                ctx.storage,
                labels,
                &seek.key,
                &value,
            )
            .ids
        }
        PatternSeekKind::In => {
            let LoraValue::List(_) = &value else {
                return None;
            };
            crate::executor::property_scan_candidates(ctx.storage, labels, &seek.key, &value, true)
                .ids
        }
        PatternSeekKind::StartsWith => {
            let LoraValue::String(_) = &value else {
                return None;
            };
            let [group] = labels else {
                return None;
            };
            let [label] = group.as_slice() else {
                return None;
            };
            let prefix_end = lora_analyzer::FunctionId::builtin("string.prefix_end")?;
            let hi = match eval_function(prefix_end, std::slice::from_ref(&value), ctx) {
                LoraValue::Null => None,
                end => Some(crate::value::lora_value_to_property(end).ok()?),
            };
            let lo = crate::value::lora_value_to_property(value).ok()?;
            ctx.storage
                .node_range_candidates(label, &seek.key, Some(&lo), hi.as_ref())?
        }
    };
    Some(ids)
}

/// One variable-length hop `-[r:T*min..max {k: v}]->(dst)` of a pattern
/// subquery being walked from a node: every path of `min..=max` hops that
/// repeats no relationship, whose relationships all carry the map's
/// properties, and that ends on a node matching the step's node pattern (and
/// on the node or relationship list the row already binds, if any). Each
/// match extends `row` with `r` bound to the path's relationships.
/// Longest pattern [`VarLengthWalk`] walks by recursion; one hop costs a
/// few hundred bytes of stack.
const RECURSIVE_WALK_MAX_HOPS: u64 = 32;

struct VarLengthWalk<'q, 'c, S: GraphStorage> {
    step: &'q WalkStep<'q>,
    row: &'q Row,
    ctx: &'q EvalContext<'c, S>,
    min_hops: u64,
    max_hops: u64,
    dst_bound: Option<lora_store::NodeId>,
    rels_bound: Option<Vec<lora_store::RelationshipId>>,
}

impl<S: GraphStorage> VarLengthWalk<'_, '_, S> {
    /// Depth-first walk from `start`, emitting matches in path order. The
    /// walk keeps its own stack (one frame per hop of the current path)
    /// instead of recursing, so a long path cannot overflow the thread's.
    fn walk(&self, start: lora_store::NodeId, out: &mut Vec<Row>) {
        // Short patterns, the common case, recurse: a few frames deep that
        // is cheaper than keeping a stack. Longer ones keep their own.
        if self.max_hops <= RECURSIVE_WALK_MAX_HOPS {
            self.visit(start, &mut Vec::new(), out);
            return;
        }
        let depth = (self.max_hops.min(64) + 1) as usize;
        let mut path: Vec<lora_store::RelationshipId> = Vec::with_capacity(depth);
        // Per hop of `path`: the untried edges of the node it reached.
        let mut frames: Vec<(Vec<(lora_store::RelationshipId, lora_store::NodeId)>, usize)> =
            Vec::with_capacity(depth);
        self.enter(start, &path, &mut frames, out);
        while let Some((edges, next)) = frames.last_mut() {
            let Some(&(rel_id, dst_id)) = edges.get(*next) else {
                frames.pop();
                // The root frame has no relationship on the path.
                if !frames.is_empty() {
                    path.pop();
                }
                continue;
            };
            *next += 1;
            if path.contains(&rel_id)
                || !rel_matches_properties(rel_id, &self.step.rel.properties, self.row, self.ctx)
            {
                continue;
            }
            path.push(rel_id);
            if !self.enter(dst_id, &path, &mut frames, out) {
                path.pop();
            }
        }
    }

    /// The recursive walk, for patterns of at most
    /// [`RECURSIVE_WALK_MAX_HOPS`] hops.
    fn visit(
        &self,
        node: lora_store::NodeId,
        path: &mut Vec<lora_store::RelationshipId>,
        out: &mut Vec<Row>,
    ) {
        let hops = path.len() as u64;
        if hops >= self.min_hops {
            self.emit(node, path, out);
        }
        if hops >= self.max_hops || eval_deadline_hit() {
            return;
        }
        let mut next = Vec::new();
        let _ = self.ctx.storage.try_for_each_expand_id(
            node,
            self.step.direction,
            &self.step.rel.types,
            |rel_id, dst_id| {
                next.push((rel_id, dst_id));
                Ok::<(), ()>(())
            },
        );
        for (rel_id, dst_id) in next {
            if path.contains(&rel_id)
                || !rel_matches_properties(rel_id, &self.step.rel.properties, self.row, self.ctx)
            {
                continue;
            }
            path.push(rel_id);
            self.visit(dst_id, path, out);
            path.pop();
        }
    }

    /// Visit `node` at the end of `path`: emit it if it matches, and push a
    /// frame of its edges when the path may grow. Returns whether it did.
    fn enter(
        &self,
        node: lora_store::NodeId,
        path: &[lora_store::RelationshipId],
        frames: &mut Vec<(Vec<(lora_store::RelationshipId, lora_store::NodeId)>, usize)>,
        out: &mut Vec<Row>,
    ) -> bool {
        let hops = path.len() as u64;
        if hops >= self.min_hops {
            self.emit(node, path, out);
        }
        if hops >= self.max_hops || eval_deadline_hit() {
            return false;
        }
        let mut next = Vec::new();
        let _ = self.ctx.storage.try_for_each_expand_id(
            node,
            self.step.direction,
            &self.step.rel.types,
            |rel_id, dst_id| {
                next.push((rel_id, dst_id));
                Ok::<(), ()>(())
            },
        );
        frames.push((next, 0));
        true
    }

    fn emit(
        &self,
        node: lora_store::NodeId,
        path: &[lora_store::RelationshipId],
        out: &mut Vec<Row>,
    ) {
        if self.dst_bound.is_some_and(|bound| bound != node)
            || self
                .rels_bound
                .as_deref()
                .is_some_and(|bound| bound != path)
        {
            return;
        }
        let matched = self
            .ctx
            .storage
            .with_node(node, |n| {
                crate::executor::node_matches_label_groups(n.labels(), &self.step.node.labels)
                    && node_matches_properties(
                        n.properties(),
                        &self.step.node.properties,
                        self.row,
                        self.ctx,
                    )
            })
            .unwrap_or(false);
        if !matched {
            return;
        }
        let mut r = self.row.clone();
        if let Some(rv) = self.step.rel.var {
            let rels = path.iter().map(|id| LoraValue::Relationship(*id)).collect();
            r.insert(rv, LoraValue::List(rels));
        }
        if let Some(nv) = self.step.node.var {
            r.insert(nv, LoraValue::Node(node));
        }
        out.push(r);
    }
}

fn node_matches_properties<'a, S: GraphStorage>(
    props: impl Into<lora_store::PropsRef<'a>>,
    expected: &Option<ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> bool {
    let Some(props_expr) = expected else {
        return true;
    };
    let props = props.into();
    let expected = eval_expr(props_expr, row, ctx);
    if let LoraValue::Map(exp) = expected {
        exp.iter().all(|(k, v)| {
            props
                .get(k.as_str())
                .map(|pv| crate::executor::value_matches_property_value(v, pv))
                .unwrap_or(false)
        })
    } else {
        true
    }
}

/// Whether relationship `rel_id` has the properties a pattern's
/// `[:T {k: v}]` map (evaluated against `row`) asks for.
fn rel_matches_properties<S: GraphStorage>(
    rel_id: lora_store::RelationshipId,
    expected: &Option<ResolvedExpr>,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> bool {
    if expected.is_none() {
        return true;
    }
    ctx.storage
        .with_relationship(rel_id, |rel| {
            node_matches_properties(rel.properties(), expected, row, ctx)
        })
        .unwrap_or(false)
}

pub fn eval_expr_result<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Result<LoraValue, String> {
    clear_eval_error();
    let value = eval_expr(expr, row, ctx);
    eval_status()?;
    Ok(value)
}

/// The outcome of the evaluations since the last [`clear_eval_error`]: the
/// error [`eval_expr_result`] would return, without wrapping a value. For a
/// hot loop that hands the value straight on (an aggregate fold) and only
/// needs to know whether evaluation failed.
#[inline]
pub(crate) fn eval_status() -> Result<(), String> {
    if crate::cancel::eval_tripped() {
        clear_eval_error();
        return Err(DEADLINE_EXCEEDED.to_string());
    }
    match take_eval_error() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

pub fn eval_truthy_result<S: GraphStorage>(
    expr: &ResolvedExpr,
    row: &Row,
    ctx: &EvalContext<'_, S>,
) -> Result<bool, String> {
    Ok(eval_expr_result(expr, row, ctx)?.is_truthy())
}

pub(super) fn eval_literal(lit: &LiteralValue) -> LoraValue {
    match lit {
        LiteralValue::Integer(v) => LoraValue::Int(*v),
        LiteralValue::Float(v) => LoraValue::Float(*v),
        LiteralValue::String(v) => LoraValue::String(v.clone()),
        LiteralValue::TypeName(v) => LoraValue::String(v.clone()),
        LiteralValue::Bool(v) => LoraValue::Bool(*v),
        LiteralValue::Null => LoraValue::Null,
        LiteralValue::Value(v) => LoraValue::from(&**v),
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
                node.properties()
                    .get(key)
                    .map(LoraValue::from)
                    .unwrap_or(LoraValue::Null)
            })
            .unwrap_or(LoraValue::Null),

        LoraValue::Relationship(id) => ctx
            .storage
            .with_relationship(*id, |rel| {
                rel.properties()
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
            // The zone's name for a named zone, else the offset.
            "timezone" => LoraValue::String(match dt.zone {
                Some(zone) => zone.name().to_string(),
                None => offset_string(dt.offset_seconds),
            }),
            "epochSeconds" => LoraValue::Int(dt.epoch_seconds()),
            "epochMillis" => LoraValue::Int(dt.to_epoch_millis()),
            _ => offset_component(dt.offset_seconds, key),
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
            "timezone" => LoraValue::String(offset_string(t.offset_seconds)),
            _ => offset_component(t.offset_seconds, key),
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

/// `offset`, `offsetMinutes`, `offsetSeconds` of a zoned temporal; null
/// for any other key.
fn offset_component(offset_seconds: i32, key: &str) -> LoraValue {
    match key {
        "offset" => LoraValue::String(offset_string(offset_seconds)),
        "offsetMinutes" => LoraValue::Int((offset_seconds / 60) as i64),
        "offsetSeconds" => LoraValue::Int(offset_seconds as i64),
        _ => LoraValue::Null,
    }
}

/// An offset as Cypher prints it: `Z`, `+02:00`, `-05:30`.
fn offset_string(offset_seconds: i32) -> String {
    if offset_seconds == 0 {
        return "Z".to_string();
    }
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let abs = offset_seconds.unsigned_abs();
    format!("{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60)
}
