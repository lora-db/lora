//! JSON ↔ `LoraValue` conversion shared by the WASM bindings.
//!
//! These helpers are pure Rust glue between `serde_json` / `JsValue`
//! and the engine's `LoraValue` model. They surface validation failures
//! through `JsError` so they can be returned directly from the
//! `#[wasm_bindgen]` methods in `lib.rs`. Kept in lockstep with the
//! Node binding — both packages emit and accept the same external
//! tagged value shapes.

use std::collections::BTreeMap;

use wasm_bindgen::prelude::*;

use lora_database::{
    snapshot_credentials_from_json, snapshot_options_from_json, LoraError, LoraValue, Row,
    SnapshotCredentials, SnapshotOptions, TransactionMode,
};
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
};

use super::INVALID_PARAMS_CODE;

pub(crate) fn js_error(code: &str, message: &str) -> JsError {
    JsError::new(&format!("{code}: {message}"))
}

/// Build a [`JsError`] from an engine-side `anyhow::Error`, using the
/// precise wire code from [`lora_database::LoraErrorCode`] as the
/// prefix so JS callers can route on it without parsing free-form
/// text.
pub(crate) fn js_error_from_anyhow(err: &anyhow::Error) -> JsError {
    let lora = LoraError::from_anyhow_ref(err);
    js_error(lora.code().as_str(), lora.message())
}

pub(crate) struct TransactionStatement {
    pub(crate) query: String,
    pub(crate) params: BTreeMap<String, LoraValue>,
}

pub(crate) fn parse_snapshot_options(value: JsValue) -> Result<SnapshotOptions, JsError> {
    let json = snapshot_js_value_to_json(value)?;
    snapshot_options_from_json(json).map_err(|e| {
        js_error(
            INVALID_PARAMS_CODE,
            &format!("invalid snapshot options: {e}"),
        )
    })
}

pub(crate) fn parse_snapshot_credentials(
    value: JsValue,
) -> Result<Option<SnapshotCredentials>, JsError> {
    let json = snapshot_js_value_to_json(value)?;
    snapshot_credentials_from_json(json).map_err(|e| {
        js_error(
            INVALID_PARAMS_CODE,
            &format!("invalid snapshot credentials: {e}"),
        )
    })
}

fn snapshot_js_value_to_json(value: JsValue) -> Result<Option<serde_json::Value>, JsError> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }

    serde_wasm_bindgen::from_value(value)
        .map(Some)
        .map_err(|e| {
            js_error(
                INVALID_PARAMS_CODE,
                &format!("invalid snapshot options: {e}"),
            )
        })
}

pub(crate) fn parse_transaction_mode(mode: Option<&str>) -> Result<TransactionMode, JsError> {
    match mode.unwrap_or("read_write") {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(js_error(
            INVALID_PARAMS_CODE,
            &format!("unknown transaction mode '{other}'"),
        )),
    }
}

pub(crate) fn parse_transaction_statements(
    value: serde_json::Value,
) -> Result<Vec<TransactionStatement>, JsError> {
    let serde_json::Value::Array(items) = value else {
        return Err(js_error(
            INVALID_PARAMS_CODE,
            "transaction statements must be an array",
        ));
    };

    items
        .into_iter()
        .map(|item| {
            let serde_json::Value::Object(mut obj) = item else {
                return Err(js_error(
                    INVALID_PARAMS_CODE,
                    "transaction statement must be an object",
                ));
            };
            let query = match obj.remove("query") {
                Some(serde_json::Value::String(query)) => query,
                _ => {
                    return Err(js_error(
                        INVALID_PARAMS_CODE,
                        "transaction statement requires query: string",
                    ));
                }
            };
            let params = match obj.remove("params") {
                None | Some(serde_json::Value::Null) => BTreeMap::new(),
                Some(other) => json_value_to_params(other)?,
            };
            Ok(TransactionStatement { query, params })
        })
        .collect()
}

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

fn lora_value_to_json(value: &LoraValue) -> serde_json::Value {
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
        VectorValues::Float64(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        VectorValues::Float32(vs) => serde_json::Value::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x as f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null)
                })
                .collect(),
        ),
        VectorValues::Integer64(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x)).collect())
        }
        VectorValues::Integer32(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        VectorValues::Integer16(vs) => {
            serde_json::Value::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        VectorValues::Integer8(vs) => {
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
/// by the shared TS `LoraPoint` union. See `lora-node::point_to_json` —
/// kept in sync so JS consumers see the same shape across both packages.
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
) -> Result<BTreeMap<String, LoraValue>, JsError> {
    match value {
        serde_json::Value::Object(obj) => {
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, json_value_to_cypher(v)?);
            }
            Ok(map)
        }
        _ => Err(js_error(
            INVALID_PARAMS_CODE,
            "params must be an object keyed by parameter name",
        )),
    }
}

fn json_value_to_cypher(value: serde_json::Value) -> Result<LoraValue, JsError> {
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
                Err(js_error(INVALID_PARAMS_CODE, "unsupported numeric value"))
            }
        }
        J::String(s) => Ok(LoraValue::String(s)),
        J::Array(items) => {
            let list = items
                .into_iter()
                .map(json_value_to_cypher)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LoraValue::List(list))
        }
        J::Object(obj) => {
            if let Some(serde_json::Value::String(kind)) = obj.get("kind") {
                match kind.as_str() {
                    "date" => {
                        let iso = require_iso(&obj, "date")?;
                        let d =
                            LoraDate::parse(iso).map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::Date(d));
                    }
                    "time" => {
                        let iso = require_iso(&obj, "time")?;
                        let t =
                            LoraTime::parse(iso).map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::Time(t));
                    }
                    "localtime" => {
                        let iso = require_iso(&obj, "localtime")?;
                        let t = LoraLocalTime::parse(iso)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::LocalTime(t));
                    }
                    "datetime" => {
                        let iso = require_iso(&obj, "datetime")?;
                        let dt = LoraDateTime::parse(iso)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::DateTime(dt));
                    }
                    "localdatetime" => {
                        let iso = require_iso(&obj, "localdatetime")?;
                        let dt = LoraLocalDateTime::parse(iso)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::LocalDateTime(dt));
                    }
                    "duration" => {
                        let iso = require_iso(&obj, "duration")?;
                        let d = LoraDuration::parse(iso)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::Duration(d));
                    }
                    "point" => {
                        let srid = obj.get("srid").and_then(|v| v.as_u64()).unwrap_or(7203) as u32;
                        let x = obj.get("x").and_then(|v| v.as_f64()).ok_or_else(|| {
                            js_error(INVALID_PARAMS_CODE, "point.x must be a number")
                        })?;
                        let y = obj.get("y").and_then(|v| v.as_f64()).ok_or_else(|| {
                            js_error(INVALID_PARAMS_CODE, "point.y must be a number")
                        })?;
                        let z = obj.get("z").and_then(|v| v.as_f64());
                        return Ok(LoraValue::Point(LoraPoint { x, y, z, srid }));
                    }
                    "vector" => {
                        let v = vector_from_json_map(&obj)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::Vector(v));
                    }
                    "binary" | "blob" => {
                        let v = binary_from_json_map(&obj)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::Binary(v));
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

fn binary_from_json_map(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<LoraBinary, String> {
    let segments = obj
        .get("segments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "binary.segments must be an array of byte arrays".to_string())?;
    let mut out = Vec::with_capacity(segments.len());
    for segment in segments {
        let values = segment
            .as_array()
            .ok_or_else(|| "binary segment must be an array of bytes".to_string())?;
        let mut chunk = Vec::with_capacity(values.len());
        for value in values {
            let byte = value
                .as_u64()
                .ok_or_else(|| "binary byte must be an integer 0..255".to_string())?;
            chunk.push(
                u8::try_from(byte)
                    .map_err(|_| "binary byte must be an integer 0..255".to_string())?,
            );
        }
        out.push(chunk);
    }
    Ok(LoraBinary::from_segments(out))
}

fn require_iso<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    tag: &str,
) -> Result<&'a str, JsError> {
    match obj.get("iso").and_then(|v| v.as_str()) {
        Some(s) => Ok(s),
        None => Err(js_error(
            INVALID_PARAMS_CODE,
            &format!("{tag} value requires iso: string"),
        )),
    }
}

/// Parse a tagged `{kind: "vector", dimension, coordinateType, values}`
/// map into a `LoraVector`. Kept in lockstep with the Node binding —
/// both use the same JSON value model.
fn vector_from_json_map(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Result<LoraVector, String> {
    let dimension = obj
        .get("dimension")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| "vector.dimension must be an integer".to_string())?;
    let coordinate_type_name = obj
        .get("coordinateType")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "vector.coordinateType must be a string".to_string())?;
    let coordinate_type = VectorCoordinateType::parse(coordinate_type_name)
        .ok_or_else(|| format!("unknown vector coordinate type `{coordinate_type_name}`"))?;
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
