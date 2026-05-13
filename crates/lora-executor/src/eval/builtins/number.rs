//! `number.*` — numeric formatting, parsing, and bit-level operations.

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "format" => format(args),
        "to_base" => to_base(args),
        "from_base" => from_base(args),
        "to_roman" => to_roman(args),
        "from_roman" => from_roman(args),
        "bitop" => bitop(args),
        "is_integer" => is_integer(args),
        "is_even" => is_even(args),
        "is_odd" => is_odd(args),
        "is_positive" => is_positive(args),
        "is_negative" => is_negative(args),
        "is_zero" => is_zero(args),
        "is_nan" => is_nan(args),
        "is_finite" => is_finite(args),
        "is_infinite" => is_infinite(args),
        _ => return None,
    })
}

fn format(args: &[LoraValue]) -> LoraValue {
    let n = match args.first() {
        Some(LoraValue::Int(i)) => *i as f64,
        Some(LoraValue::Float(f)) => *f,
        _ => return LoraValue::Null,
    };
    let precision = args.get(1).and_then(LoraValue::as_i64);
    let thousands = match args.get(2) {
        Some(LoraValue::String(s)) => Some(s.clone()),
        _ => None,
    };
    let s = match precision {
        Some(p) if p >= 0 => format!("{:.*}", p as usize, n),
        _ => {
            if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e18 {
                format!("{}", n as i64)
            } else {
                format!("{n}")
            }
        }
    };
    if let Some(sep) = thousands {
        LoraValue::String(group_thousands(&s, &sep))
    } else {
        LoraValue::String(s)
    }
}

fn group_thousands(s: &str, sep: &str) -> String {
    let (sign, rest) = if let Some(stripped) = s.strip_prefix('-') {
        ("-", stripped)
    } else {
        ("", s)
    };
    let (int_part, frac_part) = match rest.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let mut grouped = String::with_capacity(int_part.len() + int_part.len() / 3);
    for (i, ch) in int_part.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push_str(&sep.chars().rev().collect::<String>());
        }
        grouped.push(ch);
    }
    let grouped: String = grouped.chars().rev().collect();
    match frac_part {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

fn to_base(args: &[LoraValue]) -> LoraValue {
    let (Some(n), Some(radix)) = (
        args.first().and_then(LoraValue::as_i64),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    let Ok(radix) = u32::try_from(radix) else {
        return LoraValue::Null;
    };
    if !(2..=36).contains(&radix) {
        return LoraValue::Null;
    }

    let mut value = n.unsigned_abs();
    let mut digits = Vec::new();
    loop {
        digits.push(digit_char((value % u64::from(radix)) as u32));
        value /= u64::from(radix);
        if value == 0 {
            break;
        }
    }
    if n < 0 {
        digits.push('-');
    }
    digits.reverse();
    LoraValue::String(digits.into_iter().collect())
}

fn from_base(args: &[LoraValue]) -> LoraValue {
    let (Some(LoraValue::String(raw)), Some(radix)) =
        (args.first(), args.get(1).and_then(LoraValue::as_i64))
    else {
        return LoraValue::Null;
    };
    let Ok(radix) = u32::try_from(radix) else {
        return LoraValue::Null;
    };
    if !(2..=36).contains(&radix) {
        return LoraValue::Null;
    }

    let value = raw.trim();
    if value.is_empty() {
        return LoraValue::Null;
    }
    let (negative, digits) = match value.as_bytes()[0] {
        b'-' => (true, &value[1..]),
        b'+' => (false, &value[1..]),
        _ => (false, value),
    };
    if digits.is_empty() {
        return LoraValue::Null;
    }

    let mut total = 0_u64;
    for ch in digits.chars() {
        let Some(digit) = ch.to_digit(36).filter(|digit| *digit < radix) else {
            return LoraValue::Null;
        };
        total = total
            .checked_mul(u64::from(radix))
            .and_then(|n| n.checked_add(u64::from(digit)))
            .unwrap_or(u64::MAX);
        if total == u64::MAX {
            return LoraValue::Null;
        }
    }

    if negative {
        if total == (i64::MAX as u64) + 1 {
            LoraValue::Int(i64::MIN)
        } else {
            i64::try_from(total)
                .ok()
                .and_then(|n| n.checked_neg())
                .map(LoraValue::Int)
                .unwrap_or(LoraValue::Null)
        }
    } else {
        i64::try_from(total)
            .map(LoraValue::Int)
            .unwrap_or(LoraValue::Null)
    }
}

fn digit_char(digit: u32) -> char {
    match digit {
        0..=9 => (b'0' + digit as u8) as char,
        10..=35 => (b'a' + (digit as u8 - 10)) as char,
        _ => unreachable!("radix conversion only emits base-36 digits"),
    }
}

fn to_roman(args: &[LoraValue]) -> LoraValue {
    let Some(n) = args.first().and_then(LoraValue::as_i64) else {
        return LoraValue::Null;
    };
    if n <= 0 || n > 3999 {
        return LoraValue::Null;
    }
    let pairs: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    let mut n = n;
    for (val, sym) in pairs {
        while n >= val {
            out.push_str(sym);
            n -= val;
        }
    }
    LoraValue::String(out)
}

fn from_roman(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(s)) = args.first() else {
        return LoraValue::Null;
    };
    let s = s.to_ascii_uppercase();
    let value = |c: char| match c {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        'L' => 50,
        'C' => 100,
        'D' => 500,
        'M' => 1000,
        _ => -1,
    };
    let chars: Vec<char> = s.chars().collect();
    let mut total = 0_i64;
    let mut i = 0;
    while i < chars.len() {
        let v = value(chars[i]);
        if v < 0 {
            return LoraValue::Null;
        }
        let next = chars.get(i + 1).map(|c| value(*c)).unwrap_or(-1);
        if next > v {
            total += next - v;
            i += 2;
        } else {
            total += v;
            i += 1;
        }
    }
    LoraValue::Int(total)
}

fn bitop(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(LoraValue::String(op)), Some(b)) = (
        args.first().and_then(LoraValue::as_i64),
        args.get(1),
        args.get(2).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    let result = match op.to_ascii_lowercase().as_str() {
        "and" => a & b,
        "or" => a | b,
        "xor" => a ^ b,
        "shl" => a.wrapping_shl(b as u32),
        "shr" => a.wrapping_shr(b as u32),
        "not" => !a,
        _ => return LoraValue::Null,
    };
    LoraValue::Int(result)
}

fn is_integer(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(_)) => LoraValue::Bool(true),
        Some(LoraValue::Float(f)) => LoraValue::Bool(f.is_finite() && f.fract() == 0.0),
        _ => LoraValue::Null,
    }
}

fn is_even(args: &[LoraValue]) -> LoraValue {
    match args.first().and_then(LoraValue::as_i64) {
        Some(n) => LoraValue::Bool(n % 2 == 0),
        None => LoraValue::Null,
    }
}

fn is_odd(args: &[LoraValue]) -> LoraValue {
    match args.first().and_then(LoraValue::as_i64) {
        Some(n) => LoraValue::Bool(n % 2 != 0),
        None => LoraValue::Null,
    }
}

fn is_positive(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(n)) => LoraValue::Bool(*n > 0),
        Some(LoraValue::Float(n)) => LoraValue::Bool(*n > 0.0),
        _ => LoraValue::Null,
    }
}

fn is_negative(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(n)) => LoraValue::Bool(*n < 0),
        Some(LoraValue::Float(n)) => LoraValue::Bool(*n < 0.0),
        _ => LoraValue::Null,
    }
}

fn is_zero(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(n)) => LoraValue::Bool(*n == 0),
        Some(LoraValue::Float(n)) => LoraValue::Bool(*n == 0.0),
        _ => LoraValue::Null,
    }
}

fn is_nan(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Float(f)) => LoraValue::Bool(f.is_nan()),
        Some(LoraValue::Int(_)) => LoraValue::Bool(false),
        _ => LoraValue::Null,
    }
}

fn is_finite(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Float(f)) => LoraValue::Bool(f.is_finite()),
        Some(LoraValue::Int(_)) => LoraValue::Bool(true),
        _ => LoraValue::Null,
    }
}

fn is_infinite(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Float(f)) => LoraValue::Bool(f.is_infinite()),
        Some(LoraValue::Int(_)) => LoraValue::Bool(false),
        _ => LoraValue::Null,
    }
}
