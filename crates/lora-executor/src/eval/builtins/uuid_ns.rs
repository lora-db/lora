//! `uuid.*` — UUID v4 generation and validation.
//!
//! Inline implementation against `getrandom` — we don't pull in the
//! `uuid` crate for two functions.

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "new" => new_v4(),
        "from_string" => from_string(args),
        "is_valid" => is_valid(args),
        _ => return None,
    })
}

fn new_v4() -> LoraValue {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        return LoraValue::Null;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant
    LoraValue::String(format_uuid(&bytes))
}

fn from_string(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::String(s)) => match parse_uuid(s) {
            Some(bytes) => LoraValue::String(format_uuid(&bytes)),
            None => LoraValue::Null,
        },
        _ => LoraValue::Null,
    }
}

fn is_valid(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::String(s)) => LoraValue::Bool(parse_uuid(s).is_some()),
        _ => LoraValue::Null,
    }
}

fn format_uuid(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
    )
}

fn parse_uuid(s: &str) -> Option<[u8; 16]> {
    let cleaned: String = s.chars().filter(|c| *c != '-').collect();
    if cleaned.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_value(cleaned.as_bytes()[i * 2])?;
        let lo = hex_value(cleaned.as_bytes()[i * 2 + 1])?;
        *byte = (hi << 4) | lo;
    }
    Some(out)
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
