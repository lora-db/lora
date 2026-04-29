#![deny(clippy::all)]

//! WebAssembly bindings for the Lora graph database.
//!
//! The Rust engine runs synchronously inside WASM; to keep JS hosts
//! responsive, the recommended execution path is via a Web Worker (browser)
//! or worker_thread (Node). The TS wrapper that ships alongside this crate
//! provides that architecture.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

use lora_database::{
    snapshot_credentials_from_json, snapshot_options_from_json, Database as InnerDatabase,
    ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat, Row, SnapshotCredentials,
    SnapshotOptions, TransactionMode,
};
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
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
    db: Arc<InnerDatabase<InMemoryGraph>>,
}

#[wasm_bindgen(js_class = WasmDatabase)]
impl WasmDatabase {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            db: Arc::new(InnerDatabase::in_memory()),
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

        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };

        let result = self
            .db
            .execute_with_params(query, Some(options), params_map)
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;

        let QueryResult::RowArrays(row_arrays) = result else {
            return Err(js_error(LORA_ERROR_CODE, "expected RowArrays result"));
        };

        let out = serialize_rows(&row_arrays.columns, &row_arrays.rows);

        // `json_compatible` emits plain JS objects (not Maps) so the result
        // survives `structuredClone` across the worker boundary.
        out.serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    /// Open a true native row stream. Rows are pulled from the Rust executor
    /// one `next()` call at a time.
    #[wasm_bindgen(js_name = openStream)]
    pub fn open_stream(&self, query: &str, params: JsValue) -> Result<WasmQueryStream, JsError> {
        let params_map = if params.is_undefined() || params.is_null() {
            BTreeMap::new()
        } else {
            let json_value: serde_json::Value = serde_wasm_bindgen::from_value(params)
                .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
            json_value_to_params(json_value)?
        };
        let stream = unsafe { self.db.stream_with_params_owned(query, params_map) }
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;
        Ok(WasmQueryStream {
            _db: self.db.clone(),
            stream: Some(stream),
        })
    }

    /// Execute an array of `{ query, params? }` statements inside one native
    /// transaction. Returns an array of query results in statement order.
    #[wasm_bindgen(js_name = transaction)]
    pub fn transaction(
        &self,
        statements: JsValue,
        mode: Option<String>,
    ) -> Result<JsValue, JsError> {
        let json_value: serde_json::Value = serde_wasm_bindgen::from_value(statements)
            .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
        let statements = parse_transaction_statements(json_value)?;
        let mode = parse_transaction_mode(mode.as_deref())?;
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };
        let mut tx = self
            .db
            .begin_transaction(mode)
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;

        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = tx
                .execute_with_params(&statement.query, Some(options), statement.params)
                .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(js_error(LORA_ERROR_CODE, "expected RowArrays result"));
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }

        tx.commit()
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;

        serde_json::Value::Array(results)
            .serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    pub fn clear(&self) {
        self.db.clear();
    }

    #[wasm_bindgen(js_name = nodeCount)]
    pub fn node_count(&self) -> u32 {
        self.db.node_count() as u32
    }

    #[wasm_bindgen(js_name = relationshipCount)]
    pub fn relationship_count(&self) -> u32 {
        self.db.relationship_count() as u32
    }

    /// Serialize the graph into database snapshot bytes. The caller is
    /// responsible for writing them to IndexedDB, localStorage, a server, or
    /// another host-provided store — WASM has no direct filesystem access.
    /// The returned bytes can later be passed to `loadSnapshot` on any
    /// `WasmDatabase` instance.
    ///
    /// Returns the serialized bytes as a `Uint8Array`.
    #[wasm_bindgen(js_name = saveSnapshot)]
    pub fn save_snapshot(&self, options: JsValue) -> Result<Vec<u8>, JsError> {
        let options = parse_snapshot_options(options)?;
        self.db
            .save_snapshot_to_bytes_with_options(&options)
            .map(|(bytes, _)| bytes)
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))
    }

    /// Replace the graph state with a database snapshot decoded from `bytes`.
    /// Legacy store snapshot bytes are accepted for compatibility.
    ///
    /// Returns a plain object matching the shape of `SnapshotMeta`:
    /// `{ formatVersion, nodeCount, relationshipCount, walLsn }`.
    #[wasm_bindgen(js_name = loadSnapshot)]
    pub fn load_snapshot(&self, bytes: Vec<u8>, options: JsValue) -> Result<JsValue, JsError> {
        let credentials = parse_snapshot_credentials(options)?;
        let meta = self
            .db
            .load_snapshot_from_bytes_with_credentials(bytes.as_slice(), credentials.as_ref())
            .map_err(|e| js_error(LORA_ERROR_CODE, &format!("{e}")))?;

        let out = serde_json::json!({
            "formatVersion": meta.format_version,
            "nodeCount": meta.node_count as u64,
            "relationshipCount": meta.relationship_count as u64,
            "walLsn": meta.wal_lsn,
        });
        out.serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }
}

impl Default for WasmDatabase {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen(js_name = WasmQueryStream)]
pub struct WasmQueryStream {
    _db: Arc<InnerDatabase<InMemoryGraph>>,
    stream: Option<lora_database::QueryStream<'static>>,
}

#[wasm_bindgen(js_class = WasmQueryStream)]
impl WasmQueryStream {
    pub fn columns(&self) -> Result<JsValue, JsError> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| js_error(LORA_ERROR_CODE, "query stream is closed"))?;
        stream
            .columns()
            .to_vec()
            .serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<JsValue, JsError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| js_error(LORA_ERROR_CODE, "query stream is closed"))?;
        match stream.next_row() {
            Ok(Some(row)) => row_to_json(&row)
                .serialize(&Serializer::json_compatible())
                .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string())),
            Ok(None) => {
                self.stream.take();
                Ok(JsValue::NULL)
            }
            Err(e) => {
                self.stream.take();
                Err(js_error(LORA_ERROR_CODE, &format!("{e}")))
            }
        }
    }

    pub fn close(&mut self) {
        self.stream.take();
    }
}

// ===== serialization bridge =====

fn js_error(code: &str, message: &str) -> JsError {
    JsError::new(&format!("{code}: {message}"))
}

struct TransactionStatement {
    query: String,
    params: BTreeMap<String, LoraValue>,
}

fn parse_snapshot_options(value: JsValue) -> Result<SnapshotOptions, JsError> {
    let json = snapshot_js_value_to_json(value)?;
    snapshot_options_from_json(json).map_err(|e| {
        js_error(
            INVALID_PARAMS_CODE,
            &format!("invalid snapshot options: {e}"),
        )
    })
}

fn parse_snapshot_credentials(value: JsValue) -> Result<Option<SnapshotCredentials>, JsError> {
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

fn parse_transaction_mode(mode: Option<&str>) -> Result<TransactionMode, JsError> {
    match mode.unwrap_or("read_write") {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(js_error(
            INVALID_PARAMS_CODE,
            &format!("unknown transaction mode '{other}'"),
        )),
    }
}

fn parse_transaction_statements(
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

fn serialize_rows(columns: &[String], rows: &[Vec<LoraValue>]) -> serde_json::Value {
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

fn row_to_json(row: &Row) -> serde_json::Value {
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
