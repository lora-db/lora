//! `bits.*` — integer bit operations without a stringly operation argument.

use crate::value::LoraValue;

use super::super::binops::arithmetic_overflow;
use super::super::errors::set_eval_error;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "and" => binary(args, |a, b| a & b),
        "or" => binary(args, |a, b| a | b),
        "xor" => binary(args, |a, b| a ^ b),
        "shift_left" => shift(args, "bits.shift_left", shift_left),
        "shift_right" => shift(args, "bits.shift_right", shift_right),
        "not" => unary(args, |a| !a),
        _ => return None,
    })
}

fn unary(args: &[LoraValue], f: impl FnOnce(i64) -> i64) -> LoraValue {
    args.first()
        .and_then(LoraValue::as_i64)
        .map(f)
        .map(LoraValue::Int)
        .unwrap_or(LoraValue::Null)
}

fn binary(args: &[LoraValue], f: impl FnOnce(i64, i64) -> i64) -> LoraValue {
    match (
        args.first().and_then(LoraValue::as_i64),
        args.get(1).and_then(LoraValue::as_i64),
    ) {
        (Some(a), Some(b)) => LoraValue::Int(f(a, b)),
        _ => LoraValue::Null,
    }
}

fn shift(args: &[LoraValue], name: &str, f: fn(i64, i64, &str) -> LoraValue) -> LoraValue {
    match (
        args.first().and_then(LoraValue::as_i64),
        args.get(1).and_then(LoraValue::as_i64),
    ) {
        (Some(a), Some(b)) => f(a, b, name),
        _ => LoraValue::Null,
    }
}

/// `a` shifted left by `b` bits, i.e. `a * 2^b`. A shift outside 0..=63 is
/// an error, and so is a result that doesn't fit an integer, as for the
/// arithmetic operators.
pub(super) fn shift_left(a: i64, b: i64, name: &str) -> LoraValue {
    if !shift_in_range(b, name) {
        return LoraValue::Null;
    }
    let out = a << b;
    if out >> b != a {
        return arithmetic_overflow(name);
    }
    LoraValue::Int(out)
}

/// `a` shifted right by `b` bits, keeping the sign (`-8 >> 1` is `-4`).
/// A shift outside 0..=63 is an error.
pub(super) fn shift_right(a: i64, b: i64, name: &str) -> LoraValue {
    if !shift_in_range(b, name) {
        return LoraValue::Null;
    }
    LoraValue::Int(a >> b)
}

fn shift_in_range(b: i64, name: &str) -> bool {
    if (0..=63).contains(&b) {
        return true;
    }
    set_eval_error(format!("{name} shift must be between 0 and 63, got {b}"));
    false
}
