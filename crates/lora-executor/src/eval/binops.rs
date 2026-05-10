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
            LoraValue::Int(v) => LoraValue::Int(-v),
            LoraValue::Float(v) => LoraValue::Float(-v),
            _ => LoraValue::Null,
        },
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
                LoraValue::List(values) => {
                    LoraValue::Bool(values.iter().any(|v| value_eq(&lhs, v)))
                }
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
        (LoraValue::DateTime(a), LoraValue::DateTime(b)) => LoraValue::Bool(num_cmp(
            a.to_epoch_millis() as f64,
            b.to_epoch_millis() as f64,
        )),
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
        (LoraValue::DateTime(dt), LoraValue::Duration(dur)) => {
            LoraValue::DateTime(dt.add_duration(&dur))
        }
        (LoraValue::Duration(dur), LoraValue::DateTime(dt)) => {
            LoraValue::DateTime(dt.add_duration(&dur))
        }
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
