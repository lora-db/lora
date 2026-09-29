//! JSON → `LoraValue` parsing for query parameters.
//!
//! Inputs (params, transaction statements, snapshot options) cross the
//! napi boundary as `serde_json::Value` and are parsed here on the
//! libuv worker. Outputs (rows, plans, profiles) skip JSON entirely
//! and are built directly as napi values in [`crate::to_napi`].

use std::collections::BTreeMap;

use napi::bindgen_prelude::Result;
use napi::{Error as NapiError, Status};

use lora_database::LoraValue;
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
    LoraVector, RawCoordinate, VectorCoordinateType,
};

use super::INVALID_PARAMS_CODE;

/// `Number.MAX_SAFE_INTEGER` (2^53 - 1).
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

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
                // A JS number has 53 bits of integer precision. An
                // integer-valued number beyond that has already been
                // rounded by JS; storing it would silently corrupt the
                // value, so refuse it and point at bigint.
                if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i) {
                    return Err(NapiError::new(
                        Status::InvalidArg,
                        format!(
                            "{INVALID_PARAMS_CODE}: integer {i} is outside the JavaScript safe-integer range and may already be rounded; pass it as a bigint"
                        ),
                    ));
                }
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
                    // Exact 64-bit integer, produced by the TS wrapper for
                    // `bigint` params (`{ kind: "integer", value: "<decimal>" }`).
                    "integer" => {
                        let raw = obj.get("value").and_then(|v| v.as_str()).ok_or_else(|| {
                            NapiError::new(
                                Status::InvalidArg,
                                format!(
                                    "{INVALID_PARAMS_CODE}: integer.value must be a decimal string"
                                ),
                            )
                        })?;
                        let i: i64 = raw.parse().map_err(|_| {
                            NapiError::new(
                                Status::InvalidArg,
                                format!(
                                    "{INVALID_PARAMS_CODE}: integer {raw} does not fit in a signed 64-bit integer"
                                ),
                            )
                        })?;
                        return Ok(LoraValue::Int(i));
                    }
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
                        let coords = lora_store::NamedPointCoordinates {
                            srid: obj.get("srid").and_then(|v| v.as_u64()).map(|v| v as u32),
                            x: obj.get("x").and_then(|v| v.as_f64()),
                            y: obj.get("y").and_then(|v| v.as_f64()),
                            z: obj.get("z").and_then(|v| v.as_f64()),
                            longitude: obj.get("longitude").and_then(|v| v.as_f64()),
                            latitude: obj.get("latitude").and_then(|v| v.as_f64()),
                            height: obj.get("height").and_then(|v| v.as_f64()),
                        };
                        let point = coords.resolve().map_err(invalid_param)?;
                        return Ok(LoraValue::Point(point));
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
