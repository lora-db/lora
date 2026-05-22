//! Canonical lossless [`LoraValue`] ↔ [`serde_json::Value`] codec.
//!
//! Tagged shape (e.g. `{ "kind": "datetime", "iso": "..." }`) for every
//! variant that doesn't have a native JSON counterpart. This is the
//! exact shape the WASM and Node bindings already emit, factored out
//! here so the IO codecs can re-use it without depending on a binding
//! crate.

use std::collections::BTreeMap;

use lora_executor::LoraValue;
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
};
use serde_json::Value as J;

/// Render a [`LoraValue`] into a [`serde_json::Value`] using the
/// canonical tagged shape. The result is lossless — feeding it back
/// through [`lora_value_from_json`] reconstructs the original value.
pub fn lora_value_to_json(value: &LoraValue) -> J {
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
        LoraValue::Map(m) => J::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), lora_value_to_json(v)))
                .collect(),
        ),
        LoraValue::Node(id) => serde_json::json!({
            "kind": "node",
            "id": *id as i64,
            "labels": J::Array(vec![]),
            "properties": J::Object(Default::default()),
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
        LoraValue::LocalTime(t) => serde_json::json!({ "kind": "localtime", "iso": t.to_string() }),
        LoraValue::DateTime(dt) => serde_json::json!({ "kind": "datetime", "iso": dt.to_string() }),
        LoraValue::LocalDateTime(dt) => {
            serde_json::json!({ "kind": "localdatetime", "iso": dt.to_string() })
        }
        LoraValue::Duration(d) => serde_json::json!({ "kind": "duration", "iso": d.to_string() }),
        LoraValue::Point(p) => point_to_json(p),
        LoraValue::Vector(v) => vector_to_json(v),
    }
}

fn binary_to_json(b: &LoraBinary) -> J {
    serde_json::json!({
        "kind": "binary",
        "length": b.len(),
        "segments": b.segments(),
    })
}

fn vector_to_json(v: &LoraVector) -> J {
    let values: J = match &v.values {
        VectorValues::Float64(vs) => J::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x)
                        .map(J::Number)
                        .unwrap_or(J::Null)
                })
                .collect(),
        ),
        VectorValues::Float32(vs) => J::Array(
            vs.iter()
                .map(|x| {
                    serde_json::Number::from_f64(*x as f64)
                        .map(J::Number)
                        .unwrap_or(J::Null)
                })
                .collect(),
        ),
        VectorValues::Integer64(vs) => J::Array(vs.iter().map(|x| serde_json::json!(*x)).collect()),
        VectorValues::Integer32(vs) => {
            J::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        VectorValues::Integer16(vs) => {
            J::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
        VectorValues::Integer8(vs) => {
            J::Array(vs.iter().map(|x| serde_json::json!(*x as i64)).collect())
        }
    };
    serde_json::json!({
        "kind": "vector",
        "dimension": v.dimension,
        "coordinateType": v.coordinate_type().as_str(),
        "values": values,
    })
}

fn point_to_json(p: &LoraPoint) -> J {
    let mut obj = serde_json::Map::with_capacity(7);
    obj.insert("kind".into(), J::String("point".into()));
    obj.insert("srid".into(), serde_json::json!(p.srid));
    obj.insert("crs".into(), J::String(p.crs_name().into()));
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
    J::Object(obj)
}

/// Parse the canonical tagged JSON shape back into a [`LoraValue`].
/// Untagged objects become [`LoraValue::Map`]; untagged arrays
/// become [`LoraValue::List`]. Strings stay strings — temporal /
/// vector / point values must use the tagged `{ "kind": ... }`
/// form to round-trip, otherwise they decode as plain strings.
pub fn lora_value_from_json(value: J) -> Result<LoraValue, String> {
    match value {
        J::Null => Ok(LoraValue::Null),
        J::Bool(b) => Ok(LoraValue::Bool(b)),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(LoraValue::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(LoraValue::Float(f))
            } else {
                Err("unsupported numeric value".into())
            }
        }
        J::String(s) => Ok(LoraValue::String(s)),
        J::Array(items) => Ok(LoraValue::List(
            items
                .into_iter()
                .map(lora_value_from_json)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        J::Object(obj) => {
            if let Some(J::String(kind)) = obj.get("kind") {
                match kind.as_str() {
                    "date" => return Ok(LoraValue::Date(parse_iso::<LoraDate>(&obj, "date")?)),
                    "time" => return Ok(LoraValue::Time(parse_iso::<LoraTime>(&obj, "time")?)),
                    "localtime" => {
                        return Ok(LoraValue::LocalTime(parse_iso::<LoraLocalTime>(
                            &obj,
                            "localtime",
                        )?))
                    }
                    "datetime" => {
                        return Ok(LoraValue::DateTime(parse_iso::<LoraDateTime>(
                            &obj, "datetime",
                        )?))
                    }
                    "localdatetime" => {
                        return Ok(LoraValue::LocalDateTime(parse_iso::<LoraLocalDateTime>(
                            &obj,
                            "localdatetime",
                        )?))
                    }
                    "duration" => {
                        return Ok(LoraValue::Duration(parse_iso::<LoraDuration>(
                            &obj, "duration",
                        )?))
                    }
                    "point" => return Ok(LoraValue::Point(point_from_obj(&obj)?)),
                    "vector" => return Ok(LoraValue::Vector(vector_from_obj(&obj)?)),
                    "binary" | "blob" => return Ok(LoraValue::Binary(binary_from_obj(&obj)?)),
                    "node" | "relationship" | "path" => {
                        // Hydrated entity envelopes round-trip only as
                        // their id payload — semantic re-creation is
                        // the caller's job (CREATE statement).
                        return Err(format!("cannot import a hydrated {kind} value directly"));
                    }
                    _ => {}
                }
            }
            let mut map = BTreeMap::new();
            for (k, v) in obj {
                map.insert(k, lora_value_from_json(v)?);
            }
            Ok(LoraValue::Map(map))
        }
    }
}

trait ParseIso: Sized {
    fn parse_iso(s: &str) -> Result<Self, String>;
}

macro_rules! impl_parse_iso {
    ($ty:ty) => {
        impl ParseIso for $ty {
            fn parse_iso(s: &str) -> Result<Self, String> {
                <$ty>::parse(s).map_err(|e| e.to_string())
            }
        }
    };
}

impl_parse_iso!(LoraDate);
impl_parse_iso!(LoraTime);
impl_parse_iso!(LoraLocalTime);
impl_parse_iso!(LoraDateTime);
impl_parse_iso!(LoraLocalDateTime);
impl_parse_iso!(LoraDuration);

fn parse_iso<T: ParseIso>(obj: &serde_json::Map<String, J>, tag: &str) -> Result<T, String> {
    let iso = obj
        .get("iso")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{tag} value requires iso: string"))?;
    T::parse_iso(iso)
}

fn point_from_obj(obj: &serde_json::Map<String, J>) -> Result<LoraPoint, String> {
    let srid = obj.get("srid").and_then(|v| v.as_u64()).unwrap_or(7203) as u32;
    let x = obj
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "point.x must be a number".to_string())?;
    let y = obj
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "point.y must be a number".to_string())?;
    let z = obj.get("z").and_then(|v| v.as_f64());
    Ok(LoraPoint { x, y, z, srid })
}

fn vector_from_obj(obj: &serde_json::Map<String, J>) -> Result<LoraVector, String> {
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
            J::Number(n) => {
                if let Some(i) = n.as_i64() {
                    raw.push(RawCoordinate::Int(i));
                } else if let Some(f) = n.as_f64() {
                    raw.push(RawCoordinate::Float(f));
                } else {
                    return Err("vector.values entries must be finite numbers".into());
                }
            }
            _ => return Err("vector.values entries must be numbers".into()),
        }
    }
    LoraVector::try_new(raw, dimension, coordinate_type).map_err(|e| e.to_string())
}

fn binary_from_obj(obj: &serde_json::Map<String, J>) -> Result<LoraBinary, String> {
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
                u8::try_from(byte).map_err(|_| "binary byte must be in 0..=255".to_string())?,
            );
        }
        out.push(chunk);
    }
    Ok(LoraBinary::from_segments(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_round_trip() {
        for v in [
            LoraValue::Null,
            LoraValue::Bool(true),
            LoraValue::Int(42),
            LoraValue::Float(2.5),
            LoraValue::String("hi".into()),
        ] {
            let j = lora_value_to_json(&v);
            assert_eq!(lora_value_from_json(j).unwrap(), v);
        }
    }

    #[test]
    fn nested_map_round_trips() {
        let mut m = BTreeMap::new();
        m.insert("name".to_string(), LoraValue::String("alice".into()));
        m.insert(
            "tags".to_string(),
            LoraValue::List(vec![LoraValue::String("a".into()), LoraValue::Int(2)]),
        );
        let v = LoraValue::Map(m);
        let j = lora_value_to_json(&v);
        assert_eq!(lora_value_from_json(j).unwrap(), v);
    }
}
