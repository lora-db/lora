//! `crypto.*` — cryptographic and checksum hashes.
//!
//! Only algorithms the workspace already depends on are exposed here.
//! BLAKE3 is the recommended hash for new code; CRC32 is provided for
//! interop with legacy systems. Add MD5/SHA-1/SHA-256 deps later if
//! we need legacy-interop coverage — the deliberate omission keeps the
//! security surface small.

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "blake3" => blake3_hash(args),
        "crc32" => crc32(args),
        _ => return None,
    })
}

fn input_bytes(v: Option<&LoraValue>) -> Option<Vec<u8>> {
    match v? {
        LoraValue::String(s) => Some(s.as_bytes().to_vec()),
        LoraValue::Binary(b) => Some(b.to_vec()),
        _ => None,
    }
}

fn blake3_hash(args: &[LoraValue]) -> LoraValue {
    let Some(bytes) = input_bytes(args.first()) else {
        return LoraValue::Null;
    };
    let hash = blake3::hash(&bytes);
    LoraValue::String(hash.to_hex().to_string())
}

fn crc32(args: &[LoraValue]) -> LoraValue {
    let Some(bytes) = input_bytes(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::Int(crc32fast::hash(&bytes) as i64)
}
