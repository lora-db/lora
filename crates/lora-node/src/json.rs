//! JSON ↔ `LoraValue` conversion shared by the Node bindings.
//!
//! These helpers are pure Rust (no `#[napi]` attributes) — they take
//! and return `serde_json::Value` and `LoraValue`, and surface
//! validation errors through `napi::Error` so they can be returned
//! directly from the `#[napi]` methods in `lib.rs`.

use std::collections::BTreeMap;

use napi::bindgen_prelude::Result;
use napi::{Error as NapiError, Status};

use lora_database::{LoraValue, Row};
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType,
};

use super::INVALID_PARAMS_CODE;

pub(crate) fn serialize_rows(columns: &[String], rows: &[Vec<LoraValue>]) -> serde_json::Value {
    let columns_json: Vec<serde_json::Value> = columns
        .iter()
        .map(|c| serde_json::Value::String(c.clone()))
        .collect();

    let rows_json: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::with_capacity(columns.len());
            for (col, val) in columns.iter().zip(row.iter()) {
                obj.insert(col.clone(), lora_value_to_json(val));
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    serde_json::json!({
        "columns": serde_json::Value::Array(columns_json),
        "rows": serde_json::Value::Array(rows_json),
    })
}

pub(crate) fn row_to_json(row: &Row) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (_, name, value) in row.iter_named() {
        obj.insert(name.into_owned(), lora_value_to_json(value));
    }
    serde_json::Value::Object(obj)
}

// ============================================================================
// LoraValue <-> JSON conversion
// ============================================================================

pub(crate) fn lora_value_to_json(value: &LoraValue) -> serde_json::Value {
    use serde_json::Value as J;
    match value {
        LoraValue::Null => J::Null,
        LoraValue::Bool(b) => J::Bool(*b),
        LoraValue::Int(i) => J::Number((*i).into()),
        LoraValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        LoraValue::String(s) => J::String(s.clone()),
        LoraValue::Binary(b) => binary_to_json(b),
        LoraValue::List(items) => J::Array(items.iter().map(lora_value_to_json).collect()),
        LoraValue::Map(m) => {
            let obj = m
                .iter()
                .map(|(k, v)| (k.clone(), lora_value_to_json(v)))
                .collect::<serde_json::Map<_, _>>();
            J::Object(obj)
        }
        LoraValue::Node(id) => serde_json::json!({
            "kind": "node",
            "id": *id as i64,
            "labels": serde_json::Value::Array(vec![]),
            "properties": serde_json::Value::Object(Default::default()),
        }),
        LoraValue::Relationship(id) => serde_json::json!({
            "kind": "relationship",
            "id": *id as i64,
        }),
        LoraValue::Path(p) => serde_json::json!({
            "kind": "path",
            "nodes": p.nodes.iter().map(|n| *n as i64).collect::<Vec<_>>(),
            "rels": p.rels.iter().map(|n| *n as i64).collect::<Vec<_>>(),
        }),
        LoraValue::Date(d) => serde_json::json!({ "kind": "date", "iso": d.to_string() }),
        LoraValue::Time(t) => serde_json::json!({ "kind": "time", "iso": t.to_string() }),
        LoraValue::LocalTime(t) => {
            serde_json::json!({ "kind": "localtime", "iso": t.to_string() })
        }
        LoraValue::DateTime(dt) => {
            serde_json::json!({ "kind": "datetime", "iso": dt.to_string() })
        }
        LoraValue::LocalDateTime(dt) => {
            serde_json::json!({ "kind": "localdatetime", "iso": dt.to_string() })
        }
        LoraValue::Duration(d) => {
            serde_json::json!({ "kind": "duration", "iso": d.to_string() })
        }
        LoraValue::Point(p) => point_to_json(p),
        LoraValue::Vector(v) => vector_to_json(v),
    }
}

fn binary_to_json(b: &LoraBinary) -> serde_json::Value {
    serde_json::json!({
        "kind": "binary",
        "length": b.len(),
        "segments": b.segments(),
    })
}

/// Render a `LoraVector` into the canonical external tagged shape.
fn vector_to_json(v: &LoraVector) -> serde_json::Value {
    let values: serde_json::Value = match &v.values {
        lora_store::VectorValues::Float64(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        lora_store::VectorValues::Float32(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x as f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        lora_store::VectorValues::Integer64(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x)).collect())
        }
        lora_store::VectorValues::Integer32(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        lora_store::VectorValues::Integer16(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        lora_store::VectorValues::Integer8(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
    };
    serde_json::json!({
        "kind": "vector",
        "dimension": v.dimension,
        "coordinateType": v.coordinate_type().as_str(),
        "values": values,
    })
}

/// Render a `LoraPoint` into the canonical external point shape consumed
/// by the shared TS `LoraPoint` union. Cartesian points carry `x`/`y`
/// (and `z` for 3D); WGS-84 points additionally expose the geographic
/// aliases `longitude`/`latitude` (and `height` for 3D).
fn point_to_json(p: &LoraPoint) -> serde_json::Value {
    let mut obj = serde_json::Map::with_capacity(7);
    obj.insert("kind".into(), serde_json::Value::String("point".into()));
    obj.insert("srid".into(), serde_json::json!(p.srid));
    obj.insert("crs".into(), serde_json::Value::String(p.crs_name().into()));
    obj.insert("x".into(), serde_json::json!(p.x));
    obj.insert("y".into(), serde_json::json!(p.y));
    if let Some(z) = p.z {
        obj.insert("z".into(), serde_json::json!(z));
    }
    if p.is_geographic() {
        obj.insert("longitude".into(), serde_json::json!(p.longitude()));
        obj.insert("latitude".into(), serde_json::json!(p.latitude()));
        if let Some(h) = p.height() {
            obj.insert("height".into(), serde_json::json!(h));
        }
    }
    serde_json::Value::Object(obj)
}

pub(crate) fn json_value_to_params(
    value: serde_json::Value,
) -> Result<BTreeMap<String, LoraValue>> {
    match value {
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_cypher(v)?);
            }
            Ok(map)
        }
        _ => Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: params must be an object keyed by parameter name"),
        )),
    }
}

pub(crate) fn json_value_to_cypher(value: serde_json::Value) -> Result<LoraValue> {
    use serde_json::Value as J;
    match value {
        J::Null => Ok(LoraValue::Null),
        J::Bool(b) => Ok(LoraValue::Bool(b)),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LoraValue::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(LoraValue::Float(f))
            } else {
                Err(NapiError::new(
                    Status::InvalidArg,
                    format!("{INVALID_PARAMS_CODE}: unsupported numeric value"),
                ))
            }
        }
        J::String(s) => Ok(LoraValue::String(s)),
        J::Array(items) => {
            let list = items
                .into_iter()
                .map(json_value_to_cypher)
                .collect::<Result<Vec<_>>>()?;
            Ok(LoraValue::List(list))
        }
        J::Object(obj) => {
            if let Some(serde_json::Value::String(kind)) = obj.get("kind") {
                match kind.as_str() {
                    "date" => {
                        let iso = require_iso(&obj, "date")?;
                        let d = LoraDate::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::Date(d));
                    }
                    "time" => {
                        let iso = require_iso(&obj, "time")?;
                        let t = LoraTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::Time(t));
                    }
                    "localtime" => {
                        let iso = require_iso(&obj, "localtime")?;
                        let t = LoraLocalTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::LocalTime(t));
                    }
                    "datetime" => {
                        let iso = require_iso(&obj, "datetime")?;
                        let dt = LoraDateTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::DateTime(dt));
                    }
                    "localdatetime" => {
                        let iso = require_iso(&obj, "localdatetime")?;
                        let dt = LoraLocalDateTime::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::LocalDateTime(dt));
                    }
                    "duration" => {
                        let iso = require_iso(&obj, "duration")?;
                        let d = LoraDuration::parse(iso).map_err(invalid_param)?;
                        return Ok(LoraValue::Duration(d));
                    }
                    "point" => {
                        let srid = obj.get("srid").and_then(|v| v.as_u64()).unwrap_or(7203) as u32;
                        let x = obj
                            .get("x")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| invalid_param("point.x must be a number"))?;
                        let y = obj
                            .get("y")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| invalid_param("point.y must be a number"))?;
                        let z = obj.get("z").and_then(|v| v.as_f64());
                        return Ok(LoraValue::Point(LoraPoint { x, y, z, srid }));
                    }
                    "vector" => {
                        let v = vector_from_json_map(&obj).map_err(invalid_param)?;
                        return Ok(LoraValue::Vector(v));
                    }
                    "binary" | "blob" => {
                        return Ok(LoraValue::Binary(binary_from_json_map(&obj)?));
                    }
                    _ => {}
                }
            }
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_cypher(v)?);
            }
            Ok(LoraValue::Map(map))
        }
    }
}

fn binary_from_json_map(obj: &serde_json::Map<String, serde_json::Value>) -> Result<LoraBinary> {
    let segments = obj
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid_param("binary.segments must be an array of byte arrays"))?;
    let mut out = Vec::with_capacity(segments.len());
    for segment in segments {
        let bytes = segment
            .as_array()
            .ok_or_else(|| invalid_param("binary segment must be an array of bytes"))?;
        let mut chunk = Vec::with_capacity(bytes.len());
        for byte in bytes {
            let value = byte
                .as_u64()
                .ok_or_else(|| invalid_param("binary byte must be an integer 0..255"))?;
            let value = u8::try_from(value)
                .map_err(|_| invalid_param("binary byte must be an integer 0..255"))?;
            chunk.push(value);
        }
        out.push(chunk);
    }
    Ok(LoraBinary::from_segments(out))
}

fn require_iso<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    tag: &str,
) -> Result<&'a str> {
    match obj.get("iso").and_then(|v| v.as_str()) {
        Some(s) => Ok(s),
        None => Err(invalid_param(format!("{tag} value requires iso: string"))),
    }
}

pub(crate) fn invalid_param(msg: impl Into<String>) -> NapiError {
    NapiError::new(
        Status::InvalidArg,
        format!("{INVALID_PARAMS_CODE}: {}", msg.into()),
    )
}

/// Parse a tagged `{kind: "vector", dimension, coordinateType, values}`
/// map into a `LoraVector`. Used by every binding that accepts a vector
/// parameter — the validation rules are identical across bindings.
pub(crate) fn vector_from_json_map(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<LoraVector, String> {
    let dimension = obj
        .get("dimension")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "vector.dimension must be an integer".to_string())?;
    let coordinate_type_name = obj
        .get("coordinateType")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "vector.coordinateType must be a string".to_string())?;
    let coordinate_type = VectorCoordinateType::parse(coordinate_type_name)
        .ok_or_else(|| format!("unknown vector coordinate type '{coordinate_type_name}'"))?;
    let values = obj
        .get("values")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "vector.values must be an array of numbers".to_string())?;

    let mut raw = Vec::with_capacity(values.len());
    for v in values {
        match v {
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    raw.push(RawCoordinate::Int(i));
                } else if let Some(f) = n.as_f64() {
                    raw.push(RawCoordinate::Float(f));
                } else {
                    return Err("vector.values entries must be finite numbers".to_string());
                }
            }
            _ => return Err("vector.values entries must be numbers".to_string()),
        }
    }

    LoraVector::try_new(raw, dimension, coordinate_type).map_err(|e| e.to_string())
}
