//! `json.*` — JSON encode/decode/path operations.
//!
//! Uses `serde_json` directly. The path mini-language is a strict subset
//! of JSONPath: `$.foo.bar[0]` style. No filter expressions, no wildcards
//! — those are out of scope for v1.

use std::collections::BTreeMap;

use serde_json::Value as JsonValue;

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "encode" => encode(args),
        "decode" => decode(args),
        "path" => path(args),
        _ => return None,
    })
}

fn encode(args: &[LoraValue]) -> LoraValue {
    let Some(v) = args.first() else {
        return LoraValue::Null;
    };
    let pretty = matches!(args.get(1), Some(LoraValue::Bool(true)));
    let json = match to_json(v) {
        Some(j) => j,
        None => return LoraValue::Null,
    };
    let out = if pretty {
        serde_json::to_string_pretty(&json)
    } else {
        serde_json::to_string(&json)
    };
    match out {
        Ok(s) => LoraValue::String(s),
        Err(_) => LoraValue::Null,
    }
}

fn decode(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(s)) = args.first() else {
        return LoraValue::Null;
    };
    match serde_json::from_str::<JsonValue>(s) {
        Ok(j) => from_json(&j),
        Err(_) => LoraValue::Null,
    }
}

fn path(args: &[LoraValue]) -> LoraValue {
    let (Some(v), Some(LoraValue::String(p))) = (args.first(), args.get(1)) else {
        return LoraValue::Null;
    };
    walk_path(v, p).unwrap_or(LoraValue::Null)
}

fn walk_path(root: &LoraValue, path: &str) -> Option<LoraValue> {
    let path = path.strip_prefix('$').unwrap_or(path);
    let mut current = root.clone();
    let mut chars = path.chars().peekable();
    while let Some(&ch) = chars.peek() {
        match ch {
            '.' => {
                chars.next();
                let mut key = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '.' || c == '[' {
                        break;
                    }
                    key.push(c);
                    chars.next();
                }
                if key.is_empty() {
                    return None;
                }
                current = match current {
                    LoraValue::Map(m) => m.get(&key).cloned()?,
                    _ => return None,
                };
            }
            '[' => {
                chars.next();
                let mut num = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ']' {
                        break;
                    }
                    num.push(c);
                    chars.next();
                }
                chars.next(); // consume ']'
                let idx: i64 = num.parse().ok()?;
                current = match current {
                    LoraValue::List(xs) => {
                        let real = if idx < 0 { idx + xs.len() as i64 } else { idx };
                        if real < 0 || real >= xs.len() as i64 {
                            return None;
                        }
                        xs[real as usize].clone()
                    }
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    Some(current)
}

fn to_json(v: &LoraValue) -> Option<JsonValue> {
    Some(match v {
        LoraValue::Null => JsonValue::Null,
        LoraValue::Bool(b) => JsonValue::Bool(*b),
        LoraValue::Int(i) => JsonValue::Number((*i).into()),
        LoraValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        LoraValue::String(s) => JsonValue::String(s.clone()),
        LoraValue::List(xs) => {
            let mut out = Vec::with_capacity(xs.len());
            for v in xs {
                out.push(to_json(v)?);
            }
            JsonValue::Array(out)
        }
        LoraValue::Map(m) => {
            let mut out = serde_json::Map::with_capacity(m.len());
            for (k, v) in m {
                out.insert(k.clone(), to_json(v)?);
            }
            JsonValue::Object(out)
        }
        LoraValue::Date(d) => JsonValue::String(d.to_string()),
        LoraValue::DateTime(d) => JsonValue::String(d.to_string()),
        LoraValue::LocalDateTime(d) => JsonValue::String(d.to_string()),
        LoraValue::Time(t) => JsonValue::String(t.to_string()),
        LoraValue::LocalTime(t) => JsonValue::String(t.to_string()),
        LoraValue::Duration(d) => JsonValue::String(d.to_string()),
        // Graph entities can't round-trip through JSON without a snapshot;
        // they intentionally serialise to null and the caller must project
        // them with `properties(...)` first if they want them.
        _ => return None,
    })
}

fn from_json(v: &JsonValue) -> LoraValue {
    match v {
        JsonValue::Null => LoraValue::Null,
        JsonValue::Bool(b) => LoraValue::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                LoraValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                LoraValue::Float(f)
            } else {
                LoraValue::Null
            }
        }
        JsonValue::String(s) => LoraValue::String(s.clone()),
        JsonValue::Array(xs) => LoraValue::List(xs.iter().map(from_json).collect()),
        JsonValue::Object(m) => {
            let mut out: BTreeMap<String, LoraValue> = BTreeMap::new();
            for (k, v) in m {
                out.insert(k.clone(), from_json(v));
            }
            LoraValue::Map(out)
        }
    }
}
