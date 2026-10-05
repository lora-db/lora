//! Unary and binary operator evaluation, structural equality and
//! comparison, and arithmetic value combinators.
//!
//! These primitives are shared between the [`super::expr::eval_expr`]
//! dispatcher (for the `Unary` / `Binary` / `Case` arms) and the
//! built-in function library in [`super::functions`] (`add` /
//! `subtract` / etc. on temporal types and the in-list / contains
//! lookups that need full structural equality).

use lora_ast::{BinaryOp, UnaryOp};
use lora_store::LoraDuration;

use crate::value::LoraValue;

use super::errors::set_eval_error;
use super::regex;

pub(super) fn eval_unary(op: UnaryOp, value: LoraValue) -> LoraValue {
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
            LoraValue::Int(v) => match v.checked_neg() {
                Some(out) => LoraValue::Int(out),
                None => arithmetic_overflow("integer negation"),
            },
            LoraValue::Float(v) => LoraValue::Float(-v),
            _ => LoraValue::Null,
        },
    }
}

/// `lhs IN rhs`, reading the list in place.
/// openCypher `IN`: true when an element equals `lhs`; otherwise null when
/// the answer is unknown (`lhs` is null, or an element is), else false. An
/// empty list holds nothing, so `null IN []` is false.
pub(super) fn eval_in(lhs: &LoraValue, rhs: &LoraValue) -> LoraValue {
    match rhs {
        LoraValue::List(values) if values.is_empty() => LoraValue::Bool(false),
        _ if matches!(lhs, LoraValue::Null) => LoraValue::Null,
        LoraValue::List(values) => {
            if values.iter().any(|v| value_eq(lhs, v)) {
                LoraValue::Bool(true)
            } else if values.iter().any(|v| matches!(v, LoraValue::Null)) {
                LoraValue::Null
            } else {
                LoraValue::Bool(false)
            }
        }
        LoraValue::Null => LoraValue::Null,
        _ => LoraValue::Bool(false),
    }
}

pub(super) fn eval_binary(op: &BinaryOp, lhs: LoraValue, rhs: LoraValue) -> LoraValue {
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

        // Cypher ordering: null or incomparable operands give null; lists
        // compare element by element. Two temporals of different kinds are
        // an error rather than Cypher's null: `d >= date()` against a
        // DATETIME `d` would otherwise drop every row without a word.
        BinaryOp::Lt | BinaryOp::Gt | BinaryOp::Le | BinaryOp::Ge => {
            match compare_values(&lhs, &rhs) {
                None => {
                    if let Some(msg) = temporal_kind_mismatch(&lhs, &rhs) {
                        set_eval_error(msg);
                    }
                    LoraValue::Null
                }
                Some(ord) => LoraValue::Bool(match op {
                    BinaryOp::Lt => ord.is_lt(),
                    BinaryOp::Gt => ord.is_gt(),
                    BinaryOp::Le => ord.is_le(),
                    _ => ord.is_ge(),
                }),
            }
        }

        BinaryOp::Add => add_values(lhs, rhs),
        BinaryOp::Sub => sub_values(lhs, rhs),
        BinaryOp::Mul => mul_values(lhs, rhs),
        BinaryOp::Div => div_values(lhs, rhs),
        BinaryOp::Mod => mod_values(lhs, rhs),
        BinaryOp::Pow => pow_values(lhs, rhs),

        BinaryOp::In => eval_in(&lhs, &rhs),

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
            (LoraValue::String(s), LoraValue::String(pattern)) => regex::full_match(&s, &pattern)
                .map(LoraValue::Bool)
                .unwrap_or(LoraValue::Null),
            _ => LoraValue::Bool(false),
        },
    }
}

pub(super) fn substring_by_chars(s: &str, start: i64, length: Option<i64>) -> String {
    let start = start.max(0) as usize;
    let chars = s.chars().skip(start);
    match length {
        Some(length) => chars.take(length.max(0) as usize).collect(),
        None => chars.collect(),
    }
}

pub(super) fn value_eq(a: &LoraValue, b: &LoraValue) -> bool {
    match (a, b) {
        (LoraValue::Null, LoraValue::Null) => true,
        (LoraValue::Bool(x), LoraValue::Bool(y)) => x == y,
        (LoraValue::Int(x), LoraValue::Int(y)) => x == y,
        (LoraValue::Float(x), LoraValue::Float(y)) => x == y,
        (LoraValue::Int(x), LoraValue::Float(y)) => (*x as f64) == *y,
        (LoraValue::Float(x), LoraValue::Int(y)) => *x == (*y as f64),
        (LoraValue::String(x), LoraValue::String(y)) => x == y,
        (LoraValue::Binary(x), LoraValue::Binary(y)) => x == y,
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
        (LoraValue::Vector(x), LoraValue::Vector(y)) => x == y,
        _ => false,
    }
}

/// How `a` orders against `b` for `<`, `<=`, `>` and `>=`; `None` when
/// the comparison is null: a null operand, or values of kinds Cypher does
/// not order against each other (a string and a number, two temporals of
/// different kinds, ...).
///
/// Numbers compare by value, integers exactly (never through `f64`),
/// strings by code point, booleans with `false < true`, temporals of one
/// kind by the instant they denote. Lists compare lexicographically: the
/// first pair of elements that differs decides, a null or incomparable
/// pair before that makes the result null, and a list that is a prefix of
/// the other sorts first.
pub(crate) fn compare_values(a: &LoraValue, b: &LoraValue) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (a, b) {
        (LoraValue::Null, _) | (_, LoraValue::Null) => None,
        (LoraValue::Int(x), LoraValue::Int(y)) => Some(x.cmp(y)),
        (LoraValue::Int(x), LoraValue::Float(y)) => int_float_cmp(*x, *y),
        (LoraValue::Float(x), LoraValue::Int(y)) => int_float_cmp(*y, *x).map(Ordering::reverse),
        (LoraValue::Float(x), LoraValue::Float(y)) => x.partial_cmp(y),
        (LoraValue::String(x), LoraValue::String(y)) => Some(x.as_str().cmp(y.as_str())),
        (LoraValue::Bool(x), LoraValue::Bool(y)) => Some(x.cmp(y)),
        (LoraValue::Duration(x), LoraValue::Duration(y)) => x
            .total_seconds_approx()
            .partial_cmp(&y.total_seconds_approx()),
        (LoraValue::List(xs), LoraValue::List(ys)) => {
            for (x, y) in xs.iter().zip(ys.iter()) {
                match compare_values(x, y)? {
                    Ordering::Equal => continue,
                    decided => return Some(decided),
                }
            }
            Some(xs.len().cmp(&ys.len()))
        }
        _ => a.temporal_cmp(b),
    }
}

/// The error for ordering two temporal values of different kinds (a DATE
/// against a DATETIME, a TIME against a LOCAL_TIME, ...), which Cypher
/// would silently answer with null. `None` when the operands are not two
/// temporals of different kinds.
pub(crate) fn temporal_kind_mismatch(a: &LoraValue, b: &LoraValue) -> Option<String> {
    let kind = |v: &LoraValue| -> Option<&'static str> {
        Some(match v {
            LoraValue::Date(_) => "DATE",
            LoraValue::DateTime(_) => "DATETIME",
            LoraValue::LocalDateTime(_) => "LOCAL_DATETIME",
            LoraValue::Time(_) => "TIME",
            LoraValue::LocalTime(_) => "LOCAL_TIME",
            _ => return None,
        })
    };
    let (ka, kb) = (kind(a)?, kind(b)?);
    (ka != kb).then(|| {
        format!(
            "cannot compare a {ka} with a {kb}: temporal values of different types do not \
             order against each other; convert one side first, e.g. date(x) or datetime(x)"
        )
    })
}

/// An integer against a float, exactly: `2^53 + 1 > 2^53 as f64` holds.
/// NaN orders against nothing.
fn int_float_cmp(i: i64, f: f64) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if f.is_nan() {
        return None;
    }
    // Every i64 lies in [-2^63, 2^63); a float outside it decides alone.
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    if f >= TWO_63 {
        return Some(Ordering::Less);
    }
    if f < -TWO_63 {
        return Some(Ordering::Greater);
    }
    let whole = f.trunc();
    match i.cmp(&(whole as i64)) {
        Ordering::Equal => 0.0f64.partial_cmp(&(f - whole)),
        decided => Some(decided),
    }
}

fn add_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Null, _) | (_, LoraValue::Null) => LoraValue::Null,
        (LoraValue::Int(a), LoraValue::Int(b)) => match a.checked_add(b) {
            Some(out) => LoraValue::Int(out),
            None => arithmetic_overflow("integer addition"),
        },
        (LoraValue::String(a), LoraValue::String(b)) => LoraValue::String(a + &b),
        // A string and a number concatenate, as toString would print the
        // number: 'p' + 1 is 'p1' (it was a silent null).
        (LoraValue::String(a), LoraValue::Int(b)) => LoraValue::String(format!("{a}{b}")),
        (LoraValue::String(a), LoraValue::Float(b)) => LoraValue::String(format!("{a}{b}")),
        (LoraValue::Int(a), LoraValue::String(b)) => LoraValue::String(format!("{a}{b}")),
        (LoraValue::Float(a), LoraValue::String(b)) => LoraValue::String(format!("{a}{b}")),
        (LoraValue::List(mut a), LoraValue::List(b)) => {
            a.extend(b);
            LoraValue::List(a)
        }
        // A list and an element: append or prepend it.
        (LoraValue::List(mut a), b) => {
            a.push(b);
            LoraValue::List(a)
        }
        (a, LoraValue::List(mut b)) => {
            b.insert(0, a);
            LoraValue::List(b)
        }
        // Temporal + Duration
        (LoraValue::Date(d), LoraValue::Duration(dur)) => match d.try_add_duration(&dur) {
            Some(out) => LoraValue::Date(out),
            None => arithmetic_overflow("date duration addition"),
        },
        (LoraValue::Duration(dur), LoraValue::Date(d)) => match d.try_add_duration(&dur) {
            Some(out) => LoraValue::Date(out),
            None => arithmetic_overflow("date duration addition"),
        },
        (LoraValue::DateTime(dt), LoraValue::Duration(dur)) => match dt.try_add_duration(&dur) {
            Some(out) => LoraValue::DateTime(out),
            None => arithmetic_overflow("datetime duration addition"),
        },
        (LoraValue::Duration(dur), LoraValue::DateTime(dt)) => match dt.try_add_duration(&dur) {
            Some(out) => LoraValue::DateTime(out),
            None => arithmetic_overflow("datetime duration addition"),
        },
        (LoraValue::Duration(a), LoraValue::Duration(b)) => match a.try_add(&b) {
            Some(out) => LoraValue::Duration(out),
            None => arithmetic_overflow("duration addition"),
        },
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
            (Some(x), Some(y)) => LoraValue::Float(x + y),
            _ => {
                set_eval_error(format!(
                    "Cannot add {} and {}",
                    crate::errors::value_kind(&a),
                    crate::errors::value_kind(&b)
                ));
                LoraValue::Null
            }
        },
    }
}

fn sub_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Int(a), LoraValue::Int(b)) => match a.checked_sub(b) {
            Some(out) => LoraValue::Int(out),
            None => arithmetic_overflow("integer subtraction"),
        },
        // Temporal - Duration
        (LoraValue::Date(d), LoraValue::Duration(dur)) => match d.try_sub_duration(&dur) {
            Some(out) => LoraValue::Date(out),
            None => arithmetic_overflow("date duration subtraction"),
        },
        (LoraValue::DateTime(dt), LoraValue::Duration(dur)) => match dur.try_negate() {
            Some(negated) => match dt.try_add_duration(&negated) {
                Some(out) => LoraValue::DateTime(out),
                None => arithmetic_overflow("datetime duration subtraction"),
            },
            None => arithmetic_overflow("duration negation"),
        },
        // Temporal - Temporal -> Duration
        (LoraValue::Date(d1), LoraValue::Date(d2)) => {
            LoraValue::Duration(LoraDuration::in_days(&d2, &d1))
        }
        (LoraValue::DateTime(dt1), LoraValue::DateTime(dt2)) => {
            LoraValue::Duration(LoraDuration::between_datetimes(&dt2, &dt1))
        }
        // Duration - Duration
        (LoraValue::Duration(a), LoraValue::Duration(b)) => {
            match b.try_negate().and_then(|b| a.try_add(&b)) {
                Some(out) => LoraValue::Duration(out),
                None => arithmetic_overflow("duration subtraction"),
            }
        }
        (a, b) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => LoraValue::Float(a - b),
            _ => LoraValue::Null,
        },
    }
}

fn mul_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs, rhs) {
        (LoraValue::Int(a), LoraValue::Int(b)) => match a.checked_mul(b) {
            Some(out) => LoraValue::Int(out),
            None => arithmetic_overflow("integer multiplication"),
        },
        (LoraValue::Duration(d), LoraValue::Int(n)) => match d.try_mul_int(n) {
            Some(out) => LoraValue::Duration(out),
            None => arithmetic_overflow("duration multiplication"),
        },
        (LoraValue::Int(n), LoraValue::Duration(d)) => match d.try_mul_int(n) {
            Some(out) => LoraValue::Duration(out),
            None => arithmetic_overflow("duration multiplication"),
        },
        (a, b) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => LoraValue::Float(a * b),
            _ => LoraValue::Null,
        },
    }
}

fn div_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (&lhs, &rhs) {
        (LoraValue::Duration(d), LoraValue::Int(n)) if *n != 0 => {
            return match d.try_div_int(*n) {
                Some(out) => LoraValue::Duration(out),
                None => arithmetic_overflow("duration division"),
            };
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
        (LoraValue::Int(a), LoraValue::Int(b)) => match a.checked_rem(b) {
            Some(out) => LoraValue::Int(out),
            None => arithmetic_overflow("integer modulo"),
        },
        _ => LoraValue::Null,
    }
}

pub(super) fn arithmetic_overflow(op: &str) -> LoraValue {
    set_eval_error(format!("{op} overflowed"));
    LoraValue::Null
}

fn pow_values(lhs: LoraValue, rhs: LoraValue) -> LoraValue {
    match (lhs.as_f64(), rhs.as_f64()) {
        (Some(a), Some(b)) => {
            let out = a.powf(b);
            if !out.is_finite() {
                LoraValue::Null
            } else if out.fract() == 0.0 && f64_fits_i64(out) {
                LoraValue::Int(out as i64)
            } else {
                LoraValue::Float(out)
            }
        }
        _ => LoraValue::Null,
    }
}

#[inline]
fn f64_fits_i64(value: f64) -> bool {
    value >= i64::MIN as f64 && value < 9_223_372_036_854_775_808.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::errors::{clear_eval_error, take_eval_error};

    fn take_overflow_from(value: LoraValue) -> String {
        assert!(matches!(value, LoraValue::Null));
        take_eval_error().expect("overflow should set eval error")
    }

    #[test]
    fn integer_overflow_returns_null_with_error() {
        clear_eval_error();
        let err = take_overflow_from(eval_binary(
            &BinaryOp::Add,
            LoraValue::Int(i64::MAX),
            LoraValue::Int(1),
        ));
        assert!(err.contains("integer addition overflowed"));

        clear_eval_error();
        let err = take_overflow_from(eval_unary(UnaryOp::Neg, LoraValue::Int(i64::MIN)));
        assert!(err.contains("integer negation overflowed"));

        clear_eval_error();
        let err = take_overflow_from(eval_binary(
            &BinaryOp::Mod,
            LoraValue::Int(i64::MIN),
            LoraValue::Int(-1),
        ));
        assert!(err.contains("integer modulo overflowed"));
    }

    #[test]
    fn duration_overflow_returns_null_with_error() {
        let duration = LoraDuration {
            months: i64::MAX,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        };

        clear_eval_error();
        let err = take_overflow_from(eval_binary(
            &BinaryOp::Add,
            LoraValue::Duration(duration.clone()),
            LoraValue::Duration(duration),
        ));
        assert!(err.contains("duration addition overflowed"));
    }

    #[test]
    fn temporal_duration_overflow_returns_null_with_error() {
        let date = lora_store::LoraDate {
            year: i32::MAX,
            month: 12,
            day: 31,
        };

        clear_eval_error();
        let err = take_overflow_from(eval_binary(
            &BinaryOp::Add,
            LoraValue::Date(date),
            LoraValue::Duration(LoraDuration {
                months: 1,
                days: 0,
                seconds: 0,
                nanoseconds: 0,
            }),
        ));
        assert!(err.contains("date duration addition overflowed"));
    }

    #[test]
    fn pow_rejects_non_finite_results() {
        assert!(matches!(
            eval_binary(
                &BinaryOp::Pow,
                LoraValue::Float(f64::MAX),
                LoraValue::Int(2),
            ),
            LoraValue::Null
        ));
    }
}
