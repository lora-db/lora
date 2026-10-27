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

use std::collections::BTreeMap;

use lora_analyzer::{LiteralValue, ResolvedExpr};
use lora_ast::{BinaryOp, UnaryOp};

use crate::logical::{LogicalOp, LogicalPlan};

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

fn mentions_null_test(expr: &ResolvedExpr) -> bool {
    lora_ast::stack::guard(|| mentions_null_test_inner(expr))
}

fn mentions_null_test_inner(expr: &ResolvedExpr) -> bool {
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
    lora_ast::stack::guard(|| fold_inner(expr, nullness, assumed))
}

fn fold_inner(
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
}
