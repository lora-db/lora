//! Ruby → `LoraValue` conversion (params and snapshot-option JSON).
//!
//! Inverse of [`crate::to_ruby`]. Tagged hashes (`{"kind" => "date", …}`)
//! become temporal/spatial values; plain hashes become `LoraValue::Map`.
//! Symbol keys and string keys are accepted interchangeably.

use std::collections::BTreeMap;

use magnus::{
    prelude::*, r_hash::ForEach, value::ReprValue, Error as MagnusError, Float, Integer, RArray,
    RHash, RString, Ruby, Symbol, Value,
};

use lora_database::LoraValue;
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType,
};

use crate::errors::invalid_params;

pub(crate) fn ruby_value_to_params(
    ruby: &Ruby,
    value: Value,
) -> Result<BTreeMap<String, LoraValue>, MagnusError> {
    let hash = RHash::try_convert(value)
        .map_err(|_| invalid_params(ruby, "params must be a Hash keyed by parameter name"))?;
    hash_to_string_map(ruby, hash)
}

fn hash_to_string_map(
    ruby: &Ruby,
    hash: RHash,
) -> Result<BTreeMap<String, LoraValue>, MagnusError> {
    let mut out = BTreeMap::new();
    let mut inner_err: Option<MagnusError> = None;
    hash.foreach(|k: Value, v: Value| {
        let key = match coerce_key(ruby, k) {
            Ok(s) => s,
            Err(e) => {
                inner_err = Some(e);
                return Ok(ForEach::Stop);
            }
        };
        match ruby_value_to_lora(ruby, v) {
            Ok(lv) => {
                out.insert(key, lv);
                Ok(ForEach::Continue)
            }
            Err(e) => {
                inner_err = Some(e);
                Ok(ForEach::Stop)
            }
        }
    })?;
    if let Some(e) = inner_err {
        return Err(e);
    }
    Ok(out)
}

fn coerce_key(ruby: &Ruby, v: Value) -> Result<String, MagnusError> {
    // Accept both String and Symbol keys — idiomatic Ruby. Reject anything
    // else loudly; silently stringifying would mask caller mistakes.
    if let Ok(s) = RString::try_convert(v) {
        return s.to_string();
    }
    if let Ok(s) = Symbol::try_convert(v) {
        return Ok(s.name()?.into_owned());
    }
    Err(invalid_params(ruby, "param keys must be String or Symbol"))
}

pub(crate) fn ruby_optional_to_json(
    ruby: &Ruby,
    value: Value,
) -> Result<Option<serde_json::Value>, MagnusError> {
    if value.is_nil() {
        Ok(None)
    } else {
        ruby_value_to_json(ruby, value).map(Some)
    }
}

fn ruby_value_to_json(ruby: &Ruby, value: Value) -> Result<serde_json::Value, MagnusError> {
    if value.is_nil() {
        return Ok(serde_json::Value::Null);
    }
    if value.is_kind_of(ruby.class_true_class()) {
        return Ok(serde_json::Value::Bool(true));
    }
    if value.is_kind_of(ruby.class_false_class()) {
        return Ok(serde_json::Value::Bool(false));
    }
    if let Ok(i) = Integer::try_convert(value) {
        let n = i
            .to_i64()
            .map_err(|_| invalid_params(ruby, "snapshot option integer does not fit in i64"))?;
        return Ok(serde_json::Value::Number(n.into()));
    }
    if let Ok(f) = Float::try_convert(value) {
        let Some(number) = serde_json::Number::from_f64(f.to_f64()) else {
            return Err(invalid_params(ruby, "snapshot option float must be finite"));
        };
        return Ok(serde_json::Value::Number(number));
    }
    if let Ok(s) = RString::try_convert(value) {
        return Ok(serde_json::Value::String(s.to_string()?));
    }
    if let Ok(sym) = Symbol::try_convert(value) {
        return Ok(serde_json::Value::String(sym.name()?.into_owned()));
    }
    if let Ok(arr) = RArray::try_convert(value) {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr.into_iter() {
            out.push(ruby_value_to_json(ruby, item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Ok(hash) = RHash::try_convert(value) {
        let mut out = serde_json::Map::new();
        let mut error = None;
        hash.foreach(|k: Value, v: Value| {
            let key = match coerce_key(ruby, k) {
                Ok(key) => key,
                Err(e) => {
                    error = Some(e);
                    return Ok(ForEach::Stop);
                }
            };
            let json = match ruby_value_to_json(ruby, v) {
                Ok(json) => json,
                Err(e) => {
                    error = Some(e);
                    return Ok(ForEach::Stop);
                }
            };
            out.insert(key, json);
            Ok(ForEach::Continue)
        })?;
        if let Some(error) = error {
            return Err(error);
        }
        return Ok(serde_json::Value::Object(out));
    }

    let class_name = unsafe { value.classname() }.into_owned();
    Err(invalid_params(
        ruby,
        format!("unsupported snapshot option type: {class_name}"),
    ))
}

fn ruby_value_to_lora(ruby: &Ruby, v: Value) -> Result<LoraValue, MagnusError> {
    if v.is_nil() {
        return Ok(LoraValue::Null);
    }
    // Check true/false before Integer — Ruby's TrueClass / FalseClass are
    // not Integer subclasses, but bool detection is cleaner first.
    if v.is_kind_of(ruby.class_true_class()) {
        return Ok(LoraValue::Bool(true));
    }
    if v.is_kind_of(ruby.class_false_class()) {
        return Ok(LoraValue::Bool(false));
    }
    // Float MUST be checked before Integer — `Integer::try_convert`
    // succeeds on Float because Ruby's `Float#to_int` (truncating
    // coercion) makes `Float` implicitly convertible. Taking that path
    // would turn `1.5` into `1` silently; callers never want that.
    if let Ok(f) = Float::try_convert(v) {
        return Ok(LoraValue::Float(f.to_f64()));
    }
    if let Ok(i) = Integer::try_convert(v) {
        return match i.to_i64() {
            Ok(n) => Ok(LoraValue::Int(n)),
            Err(_) => Err(invalid_params(
                ruby,
                "integer parameter does not fit in i64",
            )),
        };
    }
    if let Ok(s) = RString::try_convert(v) {
        return Ok(LoraValue::String(s.to_string()?));
    }
    if let Ok(sym) = Symbol::try_convert(v) {
        // Symbols round-trip as strings — same approach as YAML/JSON
        // mappings. Engine has no dedicated symbol value.
        return Ok(LoraValue::String(sym.name()?.into_owned()));
    }
    if let Ok(arr) = RArray::try_convert(v) {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr.into_iter() {
            out.push(ruby_value_to_lora(ruby, item)?);
        }
        return Ok(LoraValue::List(out));
    }
    if let Ok(hash) = RHash::try_convert(v) {
        return ruby_hash_to_cypher(ruby, hash);
    }
    let class_name = unsafe { v.classname() }.into_owned();
    Err(invalid_params(
        ruby,
        format!("unsupported parameter type: {class_name}"),
    ))
}

/// A Hash might be a tagged value (date / time / …/ point) or a plain
/// map. Nodes / relationships / paths are opaque on the engine side and
/// cannot be reconstructed as params — there's no `"kind" => "node"`
/// tag handled here.
fn ruby_hash_to_cypher(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    if let Some(kind) = lookup_kind(ruby, hash)? {
        match kind.as_str() {
            "date" => {
                return parse_tagged(ruby, hash, "date", |iso| {
                    LoraDate::parse(iso).map(LoraValue::Date)
                });
            }
            "time" => {
                return parse_tagged(ruby, hash, "time", |iso| {
                    LoraTime::parse(iso).map(LoraValue::Time)
                });
            }
            "localtime" => {
                return parse_tagged(ruby, hash, "localtime", |iso| {
                    LoraLocalTime::parse(iso).map(LoraValue::LocalTime)
                });
            }
            "datetime" => {
                return parse_tagged(ruby, hash, "datetime", |iso| {
                    LoraDateTime::parse(iso).map(LoraValue::DateTime)
                });
            }
            "localdatetime" => {
                return parse_tagged(ruby, hash, "localdatetime", |iso| {
                    LoraLocalDateTime::parse(iso).map(LoraValue::LocalDateTime)
                });
            }
            "duration" => {
                return parse_tagged(ruby, hash, "duration", |iso| {
                    LoraDuration::parse(iso).map(LoraValue::Duration)
                });
            }
            "point" => return build_point(ruby, hash),
            "vector" => return build_vector(ruby, hash),
            "binary" | "blob" => return build_binary(ruby, hash),
            _ => { /* fall through to plain-map handling */ }
        }
    }

    Ok(LoraValue::Map(hash_to_string_map(ruby, hash)?))
}

/// Look up `"kind"` (string) or `:kind` (symbol) under either key. Keeps
/// constructor hashes usable with either Ruby idiom.
fn lookup_kind(ruby: &Ruby, hash: RHash) -> Result<Option<String>, MagnusError> {
    if let Some(v) = hash.get(ruby.str_new("kind")) {
        return kind_as_string(v).map(Some);
    }
    if let Some(v) = hash.get(ruby.to_symbol("kind")) {
        return kind_as_string(v).map(Some);
    }
    Ok(None)
}

fn kind_as_string(v: Value) -> Result<String, MagnusError> {
    if let Ok(s) = RString::try_convert(v) {
        return s.to_string();
    }
    if let Ok(s) = Symbol::try_convert(v) {
        return Ok(s.name()?.into_owned());
    }
    // Anything else means "not a tagged constructor" — return empty so
    // the caller falls through to plain-map handling instead of raising.
    Ok(String::new())
}

fn parse_tagged(
    ruby: &Ruby,
    hash: RHash,
    tag: &str,
    parse: impl FnOnce(&str) -> Result<LoraValue, String>,
) -> Result<LoraValue, MagnusError> {
    let iso = read_string(ruby, hash, "iso")?
        .ok_or_else(|| invalid_params(ruby, format!("{tag} value requires iso: String")))?;
    parse(&iso).map_err(|e| invalid_params(ruby, format!("{tag}: {e}")))
}

fn build_point(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    let srid = read_u32(ruby, hash, "srid")?.unwrap_or(7203);
    let x = read_f64(ruby, hash, "x")?.ok_or_else(|| invalid_params(ruby, "point.x required"))?;
    let y = read_f64(ruby, hash, "y")?.ok_or_else(|| invalid_params(ruby, "point.y required"))?;
    let z = read_f64(ruby, hash, "z")?;
    Ok(LoraValue::Point(LoraPoint { x, y, z, srid }))
}

fn build_vector(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    let dimension = read_i64(ruby, hash, "dimension")?
        .ok_or_else(|| invalid_params(ruby, "vector.dimension required"))?;
    let coordinate_type_name = read_string(ruby, hash, "coordinateType")?
        .ok_or_else(|| invalid_params(ruby, "vector.coordinateType required"))?;
    let coordinate_type = VectorCoordinateType::parse(&coordinate_type_name).ok_or_else(|| {
        invalid_params(
            ruby,
            format!("unknown vector coordinate type '{coordinate_type_name}'"),
        )
    })?;
    let values_value = hash_get_either(ruby, hash, "values")
        .ok_or_else(|| invalid_params(ruby, "vector.values required"))?;
    let arr = RArray::try_convert(values_value)
        .map_err(|_| invalid_params(ruby, "vector.values must be an Array"))?;

    let mut raw = Vec::with_capacity(arr.len());
    for item in arr.into_iter() {
        if item.is_kind_of(ruby.class_true_class()) || item.is_kind_of(ruby.class_false_class()) {
            return Err(invalid_params(
                ruby,
                "vector.values entries must be numeric",
            ));
        }
        if let Ok(f) = Float::try_convert(item) {
            let v = f.to_f64();
            if !v.is_finite() {
                return Err(invalid_params(
                    ruby,
                    "vector.values cannot be NaN or Infinity",
                ));
            }
            raw.push(RawCoordinate::Float(v));
            continue;
        }
        if let Ok(i) = Integer::try_convert(item) {
            raw.push(RawCoordinate::Int(i.to_i64()?));
            continue;
        }
        return Err(invalid_params(
            ruby,
            "vector.values entries must be numeric",
        ));
    }

    let v = LoraVector::try_new(raw, dimension, coordinate_type)
        .map_err(|e| invalid_params(ruby, e.to_string()))?;
    Ok(LoraValue::Vector(v))
}

fn build_binary(ruby: &Ruby, hash: RHash) -> Result<LoraValue, MagnusError> {
    let segments_value = hash_get_either(ruby, hash, "segments")
        .ok_or_else(|| invalid_params(ruby, "binary.segments required"))?;
    let arr = RArray::try_convert(segments_value)
        .map_err(|_| invalid_params(ruby, "binary.segments must be an Array"))?;
    let mut segments = Vec::with_capacity(arr.len());
    for item in arr.into_iter() {
        let segment = RString::try_convert(item)
            .map_err(|_| invalid_params(ruby, "binary.segments entries must be Strings"))?;
        segments.push(unsafe { segment.as_slice().to_vec() });
    }
    Ok(LoraValue::Binary(LoraBinary::from_segments(segments)))
}

fn read_i64(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<i64>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    Ok(Some(Integer::try_convert(v)?.to_i64().map_err(|_| {
        invalid_params(ruby, format!("{key} out of i64 range"))
    })?))
}

// ---- Hash accessors that accept either string or symbol keys ------------

pub(crate) fn hash_get_either(ruby: &Ruby, hash: RHash, key: &str) -> Option<Value> {
    if let Some(v) = hash.get(ruby.str_new(key)) {
        return Some(v);
    }
    hash.get(ruby.to_symbol(key))
}

pub(crate) fn hash_get_any(ruby: &Ruby, hash: RHash, keys: &[&str]) -> Option<Value> {
    keys.iter().find_map(|key| hash_get_either(ruby, hash, key))
}

pub(crate) fn read_nonnegative_u64(ruby: &Ruby, value: Value) -> Result<u64, MagnusError> {
    let n = Integer::try_convert(value)?.to_i64()?;
    u64::try_from(n).map_err(|_| invalid_params(ruby, "option integer must be non-negative"))
}

fn read_string(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<String>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    let s = RString::try_convert(v)?.to_string()?;
    Ok(Some(s))
}

fn read_u32(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<u32>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    let n = Integer::try_convert(v)?.to_i64()?;
    u32::try_from(n)
        .map(Some)
        .map_err(|_| invalid_params(ruby, "srid out of u32 range"))
}

fn read_f64(ruby: &Ruby, hash: RHash, key: &str) -> Result<Option<f64>, MagnusError> {
    let Some(v) = hash_get_either(ruby, hash, key) else {
        return Ok(None);
    };
    // Accept either Float or Integer — `cartesian(1, 2)` passing ints
    // shouldn't force the caller to call `.to_f` first.
    if let Ok(f) = Float::try_convert(v) {
        return Ok(Some(f.to_f64()));
    }
    if let Ok(i) = Integer::try_convert(v) {
        return Ok(Some(i.to_i64()? as f64));
    }
    Ok(None)
}
