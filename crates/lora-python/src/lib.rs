#![deny(clippy::all)]
// The pyo3 `#[pymethods]` / `#[pyo3(signature = ...)]` macros expand to code
// that includes `PyErr::from(e)` on `?` error paths; because our error type is
// already `PyErr`, clippy flags it as a useless conversion. The expansion is
// outside our control, so the allow lives at the crate level.
#![allow(clippy::useless_conversion)]

//! PyO3 bindings for the Lora graph database.
//!
//! The Rust engine is synchronous, so we expose it as a sync `Database`
//! class and release the Python GIL for the duration of each query. A
//! pure-Python `AsyncDatabase` wrapper (in `python/lora_python/_async.py`)
//! uses `asyncio.to_thread` on top of these sync methods so async callers
//! never block the event loop.
//!
//! Value conversion follows the shared `LoraValue` contract used by
//! `lora-node` and `lora-wasm`: primitives pass through as Python
//! natives; graph, temporal and spatial values are returned as tagged
//! `dict`s with a `kind` discriminator.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyAny, PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString};

use lora_database::{
    Database as InnerDatabase, DatabaseOpenOptions, ExecuteOptions, InMemoryGraph, LoraValue,
    QueryResult, ResultFormat, Row, SnapshotConfig, SnapshotOptions, TransactionMode, WalConfig,
};
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
};

// ============================================================================
// Module entry point
// ============================================================================

create_exception!(
    lora_python,
    LoraError,
    PyException,
    "Base class for Lora engine errors."
);
create_exception!(
    lora_python,
    LoraQueryError,
    LoraError,
    "Parse / analyze / execute failure."
);
create_exception!(
    lora_python,
    InvalidParamsError,
    LoraError,
    "A parameter value could not be mapped to a Lora value."
);

/// Native extension module. The pure-Python layer in `lora_python`
/// re-exports `Database` plus the typed `AsyncDatabase` wrapper.
#[pymodule]
fn _native(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Database>()?;
    m.add_class::<PyQueryStream>()?;
    m.add("LoraError", py.get_type_bound::<LoraError>())?;
    m.add("LoraQueryError", py.get_type_bound::<LoraQueryError>())?;
    m.add(
        "InvalidParamsError",
        py.get_type_bound::<InvalidParamsError>(),
    )?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

// ============================================================================
// Database
// ============================================================================

/// Synchronous Lora graph database handle.
///
/// Query execution runs with the GIL released so other Python threads —
/// notably `asyncio.to_thread` workers — can progress in parallel. Concurrent
/// read-only calls against the same `Database` can share the store read lock,
/// while writes serialize without holding the GIL.
#[pyclass(module = "lora_python._native")]
pub struct Database {
    db: Mutex<Option<Arc<InnerDatabase<InMemoryGraph>>>>,
}

#[pymethods]
impl Database {
    #[new]
    #[pyo3(signature = (database_name=None, options=None))]
    fn py_new(
        py: Python<'_>,
        database_name: Option<String>,
        options: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let options = py_database_open_options(options)?;
        let db = py
            .allow_threads(move || open_database(database_name, options))
            .map_err(LoraQueryError::new_err)?;
        Ok(Self {
            db: Mutex::new(Some(db)),
        })
    }

    /// Factory mirroring the async API shape. Returns `self` for symmetry
    /// with `AsyncDatabase.create()`.
    #[staticmethod]
    #[pyo3(signature = (database_name=None, options=None))]
    fn create(
        py: Python<'_>,
        database_name: Option<String>,
        options: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        Self::py_new(py, database_name, options)
    }

    /// Open or create an explicit WAL-backed database.
    #[staticmethod]
    #[pyo3(signature = (wal_dir, options=None))]
    fn open_wal(
        py: Python<'_>,
        wal_dir: String,
        options: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let mut options = py_database_open_options(options)?;
        if options.wal_dir.is_some() {
            return Err(InvalidParamsError::new_err(
                "wal_dir must be passed as the first argument to open_wal",
            ));
        }
        options.wal_dir = Some(wal_dir);
        let db = py
            .allow_threads(move || open_wal_database(options))
            .map_err(LoraQueryError::new_err)?;
        Ok(Self {
            db: Mutex::new(Some(db)),
        })
    }

    /// Execute a Lora query.
    ///
    /// Returns `{"columns": [...], "rows": [...]}` where each row is a
    /// `dict[str, Any]`. Releases the GIL while the engine runs.
    #[pyo3(signature = (query, params=None))]
    fn execute<'py>(
        &self,
        py: Python<'py>,
        query: String,
        params: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        // Parse params while we still hold the GIL.
        let params_map = match params {
            Some(p) if !p.is_none() => py_object_to_params(p)?,
            _ => BTreeMap::new(),
        };

        let db = self.inner()?;
        // Release the GIL for the duration of engine work.
        let exec_result = py.allow_threads(move || {
            let options = ExecuteOptions {
                format: ResultFormat::RowArrays,
            };
            db.execute_with_params(&query, Some(options), params_map)
        });

        let row_arrays = match exec_result {
            Ok(QueryResult::RowArrays(r)) => r,
            Ok(_) => {
                return Err(LoraQueryError::new_err(
                    "expected RowArrays result".to_string(),
                ));
            }
            Err(e) => return Err(LoraQueryError::new_err(format!("{e}"))),
        };

        let out = PyDict::new_bound(py);
        let columns = PyList::new_bound(py, row_arrays.columns.iter().map(|c| c.as_str()));
        out.set_item("columns", columns)?;

        let rows = PyList::empty_bound(py);
        for row in &row_arrays.rows {
            let py_row = PyDict::new_bound(py);
            for (col, val) in row_arrays.columns.iter().zip(row.iter()) {
                py_row.set_item(col, lora_value_to_py(py, val)?)?;
            }
            rows.append(py_row)?;
        }
        out.set_item("rows", rows)?;
        Ok(out)
    }

    /// Return an iterator over result rows. The query is materialized by
    /// `execute()` first, then Python consumes the row list lazily.
    #[pyo3(signature = (query, params=None))]
    fn stream<'py>(
        &self,
        _py: Python<'py>,
        query: String,
        params: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<PyQueryStream> {
        let params_map = match params {
            Some(p) if !p.is_none() => py_object_to_params(p)?,
            _ => BTreeMap::new(),
        };
        let db = self.inner()?;
        let stream = unsafe { db.stream_with_params_owned(&query, params_map) }
            .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
        Ok(PyQueryStream {
            _db: db,
            stream: Some(stream),
        })
    }

    /// Execute statement objects inside one native transaction.
    ///
    /// `statements` is an iterable of mappings with `query` and optional
    /// `params` keys. Results are returned in statement order; if any
    /// statement fails the transaction is dropped before commit.
    #[pyo3(signature = (statements, mode="read_write"))]
    fn transaction<'py>(
        &self,
        py: Python<'py>,
        statements: &Bound<'py, PyAny>,
        mode: &str,
    ) -> PyResult<Bound<'py, PyList>> {
        let parsed_mode = parse_transaction_mode(mode)?;
        let parsed_statements = py_statements_to_transaction(statements)?;
        let db = self.inner()?;

        let exec_results = py.allow_threads(move || {
            let mut tx = db.begin_transaction(parsed_mode)?;
            let mut results = Vec::with_capacity(parsed_statements.len());
            for statement in parsed_statements {
                let options = ExecuteOptions {
                    format: ResultFormat::RowArrays,
                };
                let result =
                    tx.execute_with_params(&statement.query, Some(options), statement.params)?;
                results.push(result);
            }
            tx.commit()?;
            Ok::<_, anyhow::Error>(results)
        });

        let exec_results = exec_results.map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
        let out = PyList::empty_bound(py);
        for result in exec_results {
            let QueryResult::RowArrays(row_arrays) = result else {
                return Err(LoraQueryError::new_err(
                    "expected RowArrays result".to_string(),
                ));
            };
            out.append(row_arrays_to_py(py, &row_arrays.columns, &row_arrays.rows)?)?;
        }
        Ok(out)
    }

    /// Drop every node and relationship. Constant-time.
    fn clear(&self) -> PyResult<()> {
        self.inner()?.clear();
        Ok(())
    }

    /// Release the native database handle. Idempotent.
    fn close(&self) -> PyResult<()> {
        let mut slot = self
            .db
            .lock()
            .map_err(|_| LoraQueryError::new_err("database lock poisoned"))?;
        slot.take();
        Ok(())
    }

    /// Number of nodes currently in the graph.
    #[getter]
    fn node_count(&self) -> PyResult<u64> {
        Ok(self.inner()?.node_count() as u64)
    }

    /// Number of relationships currently in the graph.
    #[getter]
    fn relationship_count(&self) -> PyResult<u64> {
        Ok(self.inner()?.relationship_count() as u64)
    }

    /// Save the graph to a snapshot file, byte string, base64 string, or
    /// file-like writer.
    #[pyo3(signature = (target=None, format=None, options=None))]
    fn save_snapshot<'py>(
        &self,
        py: Python<'py>,
        target: Option<&Bound<'py, PyAny>>,
        format: Option<&str>,
        options: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let db = self.inner()?;
        let snapshot_options = py_snapshot_options(options)?;
        let requested = format.map(str::to_string).or_else(|| {
            target
                .and_then(|value| value.extract::<String>().ok())
                .filter(|value| matches!(value.as_str(), "binary" | "bytes" | "base64"))
                .map(|value| match value.as_str() {
                    "bytes" => "binary".to_string(),
                    other => other.to_string(),
                })
        });

        if matches!(requested.as_deref(), Some("binary" | "base64")) || target.is_none() {
            let (bytes, _meta) = save_snapshot_to_vec(py, db, snapshot_options)?;
            return match requested.as_deref().unwrap_or("binary") {
                "base64" => py_base64_encode(py, &bytes),
                _ => Ok(PyBytes::new_bound(py, &bytes).into_any().unbind()),
            };
        }

        let Some(target) = target else {
            return Err(PyTypeError::new_err("snapshot target is required"));
        };

        if has_attr(target, "write")? {
            let (bytes, meta) = save_snapshot_to_vec(py, db, snapshot_options)?;
            target.call_method1("write", (PyBytes::new_bound(py, &bytes),))?;
            return Ok(snapshot_meta_to_py(py, meta)?.into_any().unbind());
        }

        let path = py_fspath(target)?;
        let meta = py
            .allow_threads(move || db.save_snapshot_to_with_options(&path, &snapshot_options))
            .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
        Ok(snapshot_meta_to_py(py, meta)?.into_any().unbind())
    }

    /// Replace the current graph state from a path, bytes-like object,
    /// base64 string, or file-like reader.
    #[pyo3(signature = (source, format=None, options=None))]
    fn load_snapshot<'py>(
        &self,
        py: Python<'py>,
        source: &Bound<'py, PyAny>,
        format: Option<&str>,
        options: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let db = self.inner()?;
        let credentials = py_snapshot_credentials(options)?;

        if matches!(format, Some("base64")) {
            let bytes = py_base64_decode(py, source)?;
            let meta = py
                .allow_threads(move || {
                    db.load_snapshot_from_bytes_with_credentials(
                        bytes.as_slice(),
                        credentials.as_ref(),
                    )
                })
                .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
            return snapshot_meta_to_py(py, meta);
        }

        if let Ok(bytes) = source.extract::<PyBackedBytes>() {
            let bytes = bytes.as_ref().to_vec();
            let meta = py
                .allow_threads(move || {
                    db.load_snapshot_from_bytes_with_credentials(
                        bytes.as_slice(),
                        credentials.as_ref(),
                    )
                })
                .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
            return snapshot_meta_to_py(py, meta);
        }

        if has_attr(source, "read")? {
            let bytes_obj = source.call_method0("read")?;
            let bytes = bytes_obj.extract::<PyBackedBytes>().map_err(|_| {
                PyTypeError::new_err("snapshot reader.read() must return bytes or bytearray")
            })?;
            let bytes = bytes.as_ref().to_vec();
            let meta = py
                .allow_threads(move || {
                    db.load_snapshot_from_bytes_with_credentials(
                        bytes.as_slice(),
                        credentials.as_ref(),
                    )
                })
                .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
            return snapshot_meta_to_py(py, meta);
        }

        if has_attr(source, "tobytes")? {
            let bytes_obj = source.call_method0("tobytes")?;
            let bytes = bytes_obj.extract::<PyBackedBytes>().map_err(|_| {
                PyTypeError::new_err("snapshot source.tobytes() must return bytes or bytearray")
            })?;
            let bytes = bytes.as_ref().to_vec();
            let meta = py
                .allow_threads(move || {
                    db.load_snapshot_from_bytes_with_credentials(
                        bytes.as_slice(),
                        credentials.as_ref(),
                    )
                })
                .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
            return snapshot_meta_to_py(py, meta);
        }

        let path = py_fspath(source)?;
        let meta = py
            .allow_threads(move || {
                db.load_snapshot_from_with_credentials(&path, credentials.as_ref())
            })
            .map_err(|e| LoraQueryError::new_err(format!("{e}")))?;
        snapshot_meta_to_py(py, meta)
    }

    fn __repr__(&self) -> String {
        match self.db.lock().ok().and_then(|slot| slot.as_ref().cloned()) {
            Some(db) => format!(
                "<lora_python.Database nodes={} relationships={}>",
                db.node_count(),
                db.relationship_count(),
            ),
            None => "<lora_python.Database closed>".to_string(),
        }
    }
}

fn save_snapshot_to_vec(
    py: Python<'_>,
    db: Arc<InnerDatabase<InMemoryGraph>>,
    options: lora_database::SnapshotOptions,
) -> PyResult<(Vec<u8>, lora_database::SnapshotMeta)> {
    let result = py.allow_threads(move || {
        db.save_snapshot_to_bytes_with_options(&options)
            .map(|(bytes, info)| (bytes, snapshot_info_to_meta(info)))
    });
    result.map_err(|e| LoraQueryError::new_err(format!("{e}")))
}

fn snapshot_info_to_meta(info: lora_database::SnapshotInfo) -> lora_database::SnapshotMeta {
    lora_database::SnapshotMeta {
        format_version: info.format_version,
        node_count: info.node_count,
        relationship_count: info.relationship_count,
        wal_lsn: info.wal_lsn,
    }
}

fn snapshot_meta_to_py<'py>(
    py: Python<'py>,
    meta: lora_database::SnapshotMeta,
) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new_bound(py);
    out.set_item("formatVersion", meta.format_version)?;
    out.set_item("nodeCount", meta.node_count as u64)?;
    out.set_item("relationshipCount", meta.relationship_count as u64)?;
    match meta.wal_lsn {
        Some(lsn) => out.set_item("walLsn", lsn)?,
        None => out.set_item("walLsn", py.None())?,
    }
    Ok(out)
}

fn has_attr(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<bool> {
    obj.hasattr(name)
}

fn py_fspath(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let os = obj.py().import_bound("os")?;
    let path = os.getattr("fspath")?.call1((obj,))?;
    path.extract::<String>().map_err(|_| {
        PyTypeError::new_err("snapshot path must be str, os.PathLike[str], bytes, or a stream")
    })
}

fn py_base64_encode(py: Python<'_>, bytes: &[u8]) -> PyResult<Py<PyAny>> {
    let base64 = py.import_bound("base64")?;
    let encoded = base64
        .getattr("b64encode")?
        .call1((PyBytes::new_bound(py, bytes),))?;
    let text = encoded.call_method1("decode", ("ascii",))?;
    Ok(text.unbind())
}

fn py_base64_decode(py: Python<'_>, source: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let base64 = py.import_bound("base64")?;
    let decoded = base64.getattr("b64decode")?.call1((source,))?;
    decoded
        .extract::<PyBackedBytes>()
        .map(|bytes| bytes.as_ref().to_vec())
        .map_err(|e| PyTypeError::new_err(format!("invalid base64 snapshot: {e}")))
}

#[pyclass(name = "QueryStream", module = "lora_python._native", unsendable)]
pub struct PyQueryStream {
    _db: Arc<InnerDatabase<InMemoryGraph>>,
    stream: Option<lora_database::QueryStream<'static>>,
}

#[pymethods]
impl PyQueryStream {
    fn columns(&self) -> PyResult<Vec<String>> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| LoraQueryError::new_err("query stream is closed"))?;
        Ok(stream.columns().to_vec())
    }

    fn close(&mut self) {
        self.stream.take();
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        let stream = match self.stream.as_mut() {
            Some(stream) => stream,
            None => return Ok(None),
        };
        match stream.next_row() {
            Ok(Some(row)) => Ok(Some(row_to_py_dict(py, &row)?)),
            Ok(None) => {
                self.stream.take();
                Ok(None)
            }
            Err(e) => {
                self.stream.take();
                Err(LoraQueryError::new_err(format!("{e}")))
            }
        }
    }
}

impl Database {
    fn inner(&self) -> PyResult<Arc<InnerDatabase<InMemoryGraph>>> {
        let slot = self
            .db
            .lock()
            .map_err(|_| LoraQueryError::new_err("database lock poisoned"))?;
        slot.as_ref()
            .cloned()
            .ok_or_else(|| LoraQueryError::new_err("database is closed"))
    }
}

struct PyDatabaseOpenOptions {
    named: DatabaseOpenOptions,
    has_database_dir: bool,
    wal_dir: Option<String>,
    snapshot_dir: Option<String>,
    snapshot_every_commits: Option<u64>,
    snapshot_keep_old: Option<usize>,
    has_snapshot_codec: bool,
    snapshot_codec: SnapshotOptions,
}

impl PyDatabaseOpenOptions {
    fn has_explicit_wal_options(&self) -> bool {
        self.wal_dir.is_some()
            || self.snapshot_dir.is_some()
            || self.snapshot_every_commits.is_some()
            || self.snapshot_keep_old.is_some()
            || self.has_snapshot_codec
    }

    fn has_snapshot_tuning_options(&self) -> bool {
        self.snapshot_every_commits.is_some()
            || self.snapshot_keep_old.is_some()
            || self.has_snapshot_codec
    }
}

fn py_database_open_options(
    options: Option<&Bound<'_, PyDict>>,
) -> PyResult<PyDatabaseOpenOptions> {
    let mut out = PyDatabaseOpenOptions {
        named: DatabaseOpenOptions::default(),
        has_database_dir: false,
        wal_dir: None,
        snapshot_dir: None,
        snapshot_every_commits: None,
        snapshot_keep_old: None,
        has_snapshot_codec: false,
        snapshot_codec: SnapshotOptions::default(),
    };
    let Some(options) = options else {
        return Ok(out);
    };
    let value = match options.get_item("database_dir")? {
        Some(value) => Some(value),
        None => options.get_item("databaseDir")?,
    };
    if let Some(value) = value {
        out.named.database_dir = value.extract::<String>()?.into();
        out.has_database_dir = true;
    }
    let value = match options.get_item("wal_dir")? {
        Some(value) => Some(value),
        None => options.get_item("walDir")?,
    };
    if let Some(value) = value {
        out.wal_dir = Some(value.extract::<String>()?);
    }
    let value = match options.get_item("snapshot_dir")? {
        Some(value) => Some(value),
        None => options.get_item("snapshotDir")?,
    };
    if let Some(value) = value {
        out.snapshot_dir = Some(value.extract::<String>()?);
    }
    let value = match options.get_item("snapshot_every_commits")? {
        Some(value) => Some(value),
        None => options.get_item("snapshotEveryCommits")?,
    };
    if let Some(value) = value {
        out.snapshot_every_commits = Some(value.extract::<u64>()?);
    }
    let value = match options.get_item("snapshot_keep_old")? {
        Some(value) => Some(value),
        None => options.get_item("snapshotKeepOld")?,
    };
    if let Some(value) = value {
        out.snapshot_keep_old = Some(value.extract::<usize>()?);
    }
    let value = match options.get_item("snapshot_options")? {
        Some(value) => Some(value),
        None => options.get_item("snapshotOptions")?,
    };
    if let Some(value) = value {
        out.has_snapshot_codec = true;
        out.snapshot_codec = lora_database::snapshot_options_from_json(Some(py_to_json(&value)?))
            .map_err(|e| {
            InvalidParamsError::new_err(format!("invalid snapshot options: {e}"))
        })?;
    }
    Ok(out)
}

fn py_snapshot_options(
    options: Option<&Bound<'_, PyAny>>,
) -> PyResult<lora_database::SnapshotOptions> {
    let json = py_optional_to_json(options)?;
    lora_database::snapshot_options_from_json(json)
        .map_err(|e| InvalidParamsError::new_err(format!("invalid snapshot options: {e}")))
}

fn py_snapshot_credentials(
    options: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<lora_database::SnapshotCredentials>> {
    let json = py_optional_to_json(options)?;
    lora_database::snapshot_credentials_from_json(json)
        .map_err(|e| InvalidParamsError::new_err(format!("invalid snapshot credentials: {e}")))
}

fn py_optional_to_json(options: Option<&Bound<'_, PyAny>>) -> PyResult<Option<serde_json::Value>> {
    match options {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => py_to_json(value).map(Some),
    }
}

fn py_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    if obj.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = obj.downcast::<PyBool>() {
        return Ok(serde_json::Value::Bool(b.is_true()));
    }
    if let Ok(i) = obj.downcast::<PyInt>() {
        let value = i.extract::<i64>().map_err(|_| {
            InvalidParamsError::new_err("snapshot option integer does not fit in i64")
        })?;
        return Ok(serde_json::Value::Number(value.into()));
    }
    if let Ok(f) = obj.downcast::<PyFloat>() {
        let value = f.extract::<f64>()?;
        let Some(number) = serde_json::Number::from_f64(value) else {
            return Err(InvalidParamsError::new_err(
                "snapshot option float must be finite",
            ));
        };
        return Ok(serde_json::Value::Number(number));
    }
    if let Ok(s) = obj.downcast::<PyString>() {
        return Ok(serde_json::Value::String(s.extract::<String>()?));
    }
    if let Ok(bytes) = obj.extract::<PyBackedBytes>() {
        return Ok(serde_json::Value::Array(
            bytes
                .as_ref()
                .iter()
                .map(|byte| serde_json::Value::Number((*byte as u64).into()))
                .collect(),
        ));
    }
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut out = Vec::with_capacity(list.len());
        for item in list {
            out.push(py_to_json(&item)?);
        }
        return Ok(serde_json::Value::Array(out));
    }
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut out = serde_json::Map::new();
        for (key, value) in dict {
            let key = key
                .extract::<String>()
                .map_err(|_| InvalidParamsError::new_err("snapshot option keys must be str"))?;
            out.insert(key, py_to_json(&value)?);
        }
        return Ok(serde_json::Value::Object(out));
    }
    Err(InvalidParamsError::new_err(format!(
        "unsupported snapshot option type: {}",
        obj.get_type().name()?,
    )))
}

fn open_database(
    database_name: Option<String>,
    options: PyDatabaseOpenOptions,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>, String> {
    if options.has_explicit_wal_options() {
        return Err(
            "wal_dir/snapshot_dir are not valid for Database.create(); use Database.open_wal()"
                .to_string(),
        );
    }
    let db = match database_name {
        Some(name) => InnerDatabase::open_named(name, options.named).map_err(|e| e.to_string())?,
        None => {
            if options.has_database_dir {
                return Err("database_name is required when database_dir is provided".to_string());
            }
            InnerDatabase::in_memory()
        }
    };
    Ok(Arc::new(db))
}

fn open_wal_database(
    options: PyDatabaseOpenOptions,
) -> Result<Arc<InnerDatabase<InMemoryGraph>>, String> {
    if options.has_database_dir {
        return Err("database_dir is not valid for Database.open_wal()".to_string());
    }
    let has_snapshot_tuning = options.has_snapshot_tuning_options();
    if options.snapshot_dir.is_none() && has_snapshot_tuning {
        return Err(
            "snapshot_dir is required when managed snapshot options are provided".to_string(),
        );
    }
    let wal_dir = options
        .wal_dir
        .ok_or_else(|| "wal_dir is required for Database.open_wal()".to_string())?;
    let wal_config = WalConfig::enabled(wal_dir);
    let db = if let Some(snapshot_dir) = options.snapshot_dir {
        let mut snapshots = SnapshotConfig::enabled(snapshot_dir)
            .keep_old(options.snapshot_keep_old.unwrap_or(1))
            .codec(options.snapshot_codec);
        if let Some(every) = options.snapshot_every_commits {
            if every != 0 {
                snapshots = snapshots.every_commits(every);
            }
        }
        InnerDatabase::open_with_wal_snapshots(wal_config, snapshots).map_err(|e| e.to_string())?
    } else {
        InnerDatabase::open_with_wal(wal_config).map_err(|e| e.to_string())?
    };
    Ok(Arc::new(db))
}

struct TransactionStatement {
    query: String,
    params: BTreeMap<String, LoraValue>,
}

fn parse_transaction_mode(mode: &str) -> PyResult<TransactionMode> {
    match mode {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(InvalidParamsError::new_err(format!(
            "unknown transaction mode '{other}'"
        ))),
    }
}

fn py_statements_to_transaction(
    statements: &Bound<'_, PyAny>,
) -> PyResult<Vec<TransactionStatement>> {
    let iter = statements
        .iter()
        .map_err(|_| InvalidParamsError::new_err("transaction statements must be an iterable"))?;
    let mut out = Vec::new();
    for item in iter {
        let item = item?;
        let mapping = item
            .downcast::<PyDict>()
            .map_err(|_| InvalidParamsError::new_err("transaction statement must be a mapping"))?;
        let query_any = mapping
            .get_item("query")?
            .ok_or_else(|| InvalidParamsError::new_err("transaction statement requires query"))?;
        let query = query_any.extract::<String>().map_err(|_| {
            InvalidParamsError::new_err("transaction statement query must be a string")
        })?;
        let params = match mapping.get_item("params")? {
            Some(params) if !params.is_none() => py_object_to_params(&params)?,
            _ => BTreeMap::new(),
        };
        out.push(TransactionStatement { query, params });
    }
    Ok(out)
}

fn row_arrays_to_py<'py>(
    py: Python<'py>,
    columns: &[String],
    rows: &[Vec<LoraValue>],
) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new_bound(py);
    let columns_py = PyList::new_bound(py, columns.iter().map(|c| c.as_str()));
    out.set_item("columns", columns_py)?;

    let rows_py = PyList::empty_bound(py);
    for row in rows {
        let py_row = PyDict::new_bound(py);
        for (col, val) in columns.iter().zip(row.iter()) {
            py_row.set_item(col, lora_value_to_py(py, val)?)?;
        }
        rows_py.append(py_row)?;
    }
    out.set_item("rows", rows_py)?;
    Ok(out)
}

fn row_to_py_dict<'py>(py: Python<'py>, row: &Row) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new_bound(py);
    for (_, name, value) in row.iter_named() {
        out.set_item(name.as_ref(), lora_value_to_py(py, value)?)?;
    }
    Ok(out)
}

// ============================================================================
// LoraValue → Python
// ============================================================================

fn lora_value_to_py<'py>(py: Python<'py>, value: &LoraValue) -> PyResult<Bound<'py, PyAny>> {
    match value {
        LoraValue::Null => Ok(py.None().into_bound(py)),
        LoraValue::Bool(b) => Ok(b.into_py(py).into_bound(py)),
        LoraValue::Int(i) => Ok(i.into_py(py).into_bound(py)),
        LoraValue::Float(f) => Ok(f.into_py(py).into_bound(py)),
        LoraValue::String(s) => Ok(s.into_py(py).into_bound(py)),
        LoraValue::Binary(b) => binary_to_py(py, b),
        LoraValue::List(items) => {
            let list = PyList::empty_bound(py);
            for item in items {
                list.append(lora_value_to_py(py, item)?)?;
            }
            Ok(list.into_any())
        }
        LoraValue::Map(m) => {
            let d = PyDict::new_bound(py);
            for (k, v) in m {
                d.set_item(k, lora_value_to_py(py, v)?)?;
            }
            Ok(d.into_any())
        }
        LoraValue::Node(id) => {
            let d = PyDict::new_bound(py);
            d.set_item("kind", "node")?;
            d.set_item("id", *id as i64)?;
            d.set_item("labels", PyList::empty_bound(py))?;
            d.set_item("properties", PyDict::new_bound(py))?;
            Ok(d.into_any())
        }
        LoraValue::Relationship(id) => {
            let d = PyDict::new_bound(py);
            d.set_item("kind", "relationship")?;
            d.set_item("id", *id as i64)?;
            Ok(d.into_any())
        }
        LoraValue::Path(p) => {
            let d = PyDict::new_bound(py);
            d.set_item("kind", "path")?;
            d.set_item(
                "nodes",
                PyList::new_bound(py, p.nodes.iter().map(|n| *n as i64)),
            )?;
            d.set_item(
                "rels",
                PyList::new_bound(py, p.rels.iter().map(|n| *n as i64)),
            )?;
            Ok(d.into_any())
        }
        LoraValue::Date(v) => tagged_iso(py, "date", v.to_string()),
        LoraValue::Time(v) => tagged_iso(py, "time", v.to_string()),
        LoraValue::LocalTime(v) => tagged_iso(py, "localtime", v.to_string()),
        LoraValue::DateTime(v) => tagged_iso(py, "datetime", v.to_string()),
        LoraValue::LocalDateTime(v) => tagged_iso(py, "localdatetime", v.to_string()),
        LoraValue::Duration(v) => tagged_iso(py, "duration", v.to_string()),
        LoraValue::Point(p) => point_to_py(py, p),
        LoraValue::Vector(v) => vector_to_py(py, v),
    }
}

fn binary_to_py<'py>(py: Python<'py>, b: &LoraBinary) -> PyResult<Bound<'py, PyAny>> {
    let d = PyDict::new_bound(py);
    d.set_item("kind", "binary")?;
    d.set_item("length", b.len())?;
    let segments = PyList::empty_bound(py);
    for segment in b.segments() {
        segments.append(PyBytes::new_bound(py, segment))?;
    }
    d.set_item("segments", segments)?;
    Ok(d.into_any())
}

/// Convert a `LoraVector` to the canonical tagged Python dict shape.
fn vector_to_py<'py>(py: Python<'py>, v: &LoraVector) -> PyResult<Bound<'py, PyAny>> {
    let d = PyDict::new_bound(py);
    d.set_item("kind", "vector")?;
    d.set_item("dimension", v.dimension as i64)?;
    d.set_item("coordinateType", v.coordinate_type().as_str())?;

    let values = PyList::empty_bound(py);
    match &v.values {
        VectorValues::Float64(vs) => {
            for x in vs {
                values.append(*x)?;
            }
        }
        VectorValues::Float32(vs) => {
            for x in vs {
                values.append(*x as f64)?;
            }
        }
        VectorValues::Integer64(vs) => {
            for x in vs {
                values.append(*x)?;
            }
        }
        VectorValues::Integer32(vs) => {
            for x in vs {
                values.append(*x as i64)?;
            }
        }
        VectorValues::Integer16(vs) => {
            for x in vs {
                values.append(*x as i64)?;
            }
        }
        VectorValues::Integer8(vs) => {
            for x in vs {
                values.append(*x as i64)?;
            }
        }
    }
    d.set_item("values", values)?;
    Ok(d.into_any())
}

fn tagged_iso<'py>(py: Python<'py>, kind: &str, iso: String) -> PyResult<Bound<'py, PyAny>> {
    let d = PyDict::new_bound(py);
    d.set_item("kind", kind)?;
    d.set_item("iso", iso)?;
    Ok(d.into_any())
}

/// Render a `LoraPoint` into the canonical external point shape exposed
/// to Python. Kept 1:1 aligned with the TS `LoraPoint` union emitted by
/// `lora-node` / `lora-wasm`:
///
/// - Cartesian: `{kind, srid, crs, x, y[, z]}`
/// - WGS-84: above plus `longitude`, `latitude`, and `height` (3D only)
fn point_to_py<'py>(py: Python<'py>, p: &LoraPoint) -> PyResult<Bound<'py, PyAny>> {
    let d = PyDict::new_bound(py);
    d.set_item("kind", "point")?;
    d.set_item("srid", p.srid)?;
    d.set_item("crs", p.crs_name())?;
    d.set_item("x", p.x)?;
    d.set_item("y", p.y)?;
    if let Some(z) = p.z {
        d.set_item("z", z)?;
    }
    if p.is_geographic() {
        d.set_item("longitude", p.longitude())?;
        d.set_item("latitude", p.latitude())?;
        if let Some(h) = p.height() {
            d.set_item("height", h)?;
        }
    }
    Ok(d.into_any())
}

// ============================================================================
// Python → LoraValue (params)
// ============================================================================

fn py_object_to_params(obj: &Bound<'_, PyAny>) -> PyResult<BTreeMap<String, LoraValue>> {
    let dict: &Bound<'_, PyDict> = obj.downcast::<PyDict>().map_err(|_| {
        InvalidParamsError::new_err("params must be a dict keyed by parameter name")
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in dict {
        let key: String = k
            .extract()
            .map_err(|_| InvalidParamsError::new_err("param keys must be str"))?;
        out.insert(key, py_to_lora_value(&v)?);
    }
    Ok(out)
}

fn py_to_lora_value(obj: &Bound<'_, PyAny>) -> PyResult<LoraValue> {
    if obj.is_none() {
        return Ok(LoraValue::Null);
    }
    // bool is a subclass of int in Python — check it first.
    if let Ok(b) = obj.downcast::<PyBool>() {
        return Ok(LoraValue::Bool(b.is_true()));
    }
    if let Ok(i) = obj.downcast::<PyInt>() {
        // i64 range; fall back to f64 if the int doesn't fit.
        return match i.extract::<i64>() {
            Ok(v) => Ok(LoraValue::Int(v)),
            Err(_) => Err(InvalidParamsError::new_err(
                "integer parameter does not fit in i64",
            )),
        };
    }
    if let Ok(f) = obj.downcast::<PyFloat>() {
        return Ok(LoraValue::Float(f.extract::<f64>()?));
    }
    if let Ok(s) = obj.downcast::<PyString>() {
        return Ok(LoraValue::String(s.extract::<String>()?));
    }
    if let Ok(bytes) = obj.extract::<PyBackedBytes>() {
        return Ok(LoraValue::Binary(LoraBinary::from_bytes(bytes.to_vec())));
    }
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut out = Vec::with_capacity(list.len());
        for item in list {
            out.push(py_to_lora_value(&item)?);
        }
        return Ok(LoraValue::List(out));
    }
    if let Ok(dict) = obj.downcast::<PyDict>() {
        return py_dict_to_cypher(dict);
    }
    Err(PyTypeError::new_err(format!(
        "unsupported parameter type: {}",
        obj.get_type().name()?,
    )))
}

fn py_dict_to_cypher(dict: &Bound<'_, PyDict>) -> PyResult<LoraValue> {
    // Tagged value?
    if let Some(kind_val) = dict.get_item("kind")? {
        if let Ok(kind) = kind_val.extract::<String>() {
            match kind.as_str() {
                "date" => {
                    return parse_tagged(dict, "date", |iso| {
                        LoraDate::parse(iso).map(LoraValue::Date)
                    })
                }
                "time" => {
                    return parse_tagged(dict, "time", |iso| {
                        LoraTime::parse(iso).map(LoraValue::Time)
                    })
                }
                "localtime" => {
                    return parse_tagged(dict, "localtime", |iso| {
                        LoraLocalTime::parse(iso).map(LoraValue::LocalTime)
                    })
                }
                "datetime" => {
                    return parse_tagged(dict, "datetime", |iso| {
                        LoraDateTime::parse(iso).map(LoraValue::DateTime)
                    })
                }
                "localdatetime" => {
                    return parse_tagged(dict, "localdatetime", |iso| {
                        LoraLocalDateTime::parse(iso).map(LoraValue::LocalDateTime)
                    })
                }
                "duration" => {
                    return parse_tagged(dict, "duration", |iso| {
                        LoraDuration::parse(iso).map(LoraValue::Duration)
                    })
                }
                "point" => {
                    let srid = dict
                        .get_item("srid")?
                        .map(|v| v.extract::<u32>())
                        .transpose()?
                        .unwrap_or(7203);
                    let x = dict
                        .get_item("x")?
                        .ok_or_else(|| InvalidParamsError::new_err("point.x required"))?
                        .extract::<f64>()?;
                    let y = dict
                        .get_item("y")?
                        .ok_or_else(|| InvalidParamsError::new_err("point.y required"))?
                        .extract::<f64>()?;
                    let z = dict
                        .get_item("z")?
                        .map(|v| v.extract::<f64>())
                        .transpose()?;
                    return Ok(LoraValue::Point(LoraPoint { x, y, z, srid }));
                }
                "vector" => {
                    return build_vector_from_dict(dict);
                }
                "binary" | "blob" => return build_binary_from_dict(dict),
                _ => { /* fall through to generic map */ }
            }
        }
    }

    let mut map = BTreeMap::new();
    for (k, v) in dict {
        let key: String = k
            .extract()
            .map_err(|_| PyTypeError::new_err("map keys must be str"))?;
        map.insert(key, py_to_lora_value(&v)?);
    }
    Ok(LoraValue::Map(map))
}

fn build_binary_from_dict(dict: &Bound<'_, PyDict>) -> PyResult<LoraValue> {
    let segments_obj = dict
        .get_item("segments")?
        .ok_or_else(|| InvalidParamsError::new_err("binary.segments required"))?;
    let segments = segments_obj
        .downcast::<PyList>()
        .map_err(|_| InvalidParamsError::new_err("binary.segments must be a list of bytes"))?;
    let mut out = Vec::with_capacity(segments.len());
    for segment in segments {
        let bytes = segment
            .extract::<PyBackedBytes>()
            .map_err(|_| InvalidParamsError::new_err("binary segment must be bytes"))?;
        out.push(bytes.to_vec());
    }
    Ok(LoraValue::Binary(LoraBinary::from_segments(out)))
}

fn parse_tagged(
    dict: &Bound<'_, PyDict>,
    tag: &str,
    parse: impl FnOnce(&str) -> Result<LoraValue, String>,
) -> PyResult<LoraValue> {
    let iso = dict
        .get_item("iso")?
        .ok_or_else(|| InvalidParamsError::new_err(format!("{tag} value requires iso: str")))?
        .extract::<String>()
        .map_err(|_| InvalidParamsError::new_err(format!("{tag}.iso must be str")))?;
    parse(&iso).map_err(|e| InvalidParamsError::new_err(format!("{tag}: {e}")))
}

fn build_vector_from_dict(dict: &Bound<'_, PyDict>) -> PyResult<LoraValue> {
    let dimension = dict
        .get_item("dimension")?
        .ok_or_else(|| InvalidParamsError::new_err("vector.dimension required"))?
        .extract::<i64>()?;
    let coordinate_type_name: String = dict
        .get_item("coordinateType")?
        .ok_or_else(|| InvalidParamsError::new_err("vector.coordinateType required"))?
        .extract()?;
    let coordinate_type = VectorCoordinateType::parse(&coordinate_type_name).ok_or_else(|| {
        InvalidParamsError::new_err(format!(
            "unknown vector coordinate type '{coordinate_type_name}'"
        ))
    })?;
    let values_obj = dict
        .get_item("values")?
        .ok_or_else(|| InvalidParamsError::new_err("vector.values required"))?;
    let values: Bound<'_, PyList> = values_obj
        .downcast_into::<PyList>()
        .map_err(|_| InvalidParamsError::new_err("vector.values must be a list"))?;

    let mut raw = Vec::with_capacity(values.len());
    for item in values.iter() {
        // bool is a subclass of int in Python — reject it explicitly so
        // `vector([True], 1, INTEGER)` doesn't silently become [1].
        if item.downcast::<PyBool>().is_ok() {
            return Err(InvalidParamsError::new_err(
                "vector.values entries must be numeric",
            ));
        }
        if let Ok(i) = item.extract::<i64>() {
            raw.push(RawCoordinate::Int(i));
        } else if let Ok(f) = item.extract::<f64>() {
            if !f.is_finite() {
                return Err(InvalidParamsError::new_err(
                    "vector.values cannot be NaN or Infinity",
                ));
            }
            raw.push(RawCoordinate::Float(f));
        } else {
            return Err(InvalidParamsError::new_err(
                "vector.values entries must be numeric",
            ));
        }
    }

    let v = LoraVector::try_new(raw, dimension, coordinate_type)
        .map_err(|e| InvalidParamsError::new_err(e.to_string()))?;
    Ok(LoraValue::Vector(v))
}

// Make the PyValueError path discoverable for future extensions.
#[allow(dead_code)]
fn pv(msg: impl Into<String>) -> PyErr {
    PyValueError::new_err(msg.into())
}
