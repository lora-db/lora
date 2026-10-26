//! Fold `$p IS NULL` / `$p IS NOT NULL` when the plan is built for known
//! parameters.
//!
//! An optional filter written as `$kinds IS NULL OR n.kind IN $kinds` is a
//! disjunction the index picker cannot use, so the seek falls to another
//! conjunct, however unselective, and every candidate it yields is held
//! until the filter drops it. With the parameter's value known, the test
//! is a constant: `false OR n.kind IN $kinds` is `n.kind IN $kinds`, which
//! the picker seeks like any `IN`, and `true OR …` drops the disjunction.
//!
//! The plan is then only valid for parameters with the same nullness;
//! [`CompiledQuery::null_params`](crate::CompiledQuery::null_params)
//! records what was assumed, and the plan cache keys on it.
//!
//! The same pass folds builtin calls on constant arguments
//! ([`fold_constant_calls`]): `datetime('2024-01-01T00:00:00Z')` in a
//! filter is otherwise parsed again for every row. Parameters are not
//! constants here, since a plan is reused across their values.

use std::collections::BTreeMap;

use lora_analyzer::{
    FunctionId, LiteralValue, ResolvedExpr, ResolvedMapSelector, ResolvedNode,
    ResolvedPatternElement, ResolvedSetItem,
};
use lora_ast::{BinaryOp, UnaryOp};

use crate::logical::{LogicalOp, LogicalPlan};

/// Evaluates a builtin call whose arguments are all constants, when the
/// plan is built. `None` leaves the call to run per row: the function is
/// not pure (`rand()`, `datetime()`), its value depends on when it runs,
/// or it fails on these arguments, so the error is still raised only when
/// a row reaches it.
pub type FoldCall<'a> = &'a dyn Fn(FunctionId, &[ResolvedExpr]) -> Option<LiteralValue>;

/// Whether a parameter is null, for the parameters the caller knows.
pub type NullnessOf<'a> = &'a dyn Fn(&str) -> Option<bool>;

/// Fold the parameter null tests of every filter in `plan`, recording in
/// `assumed` every parameter tested and the nullness it was folded for
/// (`None`: unknown, left as it is).
pub(crate) fn fold_parameter_null_tests(
    plan: &mut LogicalPlan,
    nullness: NullnessOf<'_>,
    assumed: &mut BTreeMap<String, Option<bool>>,
) {
    for op in &mut plan.nodes {
        if let LogicalOp::Filter(filter) = op {
            if mentions_null_test(&filter.predicate) {
                let predicate = std::mem::replace(
                    &mut filter.predicate,
                    ResolvedExpr::Literal(LiteralValue::Null),
                );
                filter.predicate = fold(predicate, nullness, assumed);
            }
        }
    }
}

/// Replace each builtin call on constant arguments in the expressions
/// `plan` evaluates per row with the value `fold` gives it, innermost
/// first, so `date(datetime('…'))` folds whole. Runs on the optimized
/// plan: index selection still sees the calls it recognises, such as
/// `point({...})`.
pub(crate) fn fold_constant_calls(plan: &mut LogicalPlan, fold: FoldCall<'_>) {
    for op in &mut plan.nodes {
        match op {
            LogicalOp::Filter(filter) => fold_calls(&mut filter.predicate, fold),
            LogicalOp::Projection(projection) => {
                for item in &mut projection.items {
                    fold_calls(&mut item.expr, fold);
                }
            }
            LogicalOp::Aggregation(aggregation) => {
                for item in aggregation
                    .group_by
                    .iter_mut()
                    .chain(aggregation.aggregates.iter_mut())
                {
                    fold_calls(&mut item.expr, fold);
                }
            }
            LogicalOp::Sort(sort) => {
                for item in &mut sort.items {
                    fold_calls(&mut item.expr, fold);
                }
            }
            LogicalOp::Unwind(unwind) => fold_calls(&mut unwind.expr, fold),
            LogicalOp::Set(set) => {
                for item in &mut set.items {
                    match item {
                        ResolvedSetItem::SetProperty { value, .. }
                        | ResolvedSetItem::SetVariable { value, .. }
                        | ResolvedSetItem::MutateVariable { value, .. } => fold_calls(value, fold),
                        ResolvedSetItem::SetLabels { .. } => {}
                    }
                }
            }
            LogicalOp::Create(create) => {
                for part in &mut create.pattern.parts {
                    let (head, chain) = match &mut part.element {
                        ResolvedPatternElement::Node { properties, .. } => {
                            if let Some(properties) = properties {
                                fold_calls(properties, fold);
                            }
                            continue;
                        }
                        ResolvedPatternElement::NodeChain { head, chain }
                        | ResolvedPatternElement::ShortestPath { head, chain, .. } => (head, chain),
                    };
                    let fold_node = |node: &mut ResolvedNode| {
                        if let Some(properties) = &mut node.properties {
                            fold_calls(properties, fold);
                        }
                    };
                    fold_node(head);
                    for link in chain {
                        if let Some(properties) = &mut link.rel.properties {
                            fold_calls(properties, fold);
                        }
                        fold_node(&mut link.node);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A literal, or a list or map of them: what a call can be folded over.
fn is_constant(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Literal(_) => true,
        ResolvedExpr::List(items) => items.iter().all(is_constant),
        ResolvedExpr::Map(items) => items.iter().all(|(_, value)| is_constant(value)),
        _ => false,
    }
}

fn fold_calls(expr: &mut ResolvedExpr, fold: FoldCall<'_>) {
    match expr {
        ResolvedExpr::Function {
            function,
            distinct,
            args,
        } => {
            for arg in args.iter_mut() {
                fold_calls(arg, fold);
            }
            if matches!(function, FunctionId::Builtin(_))
                && !*distinct
                && args.iter().all(is_constant)
            {
                if let Some(value) = fold(*function, args) {
                    *expr = ResolvedExpr::Literal(value);
                }
            }
        }
        ResolvedExpr::Property { expr, .. } | ResolvedExpr::Unary { expr, .. } => {
            fold_calls(expr, fold)
        }
        ResolvedExpr::Binary { lhs, rhs, .. } => {
            fold_calls(lhs, fold);
            fold_calls(rhs, fold);
        }
        ResolvedExpr::List(items) => {
            for item in items {
                fold_calls(item, fold);
            }
        }
        ResolvedExpr::Map(items) => {
            for (_, value) in items {
                fold_calls(value, fold);
            }
        }
        ResolvedExpr::Case {
            input,
            alternatives,
            else_expr,
        } => {
            if let Some(input) = input {
                fold_calls(input, fold);
            }
            for (when, then) in alternatives {
                fold_calls(when, fold);
                fold_calls(then, fold);
            }
            if let Some(else_expr) = else_expr {
                fold_calls(else_expr, fold);
            }
        }
        ResolvedExpr::ListPredicate {
            list, predicate, ..
        } => {
            fold_calls(list, fold);
            fold_calls(predicate, fold);
        }
        ResolvedExpr::ListComprehension {
            list,
            filter,
            map_expr,
            ..
        } => {
            fold_calls(list, fold);
            for inner in [filter, map_expr].into_iter().flatten() {
                fold_calls(inner, fold);
            }
        }
        ResolvedExpr::Reduce {
            init, list, expr, ..
        } => {
            fold_calls(init, fold);
            fold_calls(list, fold);
            fold_calls(expr, fold);
        }
        ResolvedExpr::MapProjection { base, selectors } => {
            fold_calls(base, fold);
            for selector in selectors {
                if let ResolvedMapSelector::Literal(_, value) = selector {
                    fold_calls(value, fold);
                }
            }
        }
        ResolvedExpr::Index { expr, index } => {
            fold_calls(expr, fold);
            fold_calls(index, fold);
        }
        ResolvedExpr::Slice { expr, from, to } => {
            fold_calls(expr, fold);
            for bound in [from, to].into_iter().flatten() {
                fold_calls(bound, fold);
            }
        }
        // Pattern subqueries keep their own copies of WHERE conjuncts
        // (`seeks`); they are left as written.
        ResolvedExpr::Variable(_)
        | ResolvedExpr::Literal(_)
        | ResolvedExpr::Parameter(_)
        | ResolvedExpr::ExistsSubquery { .. }
        | ResolvedExpr::PatternComprehension { .. } => {}
    }
}

fn mentions_null_test(expr: &ResolvedExpr) -> bool {
    match expr {
        ResolvedExpr::Binary { lhs, op, rhs } => match op {
            BinaryOp::IsNull | BinaryOp::IsNotNull => {
                matches!(**lhs, ResolvedExpr::Parameter(_))
            }
            BinaryOp::And | BinaryOp::Or => mentions_null_test(lhs) || mentions_null_test(rhs),
            _ => false,
        },
        ResolvedExpr::Unary {
            op: UnaryOp::Not,
            expr,
        } => mentions_null_test(expr),
        _ => false,
    }
}

fn boolean(expr: &ResolvedExpr) -> Option<bool> {
    match expr {
        ResolvedExpr::Literal(LiteralValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn lit(b: bool) -> ResolvedExpr {
    ResolvedExpr::Literal(LiteralValue::Bool(b))
}

/// Fold the null tests in `expr` and simplify the `AND`, `OR` and `NOT`
/// over them. Each rule holds in three-valued logic: `false AND x` and
/// `true OR x` are decided whatever `x` is, `true AND x` and `false OR x`
/// are `x`.
fn fold(
    expr: ResolvedExpr,
    nullness: NullnessOf<'_>,
    assumed: &mut BTreeMap<String, Option<bool>>,
) -> ResolvedExpr {
    match expr {
        ResolvedExpr::Binary { lhs, op, rhs }
            if matches!(op, BinaryOp::IsNull | BinaryOp::IsNotNull)
                && matches!(*lhs, ResolvedExpr::Parameter(_)) =>
        {
            let ResolvedExpr::Parameter(name) = &*lhs else {
                unreachable!()
            };
            let known = nullness(name);
            assumed.insert(name.clone(), known);
            match known {
                Some(is_null) => lit(if op == BinaryOp::IsNull {
                    is_null
                } else {
                    !is_null
                }),
                None => ResolvedExpr::Binary { lhs, op, rhs },
            }
        }
        ResolvedExpr::Binary {
            lhs,
            op: op @ (BinaryOp::And | BinaryOp::Or),
            rhs,
        } => {
            let lhs = fold(*lhs, nullness, assumed);
            let rhs = fold(*rhs, nullness, assumed);
            let decides = op == BinaryOp::Or;
            match (boolean(&lhs), boolean(&rhs)) {
                (Some(b), _) | (_, Some(b)) if b == decides => lit(decides),
                (Some(_), _) => rhs,
                (_, Some(_)) => lhs,
                _ => ResolvedExpr::Binary {
                    lhs: Box::new(lhs),
                    op,
                    rhs: Box::new(rhs),
                },
            }
        }
        ResolvedExpr::Unary {
            op: UnaryOp::Not,
            expr,
        } => {
            let inner = fold(*expr, nullness, assumed);
            match boolean(&inner) {
                Some(b) => lit(!b),
                None => ResolvedExpr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(inner),
                },
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_analyzer::symbols::VarId;

    fn param_is_null(name: &str) -> ResolvedExpr {
        ResolvedExpr::Binary {
            lhs: Box::new(ResolvedExpr::Parameter(name.into())),
            op: BinaryOp::IsNull,
            rhs: Box::new(ResolvedExpr::Literal(LiteralValue::Null)),
        }
    }

    fn other() -> ResolvedExpr {
        ResolvedExpr::Property {
            expr: Box::new(ResolvedExpr::Variable(VarId(1))),
            property: "kind".into(),
        }
    }

    fn or(a: ResolvedExpr, b: ResolvedExpr) -> ResolvedExpr {
        ResolvedExpr::Binary {
            lhs: Box::new(a),
            op: BinaryOp::Or,
            rhs: Box::new(b),
        }
    }

    #[test]
    fn a_known_parameter_folds_and_the_disjunction_simplifies() {
        let mut assumed = BTreeMap::new();
        let not_null = |_: &str| Some(false);
        let folded = fold(or(param_is_null("k"), other()), &not_null, &mut assumed);
        assert!(matches!(folded, ResolvedExpr::Property { .. }));
        assert_eq!(assumed.get("k"), Some(&Some(false)));

        let mut assumed = BTreeMap::new();
        let null = |_: &str| Some(true);
        let folded = fold(or(param_is_null("k"), other()), &null, &mut assumed);
        assert!(matches!(
            folded,
            ResolvedExpr::Literal(LiteralValue::Bool(true))
        ));
    }

    #[test]
    fn an_unknown_parameter_is_left_alone() {
        let mut assumed = BTreeMap::new();
        let unknown = |_: &str| None;
        let folded = fold(or(param_is_null("k"), other()), &unknown, &mut assumed);
        assert!(matches!(
            folded,
            ResolvedExpr::Binary {
                op: BinaryOp::Or,
                ..
            }
        ));
        assert_eq!(assumed.get("k"), Some(&None));
    }

    fn call(name: &str, args: Vec<ResolvedExpr>) -> ResolvedExpr {
        ResolvedExpr::Function {
            function: FunctionId::builtin(name).expect("builtin exists"),
            distinct: false,
            args,
        }
    }

    fn str_lit(s: &str) -> ResolvedExpr {
        ResolvedExpr::Literal(LiteralValue::String(s.into()))
    }

    /// Folds every call it is offered to the number of arguments it had,
    /// recording the names offered.
    fn folding<'a>(
        offered: &'a std::cell::RefCell<Vec<&'static str>>,
    ) -> impl Fn(FunctionId, &[ResolvedExpr]) -> Option<LiteralValue> + 'a {
        move |function, args| {
            offered.borrow_mut().push(function.name());
            (function.name() != "string.lower").then_some(LiteralValue::Integer(args.len() as i64))
        }
    }

    #[test]
    fn calls_on_constants_fold_innermost_first() {
        let offered = std::cell::RefCell::new(Vec::new());
        let fold = folding(&offered);
        // abs(size('ab', 'cd')): the inner call folds, then the outer one
        // is offered its value.
        let mut expr = call(
            "math.abs",
            vec![call(
                "value.size",
                vec![ResolvedExpr::List(vec![str_lit("ab"), str_lit("cd")])],
            )],
        );
        fold_calls(&mut expr, &fold);
        assert_eq!(*offered.borrow(), ["value.size", "math.abs"]);
        assert!(matches!(
            expr,
            ResolvedExpr::Literal(LiteralValue::Integer(1))
        ));
    }

    #[test]
    fn calls_on_rows_or_parameters_are_not_offered() {
        let offered = std::cell::RefCell::new(Vec::new());
        let fold = folding(&offered);
        for arg in [other(), ResolvedExpr::Parameter("p".into())] {
            let mut expr = call("math.abs", vec![arg]);
            fold_calls(&mut expr, &fold);
            assert!(matches!(expr, ResolvedExpr::Function { .. }));
        }
        assert!(offered.borrow().is_empty());
    }

    #[test]
    fn a_declined_call_stays() {
        let offered = std::cell::RefCell::new(Vec::new());
        let fold = folding(&offered);
        let mut expr = or(
            call("string.lower", vec![str_lit("A")]),
            call("math.abs", vec![str_lit("x")]),
        );
        fold_calls(&mut expr, &fold);
        let ResolvedExpr::Binary { lhs, rhs, .. } = expr else {
            panic!("the OR stays");
        };
        assert!(matches!(*lhs, ResolvedExpr::Function { .. }));
        assert!(matches!(
            *rhs,
            ResolvedExpr::Literal(LiteralValue::Integer(1))
        ));
    }
}
