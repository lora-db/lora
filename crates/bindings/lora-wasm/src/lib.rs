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
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, QueryResult, ResultFormat,
};

mod json;

use json::{
    js_error, js_error_from_anyhow, json_value_to_params, parse_snapshot_credentials,
    parse_snapshot_options, parse_transaction_mode, parse_transaction_statements, row_to_json,
    serialize_rows,
};
/// Deprecated umbrella code preserved for binding-level static-message
/// call sites (stream closed, lock invariants). Engine errors go through
/// [`js_error_from_anyhow`] which sets the precise `LORA_*` code.
pub(crate) const LORA_ERROR_CODE: &str = "LORA_INTERNAL";
pub(crate) const INVALID_PARAMS_CODE: &str = "LORA_INVALID_PARAMS";

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
            .map_err(|e| js_error_from_anyhow(&e))?;

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
            .map_err(|e| js_error_from_anyhow(&e))?;
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
            .map_err(|e| js_error_from_anyhow(&e))?;

        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = tx
                .execute_with_params(&statement.query, Some(options), statement.params)
                .map_err(|e| js_error_from_anyhow(&e))?;
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(js_error(LORA_ERROR_CODE, "expected RowArrays result"));
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }

        tx.commit()
            .map_err(|e| js_error_from_anyhow(&e))?;

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
            .map_err(|e| js_error_from_anyhow(&e))
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
            .map_err(|e| js_error_from_anyhow(&e))?;

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
                Err(js_error_from_anyhow(&e))
            }
        }
    }

    pub fn close(&mut self) {
        self.stream.take();
    }
}
