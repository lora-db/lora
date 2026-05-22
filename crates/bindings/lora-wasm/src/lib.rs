#![deny(clippy::all)]

//! WebAssembly bindings for the Lora graph database.
//!
//! The Rust engine runs synchronously inside WASM; to keep JS hosts
//! responsive, the recommended execution path is via a Web Worker (browser)
//! or worker_thread (Node). The TS wrapper that ships alongside this crate
//! provides that architecture.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;

use serde::Serialize;
use serde_wasm_bindgen::Serializer;
use wasm_bindgen::prelude::*;

use lora_database::{
    snapshot_info as read_snapshot_info, Compression, CsvEncoder, Database as InnerDatabase,
    ExecuteOptions, ExportStats, Format as IoFormat, ImportStats, InMemoryGraph, JsonArrayEncoder,
    JsonlEncoder, LoraValue, QueryResult, QueryStream as InnerQueryStream, ResultFormat,
    RowEncoder, RowMapping, RowParseError, SnapshotInfo, StreamingCsvDecoder,
    StreamingJsonArrayDecoder, StreamingJsonlDecoder, StreamingRowDecoder,
    DEFAULT_IMPORT_BATCH_SIZE,
};

mod json;

use json::{
    js_error, js_error_from_anyhow, js_error_from_lora, json_value_to_params,
    parse_snapshot_credentials, parse_snapshot_options, parse_transaction_mode,
    parse_transaction_statements, plan_to_json, profile_to_json, row_to_json, serialize_rows,
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

/// Inspect snapshot header metadata from raw bytes without loading the
/// snapshot into a database. Decodes only the envelope/manifest — no
/// decryption or body decompression — so this works on encrypted snapshots
/// too. Returns a plain object:
/// `{ formatVersion, walLsn, nodeCount, relationshipCount, compression,
///    encrypted, keyId }`.
#[wasm_bindgen(js_name = snapshotInfo)]
pub fn snapshot_info(bytes: &[u8]) -> Result<JsValue, JsError> {
    let info = read_snapshot_info(bytes).map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
    snapshot_info_to_js(&info)
}

fn snapshot_info_to_js(info: &SnapshotInfo) -> Result<JsValue, JsError> {
    let compression = match info.compression {
        Compression::None => serde_json::json!({ "format": "none" }),
        Compression::Gzip { level } => serde_json::json!({
            "format": "gzip",
            "level": level,
        }),
    };
    let out = serde_json::json!({
        "formatVersion": info.format_version,
        "walLsn": info.wal_lsn,
        "nodeCount": info.node_count as u64,
        "relationshipCount": info.relationship_count as u64,
        "compression": compression,
        "encrypted": info.encrypted,
        "keyId": info.key_id,
    });
    out.serialize(&Serializer::json_compatible())
        .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
}

fn parse_optional_params(params: JsValue) -> Result<BTreeMap<String, LoraValue>, JsError> {
    if params.is_undefined() || params.is_null() {
        return Ok(BTreeMap::new());
    }
    let json_value: serde_json::Value = serde_wasm_bindgen::from_value(params)
        .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
    json_value_to_params(json_value)
}

fn parse_row_format(format: &str, action: &str) -> Result<IoFormat, JsError> {
    IoFormat::parse(format).ok_or_else(|| {
        js_error(
            INVALID_PARAMS_CODE,
            &format!("unknown {action} format `{format}`"),
        )
    })
}

fn export_payload_to_js(bytes: &[u8], stats: ExportStats) -> Result<JsValue, JsError> {
    let bytes = js_sys::Uint8Array::from(bytes);
    let result = js_sys::Object::new();
    js_sys::Reflect::set(&result, &"bytes".into(), &bytes.into())
        .map_err(|_| js_error(LORA_ERROR_CODE, "set bytes failed"))?;
    let stats_js = serde_json::json!({ "rows": stats.rows })
        .serialize(&Serializer::json_compatible())
        .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
    js_sys::Reflect::set(&result, &"stats".into(), &stats_js)
        .map_err(|_| js_error(LORA_ERROR_CODE, "set stats failed"))?;
    Ok(result.into())
}

fn import_stats_to_js(stats: ImportStats) -> Result<JsValue, JsError> {
    serde_json::json!({ "rows": stats.rows, "batches": stats.batches })
        .serialize(&Serializer::json_compatible())
        .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
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

    /// Execute a Lora query and return the result as a binary buffer.
    ///
    /// Wire format is the shared `lora_binding_buffer` layout. The TS
    /// wrapper decodes it once into the same `{ columns, rows }` shape
    /// `execute()` returns, but in a tight V8 loop instead of going
    /// through `serde_wasm_bindgen` value-by-value. Materially faster
    /// on bulk reads.
    #[wasm_bindgen(js_name = executeBuffer)]
    pub fn execute_buffer(&self, query: &str, params: JsValue) -> Result<Vec<u8>, JsError> {
        let params_map = if params.is_undefined() || params.is_null() {
            BTreeMap::new()
        } else {
            let json_value: serde_json::Value = serde_wasm_bindgen::from_value(params)
                .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
            json_value_to_params(json_value)?
        };

        // ResultFormat::Rows lets the encoder iterate the engine's
        // native row format directly and skip the RowArrays projection.
        let options = ExecuteOptions {
            format: ResultFormat::Rows,
        };

        let result = self
            .db
            .execute_with_params(query, Some(options), params_map)
            .map_err(|e| js_error_from_lora(&e))?;

        let QueryResult::Rows(rows_result) = result else {
            return Err(js_error(LORA_ERROR_CODE, "expected Rows result"));
        };

        Ok(lora_binding_buffer::encode_query_rows(&rows_result.rows))
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
            .map_err(|e| js_error_from_lora(&e))?;

        let QueryResult::RowArrays(row_arrays) = result else {
            return Err(js_error(LORA_ERROR_CODE, "expected RowArrays result"));
        };

        let out = serialize_rows(&row_arrays.columns, &row_arrays.rows);

        // `json_compatible` emits plain JS objects (not Maps) so the result
        // survives `structuredClone` across the worker boundary.
        out.serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    /// Compile a query and return its plan without executing it.
    ///
    /// Mutating queries (CREATE / MERGE / SET / DELETE / REMOVE) leave
    /// the graph untouched — this method never runs the executor.
    pub fn explain(&self, query: &str, params: JsValue) -> Result<JsValue, JsError> {
        let params_map = if params.is_undefined() || params.is_null() {
            None
        } else {
            let json_value: serde_json::Value = serde_wasm_bindgen::from_value(params)
                .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
            Some(json_value_to_params(json_value)?)
        };

        let plan = self
            .db
            .explain(query, params_map)
            .map_err(|e| js_error_from_lora(&e))?;
        let out = plan_to_json(&plan);
        out.serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    /// Execute a query and return the plan plus runtime metrics.
    ///
    /// **PROFILE executes the query for real.** Mutating queries are
    /// persisted exactly as in `execute()`. Use `explain()` to inspect
    /// a mutating plan without running it.
    pub fn profile(&self, query: &str, params: JsValue) -> Result<JsValue, JsError> {
        let params_map = if params.is_undefined() || params.is_null() {
            None
        } else {
            let json_value: serde_json::Value = serde_wasm_bindgen::from_value(params)
                .map_err(|e| js_error(INVALID_PARAMS_CODE, &e.to_string()))?;
            Some(json_value_to_params(json_value)?)
        };

        let prof = self
            .db
            .profile(query, params_map)
            .map_err(|e| js_error_from_lora(&e))?;
        let out = profile_to_json(&prof);
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
            .map_err(|e| js_error_from_lora(&e))?;
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
            .map_err(|e| js_error_from_lora(&e))?;

        let mut results = Vec::with_capacity(statements.len());
        for statement in statements {
            let result = tx
                .execute_with_params(&statement.query, Some(options), statement.params)
                .map_err(|e| js_error_from_lora(&e))?;
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(js_error(LORA_ERROR_CODE, "expected RowArrays result"));
            };
            results.push(serialize_rows(&row_arrays.columns, &row_arrays.rows));
        }

        tx.commit().map_err(|e| js_error_from_lora(&e))?;

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
            .map_err(|e| js_error_from_lora(&e))
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
            .map_err(|e| js_error_from_lora(&e))?;

        let out = serde_json::json!({
            "formatVersion": meta.format_version,
            "nodeCount": meta.node_count as u64,
            "relationshipCount": meta.relationship_count as u64,
            "walLsn": meta.wal_lsn,
        });
        out.serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    /// Run a query and serialize the result rows as the chosen format.
    /// `format` is one of `"jsonl"`, `"json"`, or `"csv"`. Returns the
    /// encoded bytes as a `Uint8Array` plus a row count.
    ///
    /// Returns `{ bytes: Uint8Array, stats: { rows: number } }`.
    #[wasm_bindgen(js_name = exportRows)]
    pub fn export_rows(
        &self,
        query: &str,
        params: JsValue,
        format: &str,
    ) -> Result<JsValue, JsError> {
        let params_map = parse_optional_params(params)?;
        let format = parse_row_format(format, "export")?;
        let mut out: Vec<u8> = Vec::new();
        // WASM always uses InMemoryGraph, so the true streaming
        // cursor is available. Pull rows row-at-a-time into `out`
        // instead of materialising a RowArrays projection first.
        let stats = self
            .db
            .export_query_streaming(query, params_map, format, &mut out)
            .map_err(|e| js_error_from_lora(&e))?;
        export_payload_to_js(out.as_slice(), stats)
    }

    /// Decode rows from `bytes` and apply them through the supplied
    /// [`RowMapping`]. `mapping` is JSON-serialised; see the `RowMapping`
    /// shape in `lora-io`. Returns `{ rows, batches }`.
    #[wasm_bindgen(js_name = importRows)]
    pub fn import_rows(
        &self,
        bytes: Vec<u8>,
        format: &str,
        mapping: JsValue,
        batch_size: Option<u32>,
    ) -> Result<JsValue, JsError> {
        let format = parse_row_format(format, "import")?;
        let mapping: RowMapping = serde_wasm_bindgen::from_value(mapping)
            .map_err(|e| js_error(INVALID_PARAMS_CODE, &format!("invalid mapping: {e}")))?;
        let stats = self
            .db
            .import_rows(
                std::io::Cursor::new(bytes),
                format,
                &mapping,
                batch_size.map(|n| n as usize),
            )
            .map_err(|e| js_error_from_lora(&e))?;
        import_stats_to_js(stats)
    }

    /// Decode rows from `bytes` and execute `template` once per batch
    /// with `$rows` bound to the batch payload. Escape hatch for the
    /// auto-mapping path: anything Cypher accepts can be used here.
    #[wasm_bindgen(js_name = importRowsWithCypher)]
    pub fn import_rows_with_cypher(
        &self,
        bytes: Vec<u8>,
        format: &str,
        template: &str,
        batch_size: Option<u32>,
    ) -> Result<JsValue, JsError> {
        let format = parse_row_format(format, "import")?;
        let stats = self
            .db
            .import_with_template(
                std::io::Cursor::new(bytes),
                format,
                template,
                batch_size.map(|n| n as usize),
            )
            .map_err(|e| js_error_from_lora(&e))?;
        import_stats_to_js(stats)
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
    stream: Option<InnerQueryStream<'static>>,
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

// ---------------------------------------------------------------------------
// Streaming row export
// ---------------------------------------------------------------------------

/// Streaming row export cursor.
///
/// Holds a native [`InnerQueryStream`] plus a format-specific
/// [`RowEncoder`] writing into a shared `Vec<u8>` buffer. Each
/// [`Self::next`] call pulls a fixed number of rows from the cursor,
/// encodes them, and returns whatever bytes the encoder wrote since
/// the previous call. Once the cursor is exhausted, [`Self::next`]
/// emits the encoder's trailer chunk and then returns `null`.
///
/// The buffer is reused across chunks (`mem::take` swaps it out
/// rather than reallocating), keeping per-chunk allocation cost
/// independent of the total export size.
#[wasm_bindgen(js_name = WasmRowExport)]
pub struct WasmRowExport {
    /// Kept alive so the QueryStream's snapshot references stay valid.
    _db: Arc<InnerDatabase<InMemoryGraph>>,
    stream: Option<InnerQueryStream<'static>>,
    encoder: Option<Box<dyn RowEncoder>>,
    buffer: Rc<RefCell<Vec<u8>>>,
    columns: Vec<String>,
    began: bool,
    finished: bool,
}

/// Rows pulled per `next()` call. Sized so the per-chunk
/// JSON-encoded payload stays roughly in the 32–256 KiB range
/// for typical row widths.
const EXPORT_ROWS_PER_CHUNK: usize = 256;

/// Pass-through writer that appends to a shared `Vec<u8>`.
/// Used so [`WasmRowExport`] can drain the encoder's output
/// without owning the encoder's writer through a static type
/// parameter.
struct SharedBufferWriter(Rc<RefCell<Vec<u8>>>);

impl Write for SharedBufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[wasm_bindgen(js_class = WasmRowExport)]
impl WasmRowExport {
    pub fn columns(&self) -> Result<JsValue, JsError> {
        self.columns
            .clone()
            .serialize(&Serializer::json_compatible())
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    /// Pull and encode the next chunk of rows. Returns the encoded
    /// bytes (possibly empty if the encoder buffered without
    /// flushing — JSONL/CSV/JSON encoders always flush per row, so
    /// this is rare), or `null` when the export is fully drained.
    /// After `null` is returned, the cursor is closed automatically.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<JsValue, JsError> {
        if self.finished && self.buffer.borrow().is_empty() {
            self.close_internal();
            return Ok(JsValue::NULL);
        }

        // First call: emit the encoder header (CSV header row,
        // opening `[` for JSON array). JSONL writes nothing here.
        if !self.began {
            let encoder = self
                .encoder
                .as_mut()
                .ok_or_else(|| js_error(LORA_ERROR_CODE, "export cursor is closed"))?;
            encoder
                .begin(&self.columns)
                .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
            self.began = true;
        }

        if !self.finished {
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| js_error(LORA_ERROR_CODE, "export cursor is closed"))?;
            let encoder = self
                .encoder
                .as_mut()
                .ok_or_else(|| js_error(LORA_ERROR_CODE, "export cursor is closed"))?;

            for _ in 0..EXPORT_ROWS_PER_CHUNK {
                match stream.next_row() {
                    Ok(Some(row)) => {
                        encoder
                            .write_row(&row)
                            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
                    }
                    Ok(None) => {
                        encoder
                            .finish()
                            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
                        self.finished = true;
                        break;
                    }
                    Err(e) => {
                        self.close_internal();
                        return Err(js_error_from_anyhow(&e));
                    }
                }
            }
        }

        let chunk = std::mem::take(&mut *self.buffer.borrow_mut());
        if chunk.is_empty() && self.finished {
            self.close_internal();
            return Ok(JsValue::NULL);
        }
        // Returning the chunk via serde_wasm_bindgen would round-trip
        // through structured-clone; ship the Uint8Array directly so
        // its backing buffer is transferable in the worker hop.
        Ok(js_sys::Uint8Array::from(chunk.as_slice()).into())
    }

    pub fn close(&mut self) {
        self.close_internal();
    }

    fn close_internal(&mut self) {
        self.stream.take();
        self.encoder.take();
    }
}

impl WasmDatabase {
    fn open_export_internal(
        &self,
        query: &str,
        params: JsValue,
        format: IoFormat,
    ) -> Result<WasmRowExport, JsError> {
        let params_map = parse_optional_params(params)?;
        // SAFETY: see WasmDatabase::open_stream — the cursor's lifetime
        // is faked to 'static because the WasmRowExport keeps the
        // Arc<InnerDatabase> alive for the cursor's whole lifetime.
        let stream = unsafe { self.db.stream_with_params_owned(query, params_map) }
            .map_err(|e| js_error_from_lora(&e))?;
        let columns = stream.columns().to_vec();

        let buffer = Rc::new(RefCell::new(Vec::with_capacity(64 * 1024)));
        let writer = SharedBufferWriter(buffer.clone());
        let encoder: Box<dyn RowEncoder> = match format {
            IoFormat::Jsonl => Box::new(JsonlEncoder::new(writer)),
            IoFormat::Json => Box::new(JsonArrayEncoder::new(writer)),
            IoFormat::Csv => Box::new(CsvEncoder::new(writer)),
        };
        Ok(WasmRowExport {
            _db: self.db.clone(),
            stream: Some(stream),
            encoder: Some(encoder),
            buffer,
            columns,
            began: false,
            finished: false,
        })
    }
}

#[wasm_bindgen(js_class = WasmDatabase)]
impl WasmDatabase {
    /// Open a streaming row-export cursor. Each `next()` call pulls a
    /// chunk of rows, encodes them, and returns the encoded bytes as
    /// a `Uint8Array`. Returns `null` once the cursor is fully drained
    /// (and closes the cursor as a side effect).
    #[wasm_bindgen(js_name = openExport)]
    pub fn open_export(
        &self,
        query: &str,
        params: JsValue,
        format: &str,
    ) -> Result<WasmRowExport, JsError> {
        let fmt = parse_row_format(format, "export")?;
        self.open_export_internal(query, params, fmt)
    }

    /// Open a streaming row-import cursor. The caller feeds chunks of
    /// bytes via [`WasmRowImport::feed`]; the cursor accumulates them
    /// into completed records, batches them, and runs each batch as
    /// one `UNWIND $rows AS r ...` Cypher statement. JSON-array format
    /// is rejected — use JSONL for streaming JSON imports.
    ///
    /// `mappingOrTemplate` is either a JSON object matching `RowMapping`
    /// (auto-mapping path) or a string containing a Cypher template
    /// that binds `$rows` (escape hatch). The distinction is made by
    /// JS type — strings go straight to the template path.
    #[wasm_bindgen(js_name = openImport)]
    pub fn open_import(
        &self,
        format: &str,
        mapping_or_template: JsValue,
        batch_size: Option<u32>,
        dry_run: Option<bool>,
        permissive: Option<bool>,
    ) -> Result<WasmRowImport, JsError> {
        let fmt = parse_row_format(format, "import")?;
        let template = resolve_import_template(mapping_or_template)?;
        let mut decoder: Box<dyn StreamingRowDecoder> = match fmt {
            IoFormat::Jsonl => Box::new(StreamingJsonlDecoder::new()),
            IoFormat::Csv => Box::new(StreamingCsvDecoder::new()),
            IoFormat::Json => Box::new(StreamingJsonArrayDecoder::new()),
        };
        let permissive = permissive.unwrap_or(false);
        if permissive {
            decoder.set_permissive(true);
        }
        let batch_size = batch_size
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_IMPORT_BATCH_SIZE)
            .max(1);
        Ok(WasmRowImport {
            db: self.db.clone(),
            decoder: Some(decoder),
            template,
            batch: Vec::with_capacity(batch_size),
            batch_size,
            dry_run: dry_run.unwrap_or(false),
            rows: 0,
            batches: 0,
            skipped: 0,
            errors: Vec::new(),
        })
    }
}

/// Either render a [`RowMapping`] to its Cypher template, or accept a
/// caller-supplied template string verbatim. The JS side distinguishes
/// by passing either an object or a string; we sniff `JsValue::as_string`
/// before falling back to serde decoding.
fn resolve_import_template(value: JsValue) -> Result<String, JsError> {
    if let Some(template) = value.as_string() {
        return Ok(template);
    }
    let mapping: RowMapping = serde_wasm_bindgen::from_value(value).map_err(|e| {
        js_error(
            INVALID_PARAMS_CODE,
            &format!("invalid mapping (expected a string template or RowMapping object): {e}"),
        )
    })?;
    mapping
        .to_cypher()
        .map_err(|e| js_error(INVALID_PARAMS_CODE, &format!("invalid row mapping: {e}")))
}

/// Streaming row-import cursor.
///
/// Holds a [`StreamingRowDecoder`] (JSONL or CSV) plus a Cypher
/// template. Each [`Self::feed`] call appends bytes to the decoder
/// and flushes any completed records into a batch buffer; full
/// batches execute as one parameterised statement and clear the
/// buffer. [`Self::finish`] drains the decoder's residual and
/// flushes the final partial batch. The encoder buffer + one batch
/// of `LoraValue::Map` records is the only steady-state memory
/// footprint — peak resident set is independent of total import size.
#[wasm_bindgen(js_name = WasmRowImport)]
pub struct WasmRowImport {
    db: Arc<InnerDatabase<InMemoryGraph>>,
    decoder: Option<Box<dyn StreamingRowDecoder>>,
    template: String,
    batch: Vec<lora_database::LoraValue>,
    batch_size: usize,
    /// When set, batches are validated and counted but never executed
    /// against the engine. Used by the playground's Preview path to
    /// verify a mapping + parseability before mutating the graph.
    dry_run: bool,
    rows: u64,
    batches: u64,
    /// Total number of records the decoder skipped in permissive mode
    /// since this cursor was opened.
    skipped: u64,
    /// Per-record parse errors collected from the decoder in permissive
    /// mode. Capped at [`MAX_REPORTED_IMPORT_ERRORS`]; the `skipped`
    /// counter keeps the total visible even after the cap is reached.
    errors: Vec<RowParseError>,
}

/// Cap on the number of structured parse errors WasmRowImport will
/// carry. Bound here is so a pathological 1M-bad-row file doesn't
/// inflate the WASM-to-JS payload — `skipped` still reports the true
/// total beyond this cap.
const MAX_REPORTED_IMPORT_ERRORS: usize = 100;

#[wasm_bindgen(js_class = WasmRowImport)]
impl WasmRowImport {
    /// Append a chunk of bytes and process any records that became
    /// complete. Returns progress: total bytes ingested, completed
    /// rows so far, and batches committed so far.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<JsValue, JsError> {
        let decoder = self
            .decoder
            .as_mut()
            .ok_or_else(|| js_error(LORA_ERROR_CODE, "import cursor is closed"))?;
        decoder
            .feed(chunk)
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
        let records = decoder
            .drain()
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
        self.absorb_decoder_errors();
        for record in records {
            self.push_record(record)?;
        }
        self.progress_to_js()
    }

    /// Signal that no more bytes will be fed, drain any residual
    /// records, and flush the final partial batch. Returns the
    /// final stats and closes the cursor.
    pub fn finish(&mut self) -> Result<JsValue, JsError> {
        let mut decoder = self
            .decoder
            .take()
            .ok_or_else(|| js_error(LORA_ERROR_CODE, "import cursor is closed"))?;
        let records = decoder
            .finish()
            .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))?;
        // Drain any errors the decoder accumulated during finish(),
        // then push records and flush the final partial batch.
        let final_errors = decoder.take_errors();
        let final_error_count = final_errors.len() as u64;
        self.skipped += final_error_count;
        let remaining_capacity = MAX_REPORTED_IMPORT_ERRORS.saturating_sub(self.errors.len());
        self.errors
            .extend(final_errors.into_iter().take(remaining_capacity));
        for record in records {
            self.push_record(record)?;
        }
        if !self.batch.is_empty() {
            self.flush_batch()?;
        }
        self.stats_to_js()
    }

    pub fn close(&mut self) {
        self.decoder.take();
        self.batch.clear();
    }
}

impl WasmRowImport {
    /// Pull whatever the decoder has accumulated since the last call
    /// and fold it into the cursor's running totals. Honors the
    /// per-cursor cap so the JS payload stays bounded even when a
    /// large file is full of parse errors.
    fn absorb_decoder_errors(&mut self) {
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        let fresh = decoder.take_errors();
        if fresh.is_empty() {
            return;
        }
        self.skipped += fresh.len() as u64;
        let remaining = MAX_REPORTED_IMPORT_ERRORS.saturating_sub(self.errors.len());
        if remaining > 0 {
            self.errors.extend(fresh.into_iter().take(remaining));
        }
    }

    fn push_record(
        &mut self,
        record: Vec<(String, lora_database::LoraValue)>,
    ) -> Result<(), JsError> {
        let row_map: BTreeMap<String, lora_database::LoraValue> = record.into_iter().collect();
        self.batch.push(lora_database::LoraValue::Map(row_map));
        if self.batch.len() >= self.batch_size {
            self.flush_batch()?;
        }
        Ok(())
    }

    fn flush_batch(&mut self) -> Result<(), JsError> {
        let batch = std::mem::take(&mut self.batch);
        let batch_len = batch.len() as u64;
        if !self.dry_run {
            let mut params = BTreeMap::new();
            params.insert("rows".to_string(), lora_database::LoraValue::List(batch));
            self.db
                .execute_with_params(&self.template, None, params)
                .map_err(|e| js_error_from_lora(&e))?;
        }
        self.rows += batch_len;
        self.batches += 1;
        Ok(())
    }

    fn progress_to_js(&self) -> Result<JsValue, JsError> {
        let bytes_fed = self.decoder.as_ref().map(|d| d.bytes_fed()).unwrap_or(0);
        let rows_seen = self.decoder.as_ref().map(|d| d.rows_emitted()).unwrap_or(0);
        serde_json::json!({
            "bytesFed": bytes_fed,
            "rowsSeen": rows_seen,
            "rowsCommitted": self.rows,
            "batches": self.batches,
            "skipped": self.skipped,
        })
        .serialize(&Serializer::json_compatible())
        .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }

    fn stats_to_js(&self) -> Result<JsValue, JsError> {
        let errors: Vec<serde_json::Value> = self
            .errors
            .iter()
            .map(|e| {
                serde_json::json!({
                    "row": e.row,
                    "column": e.column,
                    "rawSample": e.raw_sample,
                    "message": e.message,
                })
            })
            .collect();
        serde_json::json!({
            "rows": self.rows,
            "batches": self.batches,
            "skipped": self.skipped,
            "errors": errors,
        })
        .serialize(&Serializer::json_compatible())
        .map_err(|e| js_error(LORA_ERROR_CODE, &e.to_string()))
    }
}
