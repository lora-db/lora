#![deny(clippy::all)]

//! Node.js N-API bindings for the Lora graph database.
//!
//! Query execution runs on the libuv threadpool via [`napi::Task`] so the
//! JS main thread (event loop) stays responsive for the duration of a
//! query. The JS `execute()` method returns a real Promise backed by an
//! `AsyncTask`; parameter parsing, query planning, execution and result
//! serialisation all happen on a worker thread.
//!
//! `clear()`, `nodeCount()`, `relationshipCount()` stay synchronous —
//! they are constant-time lock-and-read operations and the cost of a
//! thread hop would dominate the useful work.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi::{Env, Error as NapiError, JsUnknown, Status, Task};
use napi_derive::napi;

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
    Row, Snapshotable, TransactionMode, WalConfig,
};
use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, RawCoordinate, VectorCoordinateType,
};

const LORA_ERROR_CODE: &str = "LORA_ERROR";
const INVALID_PARAMS_CODE: &str = "INVALID_PARAMS";

/// Lora graph database handle exposed to Node.
///
/// Wraps an `Arc<Database<InMemoryGraph>>`; the same handle is cloned
/// onto the libuv threadpool for each `execute()` call. Multiple
/// concurrent queries against the same `Database` can share read-only
/// work; writes serialize on the inner store's write lock without
/// blocking the JS event loop.
///
/// With no constructor arg the database is purely in-memory. Passing a
/// WAL directory path enables write-ahead logging: the binding opens or
/// creates the WAL there, replays committed writes on boot, and then
/// serves queries against the recovered graph.
#[napi]
pub struct Database {
    db: Mutex<Option<Arc<InnerDatabase<InMemoryGraph>>>>,
}

#[napi]
impl Database {
    /// Construct a database.
    ///
    /// - `undefined` / `null` => fresh in-memory graph.
    /// - `string` => WAL-backed graph rooted at that directory.
    #[napi(constructor)]
    pub fn new(
        #[napi(ts_arg_type = "string | null | undefined")] wal_dir: Option<String>,
    ) -> Result<Self> {
        let db = match wal_dir {
            None => InnerDatabase::in_memory(),
            Some(dir) => InnerDatabase::open_with_wal(WalConfig::enabled(dir))
                .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?,
        };
        Ok(Self {
            db: Mutex::new(Some(Arc::new(db))),
        })
    }

    /// Execute a Lora query on the libuv threadpool.
    ///
    /// The returned JS Promise resolves with `{ columns, rows }`. Values
    /// follow the shared `LoraValue` TypeScript union: primitives pass
    /// through, nodes / relationships / paths carry a `kind` discriminator,
    /// and temporal / spatial values are tagged objects.
    ///
    /// Errors surface as `LoraError` in the TS wrapper with a narrowed
    /// `code` (`LORA_ERROR`, `INVALID_PARAMS`).
    #[napi(ts_return_type = "Promise<{ columns: string[]; rows: Array<Record<string, any>> }>")]
    pub fn execute(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
    ) -> Result<AsyncTask<ExecuteTask>> {
        Ok(AsyncTask::new(ExecuteTask {
            db: self.inner()?,
            query,
            params,
        }))
    }

    /// Open a true native row stream.
    ///
    /// The returned handle owns the Rust `QueryStream`, so rows are pulled
    /// from the executor one `next()` call at a time instead of materializing
    /// the whole result up front.
    #[napi(ts_return_type = "QueryStream")]
    pub fn open_stream(
        &self,
        query: String,
        #[napi(ts_arg_type = "Record<string, any> | null | undefined")] params: Option<
            serde_json::Value,
        >,
    ) -> Result<NativeQueryStream> {
        let params_map = match params {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => json_value_to_params(other)?,
        };
        let db = self.inner()?;
        let stream = unsafe { db.stream_with_params_owned(&query, params_map) }
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(NativeQueryStream {
            _db: db,
            stream: Mutex::new(Some(stream)),
        })
    }

    /// Execute multiple statements inside one core transaction.
    ///
    /// `statements` is an array of `{ query, params? }` objects. Results are
    /// returned in statement order. If any statement fails, the transaction is
    /// rolled back by dropping the native transaction before commit.
    #[napi(
        ts_return_type = "Promise<Array<{ columns: string[]; rows: Array<Record<string, any>> }>>"
    )]
    pub fn transaction(
        &self,
        #[napi(ts_arg_type = "Array<{ query: string; params?: Record<string, any> | null }>")]
        statements: serde_json::Value,
        #[napi(
            ts_arg_type = "\"read_write\" | \"read_only\" | \"readwrite\" | \"readonly\" | null | undefined"
        )]
        mode: Option<String>,
    ) -> Result<AsyncTask<TransactionTask>> {
        Ok(AsyncTask::new(TransactionTask {
            db: self.inner()?,
            statements,
            mode,
        }))
    }

    /// Drop every node and relationship, returning the database to an empty
    /// state. Useful for test isolation. Synchronous — constant-time.
    #[napi]
    pub fn clear(&self) -> Result<()> {
        self.inner()?.clear();
        Ok(())
    }

    /// Number of nodes in the graph. Synchronous.
    #[napi]
    pub fn node_count(&self) -> Result<u32> {
        Ok(self.inner()?.node_count() as u32)
    }

    /// Number of relationships in the graph. Synchronous.
    #[napi]
    pub fn relationship_count(&self) -> Result<u32> {
        Ok(self.inner()?.relationship_count() as u32)
    }

    /// Release the native database handle. Idempotent.
    ///
    /// Any query already dispatched to the libuv threadpool keeps its cloned
    /// handle until it finishes; new operations fail with `database is closed`.
    #[napi]
    pub fn dispose(&self) -> Result<()> {
        let mut slot = self
            .db
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, closed_error_message()))?;
        slot.take();
        Ok(())
    }

    /// Save the graph to a snapshot file. Atomic: the target is only
    /// replaced once the whole payload has been written + fsync'd.
    /// Synchronous — snapshots are usually infrequent and running on the
    /// event loop dodges the cost of a thread hop for small graphs.
    #[napi(
        ts_return_type = "{ formatVersion: number; nodeCount: number; relationshipCount: number; walLsn: number | null }"
    )]
    pub fn save_snapshot(&self, path: String) -> Result<serde_json::Value> {
        let meta = self
            .inner()?
            .save_snapshot_to(&path)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(snapshot_meta_to_json(meta))
    }

    /// Serialize the current graph into snapshot bytes.
    #[napi(ts_return_type = "Buffer")]
    pub fn save_snapshot_to_bytes(&self) -> Result<Buffer> {
        let mut buf = Vec::new();
        self.inner()?
            .with_store(|store| store.save_snapshot(&mut buf))
            .map_err(|e| NapiError::new(Status::GenericFailure, e.to_string()))?;
        Ok(Buffer::from(buf))
    }

    /// Replace the current graph state with a snapshot loaded from disk.
    #[napi(
        ts_return_type = "{ formatVersion: number; nodeCount: number; relationshipCount: number; walLsn: number | null }"
    )]
    pub fn load_snapshot(&self, path: String) -> Result<serde_json::Value> {
        let meta = self
            .inner()?
            .load_snapshot_from(&path)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
        Ok(snapshot_meta_to_json(meta))
    }

    /// Replace the current graph state with a snapshot loaded from bytes.
    #[napi(
        ts_return_type = "{ formatVersion: number; nodeCount: number; relationshipCount: number; walLsn: number | null }"
    )]
    pub fn load_snapshot_from_bytes(
        &self,
        #[napi(ts_arg_type = "Uint8Array | Buffer")] bytes: Buffer,
    ) -> Result<serde_json::Value> {
        let meta = self
            .inner()?
            .with_store_mut(|store| store.load_snapshot(bytes.as_ref()))
            .map_err(|e| NapiError::new(Status::GenericFailure, e.to_string()))?;
        Ok(snapshot_meta_to_json(meta))
    }

    fn inner(&self) -> Result<Arc<InnerDatabase<InMemoryGraph>>> {
        let slot = self
            .db
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, closed_error_message()))?;
        slot.as_ref()
            .cloned()
            .ok_or_else(|| NapiError::new(Status::GenericFailure, closed_error_message()))
    }
}

fn snapshot_meta_to_json(meta: lora_database::SnapshotMeta) -> serde_json::Value {
    serde_json::json!({
        "formatVersion": meta.format_version,
        "nodeCount": meta.node_count as u64,
        "relationshipCount": meta.relationship_count as u64,
        "walLsn": meta.wal_lsn,
    })
}

impl Default for Database {
    fn default() -> Self {
        Self::new(None).expect("in-memory Database::default should not fail")
    }
}

#[napi(js_name = "QueryStream")]
pub struct NativeQueryStream {
    _db: Arc<InnerDatabase<InMemoryGraph>>,
    stream: Mutex<Option<lora_database::QueryStream<'static>>>,
}

#[napi]
impl NativeQueryStream {
    #[napi(ts_return_type = "string[]")]
    pub fn columns(&self) -> Result<Vec<String>> {
        let guard = self
            .stream
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream lock poisoned"))?;
        let stream = guard
            .as_ref()
            .ok_or_else(|| NapiError::new(Status::GenericFailure, "query stream is closed"))?;
        Ok(stream.columns().to_vec())
    }

    #[napi(ts_return_type = "Record<string, any> | null")]
    pub fn next(&self) -> Result<Option<serde_json::Value>> {
        let mut guard = self
            .stream
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream lock poisoned"))?;
        let stream = guard
            .as_mut()
            .ok_or_else(|| NapiError::new(Status::GenericFailure, "query stream is closed"))?;
        match stream.next_row() {
            Ok(Some(row)) => Ok(Some(row_to_json(&row))),
            Ok(None) => {
                guard.take();
                Ok(None)
            }
            Err(e) => {
                guard.take();
                Err(NapiError::new(
                    Status::GenericFailure,
                    format!("{LORA_ERROR_CODE}: {e}"),
                ))
            }
        }
    }

    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut guard = self
            .stream
            .lock()
            .map_err(|_| NapiError::new(Status::GenericFailure, "stream lock poisoned"))?;
        guard.take();
        Ok(())
    }
}

// ============================================================================
// Threadpool task
// ============================================================================

/// Work unit for `Database.execute`. Owns its inputs so it can move onto the
/// libuv worker pool and run without touching the JS main thread until it
/// resolves the Promise with the serialised `{columns, rows}` payload.
pub struct ExecuteTask {
    db: Arc<InnerDatabase<InMemoryGraph>>,
    query: String,
    params: Option<serde_json::Value>,
}

impl Task for ExecuteTask {
    type Output = serde_json::Value;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        // Parse params here (on the worker thread) so param-validation errors
        // surface as Promise rejections, not synchronous throws. Matches the
        // lora-wasm semantics.
        let params_map = match self.params.take() {
            None | Some(serde_json::Value::Null) => BTreeMap::new(),
            Some(other) => json_value_to_params(other)?,
        };

        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };

        let result = self
            .db
            .execute_with_params(&self.query, Some(options), params_map)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;

        let QueryResult::RowArrays(row_arrays) = result else {
            return Err(NapiError::new(
                Status::GenericFailure,
                "expected RowArrays result".to_string(),
            ));
        };

        Ok(serialize_rows(&row_arrays.columns, &row_arrays.rows))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        // `serde-json` feature on napi bridges serde_json::Value → JS objects.
        env.to_js_value(&output)
    }
}

pub struct TransactionTask {
    db: Arc<InnerDatabase<InMemoryGraph>>,
    statements: serde_json::Value,
    mode: Option<String>,
}

impl Task for TransactionTask {
    type Output = serde_json::Value;
    type JsValue = JsUnknown;

    fn compute(&mut self) -> Result<Self::Output> {
        let mode = parse_transaction_mode(self.mode.as_deref())?;
        let statements = parse_transaction_statements(std::mem::take(&mut self.statements))?;
        let options = ExecuteOptions {
            format: ResultFormat::RowArrays,
        };
        let mut tx = self
            .db
            .begin_transaction(mode)
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;

        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = tx
                .execute_with_params(&statement.query, Some(options), statement.params)
                .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(NapiError::new(
                    Status::GenericFailure,
                    "expected RowArrays result".to_string(),
                ));
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }

        tx.commit()
            .map_err(|e| NapiError::new(Status::GenericFailure, format_error(&e)))?;

        Ok(serde_json::Value::Array(results))
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        env.to_js_value(&output)
    }
}

struct TransactionStatement {
    query: String,
    params: BTreeMap<String, LoraValue>,
}

fn parse_transaction_mode(mode: Option<&str>) -> Result<TransactionMode> {
    match mode.unwrap_or("read_write") {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: unknown transaction mode '{other}'"),
        )),
    }
}

fn parse_transaction_statements(value: serde_json::Value) -> Result<Vec<TransactionStatement>> {
    let serde_json::Value::Array(items) = value else {
        return Err(NapiError::new(
            Status::InvalidArg,
            format!("{INVALID_PARAMS_CODE}: transaction statements must be an array"),
        ));
    };

    items
        .into_iter()
        .map(|item| {
            let serde_json::Value::Object(mut obj) = item else {
                return Err(NapiError::new(
                    Status::InvalidArg,
                    format!("{INVALID_PARAMS_CODE}: transaction statement must be an object"),
                ));
            };
            let query = match obj.remove("query") {
                Some(serde_json::Value::String(query)) => query,
                _ => {
                    return Err(NapiError::new(
                        Status::InvalidArg,
                        format!(
                            "{INVALID_PARAMS_CODE}: transaction statement requires query: string"
                        ),
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

// ============================================================================
// LoraValue <-> JSON conversion
// ============================================================================

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
        LoraValue::Vector(v) => vector_to_json(v),
    }
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

fn json_value_to_params(value: serde_json::Value) -> Result<BTreeMap<String, LoraValue>> {
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

fn json_value_to_cypher(value: serde_json::Value) -> Result<LoraValue> {
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
) -> Result<&'a str> {
    match obj.get("iso").and_then(|v| v.as_str()) {
        Some(s) => Ok(s),
        None => Err(invalid_param(format!("{tag} value requires iso: string"))),
    }
}

fn invalid_param(msg: impl Into<String>) -> NapiError {
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

fn format_error(err: &anyhow::Error) -> String {
    format!("{LORA_ERROR_CODE}: {err}")
}

fn closed_error_message() -> String {
    format!("{LORA_ERROR_CODE}: database is closed")
}
