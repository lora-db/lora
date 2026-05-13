//! `bits.*` — integer bit operations without a stringly operation argument.

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "and" => binary(args, |a, b| a & b),
        "or" => binary(args, |a, b| a | b),
        "xor" => binary(args, |a, b| a ^ b),
        "shift_left" => binary(args, |a, b| a.wrapping_shl(b as u32)),
        "shift_right" => binary(args, |a, b| a.wrapping_shr(b as u32)),
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
