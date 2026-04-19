#![deny(clippy::all)]

//! WebAssembly bindings for the Lora graph database.
//!
//! The Rust engine runs synchronously inside WASM; to keep JS hosts
//! responsive, the recommended execution path is via a Web Worker (browser)
//! or worker_thread (Node). The TS wrapper that ships alongside this crate
//! provides that architecture.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

use lora_database::{
    LoraValue, Database as InnerDatabase, ExecuteOptions, InMemoryGraph, QueryResult,
    ResultFormat,
};
use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime,
};

const LORA_ERROR_CODE: &str = "LORA_ERROR";
const INVALID_PARAMS_CODE: &str = "INVALID_PARAMS";

/// Call once at module start to install a panic hook that routes Rust
/// panics to `console.error`. No-op if compiled without the default feature.
#[wasm_bindgen(js_name = init)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// In-memory Lora graph database handle.
#[wasm_bindgen(js_name = WasmDatabase)]
pub struct WasmDatabase {
    store: Arc<Mutex<InMemoryGraph>>,
}

#[wasm_bindgen(js_class = WasmDatabase)]
impl WasmDatabase {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(InMemoryGraph::new())),
        }
    }

    /// Execute a Lora query. `params` may be `undefined`, `null`, or a
    /// plain object keyed by parameter name.
    ///
    /// Returns `{ columns: string[], rows: Array<Record<string, LoraValue>> }`
    /// as a plain JS object (structured-clonable).
    pub fn execute(&self, query: &str, params: JsValue) -> Result<JsValue, JsError> {
        let params_map = if params.is_undefined() || params.is_null() {
            BTreeMap::new()
        } else {
            let json_value: serde_json::Value = serde_wasm_bindgen::from_value(params)
                .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
            json_value_to_params(json_value)?
        };

        let db = InnerDatabase::new(Arc::clone(&self.store));
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };

        let result = db
            .execute_with_params(query, Some(options), params_map)
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;

        let QueryResult::RowArrays(row_arrays) = result else {
            return Err(js_error(LORA_ERROR_CODE, "expected RowArrays result"));
        };

        let columns_json: Vec<serde_json::Value> = row_arrays
            .columns
            .iter()
            .map(|c| serde_json::Value::String(c.clone()))
            .collect();

        let rows_json: Vec<serde_json::Value> = row_arrays
            .rows
            .iter()
            .map(|row| {
                let mut obj = serde_json::Map::with_capacity(row_arrays.columns.len());
                for (col, val) in row_arrays.columns.iter().zip(row.iter()) {
                    obj.insert(col.clone(), lora_value_to_json(val));
                }
                serde_json::Value::Object(obj)
            })
            .collect();

        let out = serde_json::json!({
            "columns": serde_json::Value::Array(columns_json),
            "rows": serde_json::Value::Array(rows_json),
        });

        // `json_compatible` emits plain JS objects (not Maps) so the result
        // survives `structuredClone` across the worker boundary.
        out.serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    pub fn clear(&self) {
        let mut guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        *guard = InMemoryGraph::new();
    }

    #[wasm_bindgen(js_name = nodeCount)]
    pub fn node_count(&self) -> u32 {
        use lora_store::GraphStorage;
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        guard.node_count() as u32
    }

    #[wasm_bindgen(js_name = relationshipCount)]
    pub fn relationship_count(&self) -> u32 {
        use lora_store::GraphStorage;
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        guard.relationship_count() as u32
    }
}

impl Default for WasmDatabase {
    fn default() -> Self {
        Self::new()
    }
}

// ===== serialization bridge =====

use serde::Serialize;

fn js_error(code: &str, message: &str) -> JsError {
    JsError::new(&format!("{code}: {message}"))
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
    }
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

fn json_value_to_params(value: serde_json::Value) -> Result<BTreeMap<String, LoraValue>, JsError> {
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
                        let d = LoraDate::parse(iso)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
                        return Ok(LoraValue::Date(d));
                    }
                    "time" => {
                        let iso = require_iso(&obj, "time")?;
                        let t = LoraTime::parse(iso)
                            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e))?;
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
                        let srid = obj
                            .get("srid")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(7203) as u32;
                        let x = obj
                            .get("x")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| js_error(INVALID_PARAMS_CODE, "point.x must be a number"))?;
                        let y = obj
                            .get("y")
                            .and_then(|v| v.as_f64())
                            .ok_or_else(|| js_error(INVALID_PARAMS_CODE, "point.y must be a number"))?;
                        let z = obj.get("z").and_then(|v| v.as_f64());
                        return Ok(LoraValue::Point(LoraPoint { x, y, z, srid }));
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
