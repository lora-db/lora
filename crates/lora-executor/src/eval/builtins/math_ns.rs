//! `math.*` — extended numerics beyond the Cypher language built-ins
//! (`abs`, `sqrt`, `sin`, etc. continue to live in
//! [`super::super::functions`]).

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "min" => min(args),
        "max" => max(args),
        "round" => round(args),
        "trunc" => trunc(args),
        "sigmoid" => unary_f(args, |f| 1.0 / (1.0 + (-f).exp())),
        "tanh" => unary_f(args, f64::tanh),
        "cosh" => unary_f(args, f64::cosh),
        "sinh" => unary_f(args, f64::sinh),
        "cot" => unary_f(args, |f| 1.0 / f.tan()),
        "coth" => unary_f(args, |f| 1.0 / f.tanh()),
        "atan2" => binary_f(args, f64::atan2),
        "pow" => binary_f(args, f64::powf),
        "hypot" => binary_f(args, f64::hypot),
        "log_base" => log_base(args),
        "gcd" => gcd(args),
        "lcm" => lcm(args),
        "clamp" => clamp(args),
        "lerp" => lerp(args),
        "abs" => abs(args),
        "ceil" => ceil(args),
        "floor" => floor(args),
        "sqrt" => unary_f_guarded(args, |f| (f >= 0.0).then(|| f.sqrt())),
        "sign" => sign(args),
        "log" | "ln" => unary_f_guarded(args, |f| (f > 0.0).then(|| f.ln())),
        "log10" => unary_f_guarded(args, |f| (f > 0.0).then(|| f.log10())),
        "exp" => unary_f(args, f64::exp),
        "sin" => unary_f(args, f64::sin),
        "cos" => unary_f(args, f64::cos),
        "tan" => unary_f(args, f64::tan),
        "asin" => unary_f_guarded(args, |f| (-1.0..=1.0).contains(&f).then(|| f.asin())),
        "acos" => unary_f_guarded(args, |f| (-1.0..=1.0).contains(&f).then(|| f.acos())),
        "atan" => unary_f(args, f64::atan),
        "degrees" => unary_f(args, f64::to_degrees),
        "radians" => unary_f(args, f64::to_radians),
        "pi" => LoraValue::Float(std::f64::consts::PI),
        "e" => LoraValue::Float(std::f64::consts::E),
        "random" => random(),
        _ => return None,
    })
}

fn unary_f_guarded(args: &[LoraValue], op: impl FnOnce(f64) -> Option<f64>) -> LoraValue {
    args.first()
        .and_then(LoraValue::as_f64)
        .and_then(op)
        .map(LoraValue::Float)
        .unwrap_or(LoraValue::Null)
}

fn min(args: &[LoraValue]) -> LoraValue {
    min_max(args, |current, candidate| candidate < current)
}

fn max(args: &[LoraValue]) -> LoraValue {
    min_max(args, |current, candidate| candidate > current)
}

fn min_max(args: &[LoraValue], pick_rhs: impl Fn(f64, f64) -> bool) -> LoraValue {
    let mut best = None;
    let mut all_ints = true;
    for arg in args {
        match arg {
            LoraValue::Int(n) => {
                let n = *n as f64;
                if best.is_none_or(|current| pick_rhs(current, n)) {
                    best = Some(n);
                }
            }
            LoraValue::Float(n) if n.is_finite() => {
                all_ints = false;
                if best.is_none_or(|current| pick_rhs(current, *n)) {
                    best = Some(*n);
                }
            }
            _ => return LoraValue::Null,
        }
    }
    match best {
        Some(n) if all_ints => LoraValue::Int(n as i64),
        Some(n) => LoraValue::Float(n),
        None => LoraValue::Null,
    }
}

fn abs(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(i)) => i
            .checked_abs()
            .map(LoraValue::Int)
            .unwrap_or(LoraValue::Null),
        Some(LoraValue::Float(f)) => LoraValue::Float(f.abs()),
        _ => LoraValue::Null,
    }
}

fn ceil(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Float(f)) => f64_to_i64(f.ceil())
            .map(LoraValue::Int)
            .unwrap_or(LoraValue::Null),
        Some(LoraValue::Int(i)) => LoraValue::Int(*i),
        _ => LoraValue::Null,
    }
}

fn floor(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Float(f)) => f64_to_i64(f.floor())
            .map(LoraValue::Int)
            .unwrap_or(LoraValue::Null),
        Some(LoraValue::Int(i)) => LoraValue::Int(*i),
        _ => LoraValue::Null,
    }
}

fn trunc(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Float(f)) => f64_to_i64(f.trunc())
            .map(LoraValue::Int)
            .unwrap_or(LoraValue::Null),
        Some(LoraValue::Int(i)) => LoraValue::Int(*i),
        _ => LoraValue::Null,
    }
}

fn sign(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(i)) => LoraValue::Int(i.signum()),
        Some(LoraValue::Float(f)) => {
            if f.is_nan() {
                LoraValue::Null
            } else {
                LoraValue::Int(f.signum() as i64)
            }
        }
        _ => LoraValue::Null,
    }
}

fn random() -> LoraValue {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let hash = ((nanos as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)) as f64;
    LoraValue::Float((hash / u64::MAX as f64).abs())
}

fn f64_to_i64(value: f64) -> Option<i64> {
    (value.is_finite() && value >= i64::MIN as f64 && value < 9_223_372_036_854_775_808.0)
        .then_some(value as i64)
}

fn unary_f(args: &[LoraValue], f: impl Fn(f64) -> f64) -> LoraValue {
    match args.first().and_then(LoraValue::as_f64) {
        Some(v) => LoraValue::Float(f(v)),
        None => LoraValue::Null,
    }
}

fn binary_f(args: &[LoraValue], f: impl Fn(f64, f64) -> f64) -> LoraValue {
    match (
        args.first().and_then(LoraValue::as_f64),
        args.get(1).and_then(LoraValue::as_f64),
    ) {
        (Some(a), Some(b)) => LoraValue::Float(f(a, b)),
        _ => LoraValue::Null,
    }
}

fn round(args: &[LoraValue]) -> LoraValue {
    let Some(n) = args.first().and_then(LoraValue::as_f64) else {
        return LoraValue::Null;
    };
    let digits_arg = args.get(1).and_then(LoraValue::as_i64);
    let mode = match args.get(2) {
        Some(LoraValue::String(s)) => s.to_ascii_lowercase(),
        _ => "half_up".to_string(),
    };
    let digits = digits_arg.unwrap_or(0);
    let scale = 10f64.powi(digits as i32);
    let scaled = n * scale;
    let rounded = match mode.as_str() {
        "ceil" => scaled.ceil(),
        "floor" => scaled.floor(),
        "trunc" => scaled.trunc(),
        "half_up" => {
            if scaled >= 0.0 {
                (scaled + 0.5).floor()
            } else {
                -((-scaled + 0.5).floor())
            }
        }
        "half_even" => scaled.round_ties_even(),
        _ => return LoraValue::Null,
    };
    let result = rounded / scale;
    // No digits arg → match Cypher convention and return an INT.
    if digits_arg.is_none() {
        if let Some(i) = f64_to_i64(result) {
            return LoraValue::Int(i);
        }
    }
    LoraValue::Float(result)
}

fn log_base(args: &[LoraValue]) -> LoraValue {
    let (Some(n), Some(base)) = (
        args.first().and_then(LoraValue::as_f64),
        args.get(1).and_then(LoraValue::as_f64),
    ) else {
        return LoraValue::Null;
    };
    if n <= 0.0 || base <= 0.0 || base == 1.0 {
        return LoraValue::Null;
    }
    LoraValue::Float(n.log(base))
}

fn gcd(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b)) = (
        args.first().and_then(LoraValue::as_i64),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    LoraValue::Int(gcd_i(a.unsigned_abs(), b.unsigned_abs()) as i64)
}

fn gcd_i(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

fn lcm(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b)) = (
        args.first().and_then(LoraValue::as_i64),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    if a == 0 || b == 0 {
        return LoraValue::Int(0);
    }
    let g = gcd_i(a.unsigned_abs(), b.unsigned_abs());
    LoraValue::Int(((a / g as i64) * b).abs())
}

fn clamp(args: &[LoraValue]) -> LoraValue {
    let (Some(x), Some(lo), Some(hi)) = (
        args.first().and_then(LoraValue::as_f64),
        args.get(1).and_then(LoraValue::as_f64),
        args.get(2).and_then(LoraValue::as_f64),
    ) else {
        return LoraValue::Null;
    };
    if matches!(args.first(), Some(LoraValue::Int(_)))
        && matches!(args.get(1), Some(LoraValue::Int(_)))
        && matches!(args.get(2), Some(LoraValue::Int(_)))
    {
        LoraValue::Int((x as i64).clamp(lo as i64, hi as i64))
    } else {
        LoraValue::Float(x.clamp(lo, hi))
    }
}

fn lerp(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b), Some(t)) = (
        args.first().and_then(LoraValue::as_f64),
        args.get(1).and_then(LoraValue::as_f64),
        args.get(2).and_then(LoraValue::as_f64),
    ) else {
        return LoraValue::Null;
    };
    LoraValue::Float(a + (b - a) * t)
}
