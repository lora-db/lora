//! `bytes.*` — operations on binary values.

use std::io::{Read, Write};

use flate2::read::{DeflateDecoder, GzDecoder};
use flate2::write::{DeflateEncoder, GzEncoder};
use flate2::Compression;
use lora_store::LoraBinary;

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "size" => size(args),
        "from_string" => from_string(args),
        "to_string" => to_string_op(args),
        "base64_encode" => base64_encode(args),
        "base64_decode" => base64_decode(args),
        "hex_encode" => hex_encode(args),
        "hex_decode" => hex_decode(args),
        "compress" => compress(args),
        "decompress" => decompress(args),
        _ => return None,
    })
}

fn as_bytes(v: Option<&LoraValue>) -> Option<Vec<u8>> {
    match v? {
        LoraValue::Binary(b) => Some(b.to_vec()),
        _ => None,
    }
}

fn size(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Binary(b)) => LoraValue::Int(b.len() as i64),
        Some(LoraValue::String(s)) => LoraValue::Int(s.len() as i64),
        _ => LoraValue::Null,
    }
}

fn from_string(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(s)) = args.first() else {
        return LoraValue::Null;
    };
    // Encoding arg accepted for forward compat; UTF-8 is the only supported encoding.
    LoraValue::Binary(LoraBinary::from_bytes(s.as_bytes().to_vec()))
}

fn to_string_op(args: &[LoraValue]) -> LoraValue {
    let Some(b) = as_bytes(args.first()) else {
        return LoraValue::Null;
    };
    match String::from_utf8(b) {
        Ok(s) => LoraValue::String(s),
        Err(_) => LoraValue::Null,
    }
}

fn base64_encode(args: &[LoraValue]) -> LoraValue {
    let Some(bytes) = as_bytes(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::String(base64_encode_bytes(&bytes))
}

fn base64_decode(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(s)) = args.first() else {
        return LoraValue::Null;
    };
    match base64_decode_str(s) {
        Some(bytes) => LoraValue::Binary(LoraBinary::from_bytes(bytes)),
        None => LoraValue::Null,
    }
}

fn hex_encode(args: &[LoraValue]) -> LoraValue {
    let Some(bytes) = as_bytes(args.first()) else {
        return LoraValue::Null;
    };
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    LoraValue::String(out)
}

fn hex_decode(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(s)) = args.first() else {
        return LoraValue::Null;
    };
    if s.len() % 2 != 0 {
        return LoraValue::Null;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = hex_digit(chunk[0]);
        let lo = hex_digit(chunk[1]);
        match (hi, lo) {
            (Some(h), Some(l)) => out.push((h << 4) | l),
            _ => return LoraValue::Null,
        }
    }
    LoraValue::Binary(LoraBinary::from_bytes(out))
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn compress(args: &[LoraValue]) -> LoraValue {
    let Some(bytes) = as_bytes(args.first()) else {
        // Fall back to treating a string input as UTF-8 bytes.
        if let Some(LoraValue::String(s)) = args.first() {
            return compress_with(s.as_bytes(), args.get(1));
        }
        return LoraValue::Null;
    };
    compress_with(&bytes, args.get(1))
}

fn compress_with(bytes: &[u8], algo_arg: Option<&LoraValue>) -> LoraValue {
    let algo = match algo_arg {
        Some(LoraValue::String(s)) => s.to_ascii_lowercase(),
        _ => "gzip".to_string(),
    };
    let result = match algo.as_str() {
        "gzip" => {
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            if enc.write_all(bytes).is_err() {
                return LoraValue::Null;
            }
            enc.finish()
        }
        "deflate" => {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            if enc.write_all(bytes).is_err() {
                return LoraValue::Null;
            }
            enc.finish()
        }
        _ => return LoraValue::Null,
    };
    match result {
        Ok(v) => LoraValue::Binary(LoraBinary::from_bytes(v)),
        Err(_) => LoraValue::Null,
    }
}

fn decompress(args: &[LoraValue]) -> LoraValue {
    let Some(bytes) = as_bytes(args.first()) else {
        return LoraValue::Null;
    };
    let algo = match args.get(1) {
        Some(LoraValue::String(s)) => s.to_ascii_lowercase(),
        _ => "gzip".to_string(),
    };
    let mut out = Vec::new();
    let result = match algo.as_str() {
        "gzip" => GzDecoder::new(bytes.as_slice()).read_to_end(&mut out),
        "deflate" => DeflateDecoder::new(bytes.as_slice()).read_to_end(&mut out),
        _ => return LoraValue::Null,
    };
    if result.is_err() {
        return LoraValue::Null;
    }
    LoraValue::Binary(LoraBinary::from_bytes(out))
}

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode_bytes(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let chunks = input.chunks(3);
    for chunk in chunks {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(BASE64_TABLE[(b0 >> 2) as usize] as char);
        out.push(BASE64_TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64_TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(BASE64_TABLE[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode_str(input: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(input.len() / 4 * 3);
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    for chunk in bytes.chunks(4) {
        let v: [Option<u8>; 4] = [
            base64_value(chunk[0]),
            base64_value(chunk[1]),
            base64_value(chunk[2]),
            base64_value(chunk[3]),
        ];
        let b0 = v[0]?;
        let b1 = v[1]?;
        decoded.push((b0 << 2) | (b1 >> 4));
        if chunk[2] != b'=' {
            let b2 = v[2]?;
            decoded.push((b1 << 4) | (b2 >> 2));
            if chunk[3] != b'=' {
                let b3 = v[3]?;
                decoded.push((b2 << 6) | b3);
            }
        }
    }
    Some(decoded)
}

fn base64_value(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        b'=' => Some(0),
        _ => None,
    }
}
