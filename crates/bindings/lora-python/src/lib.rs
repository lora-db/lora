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

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyAny, PyBytes, PyDict, PyList};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraError as EngineLoraError,
    QueryResult, ResultFormat, SnapshotConfig, SnapshotOptions, WalConfig,
};

mod errors;
mod from_python;
mod stream;
mod to_python;

use stream::NativeStream;

use errors::{lora_query_err_from_anyhow, InvalidParamsError, LoraError, LoraQueryError};
use from_python::{
    has_attr, parse_transaction_mode, py_base64_decode, py_base64_encode, py_database_open_options,
    py_fspath, py_object_to_params, py_snapshot_credentials, py_snapshot_options,
    py_statements_to_transaction, PyDatabaseOpenOptions,
};
use to_python::{
    lora_value_to_py, query_plan_to_py, query_profile_to_py, row_arrays_to_py, row_to_py_dict,
    snapshot_info_to_meta, snapshot_meta_to_py,
};

// ============================================================================
// Module entry point
// ============================================================================

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
            Err(e) => return Err(lora_query_err_from_anyhow(e)),
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

    /// Compile a query and return its plan as a `dict` without
    /// executing it.
    ///
    /// Mutating queries (`CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE`)
    /// leave the graph untouched. Errors surface with the same
    /// `LoraQueryError` shape as `execute()`.
    #[pyo3(signature = (query, params=None))]
    fn explain<'py>(
        &self,
        py: Python<'py>,
        query: String,
        params: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let params_map = match params {
            Some(p) if !p.is_none() => Some(py_object_to_params(p)?),
            _ => None,
        };
        let db = self.inner()?;
        let plan = py
            .allow_threads(move || db.explain(&query, params_map))
            .map_err(lora_query_err_from_anyhow)?;
        query_plan_to_py(py, &plan)
    }

    /// Execute a query and return the plan plus runtime metrics as a
    /// `dict`.
    ///
    /// **PROFILE executes the query for real.** Mutating queries are
    /// persisted exactly as in `execute()`. Use `explain()` to inspect
    /// a mutating plan without running it.
    #[pyo3(signature = (query, params=None))]
    fn profile<'py>(
        &self,
        py: Python<'py>,
        query: String,
        params: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyDict>> {
        let params_map = match params {
            Some(p) if !p.is_none() => Some(py_object_to_params(p)?),
            _ => None,
        };
        let db = self.inner()?;
        let prof = py
            .allow_threads(move || db.profile(&query, params_map))
            .map_err(lora_query_err_from_anyhow)?;
        query_profile_to_py(py, &prof)
    }

    /// Return an iterator that pulls result rows one at a time. A mutating
    /// query holds the writer lock until the stream is exhausted (commit)
    /// or closed (rollback).
    #[pyo3(signature = (query, params=None))]
    fn stream<'py>(
        &self,
        py: Python<'py>,
        query: String,
        params: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<PyQueryStream> {
        let params_map = match params {
            Some(p) if !p.is_none() => py_object_to_params(p)?,
            _ => BTreeMap::new(),
        };
        let db = self.inner()?;
        // A mutating stream waits here for the writer lock, so the GIL is
        // released: the thread holding the lock may need it to finish.
        let (stream, columns) = py
            .allow_threads(move || NativeStream::open(&db, &query, params_map))
            .map_err(lora_query_err_from_anyhow)?;
        Ok(PyQueryStream {
            columns,
            stream: Mutex::new(Some(stream)),
        })
    }

    /// Whether `stream(query)` would open a mutating stream, i.e. one that
    /// holds the writer lock until it is exhausted or closed. Compiles the
    /// query into the plan cache (the stream then reuses the plan) without
    /// running it. Used by `AsyncDatabase.stream` to pick where to open it.
    fn _stream_is_mutating(&self, py: Python<'_>, query: String) -> PyResult<bool> {
        let db = self.inner()?;
        py.allow_threads(move || db.explain(&query, None))
            .map(|plan| plan.shape.is_mutating())
            .map_err(lora_query_err_from_anyhow)
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
            Ok::<_, EngineLoraError>(results)
        });

        let exec_results = exec_results.map_err(lora_query_err_from_anyhow)?;
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

    /// Drop every node and relationship. Constant-time, but waits for the
    /// writer lock, so it runs with the GIL released.
    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        let db = self.inner()?;
        py.allow_threads(move || db.clear());
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
            .map_err(lora_query_err_from_anyhow)?;
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
                .map_err(lora_query_err_from_anyhow)?;
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
                .map_err(lora_query_err_from_anyhow)?;
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
                .map_err(lora_query_err_from_anyhow)?;
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
                .map_err(lora_query_err_from_anyhow)?;
            return snapshot_meta_to_py(py, meta);
        }

        let path = py_fspath(source)?;
        let meta = py
            .allow_threads(move || {
                db.load_snapshot_from_with_credentials(&path, credentials.as_ref())
            })
            .map_err(lora_query_err_from_anyhow)?;
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

/// A row stream. Thread-safe: a mutating stream runs on its own thread
/// (see `stream.rs`), so `next()`, `close()` and the final drop may happen
/// on any Python thread, before or after the `Database` is closed.
#[pyclass(name = "QueryStream", module = "lora_python._native")]
pub struct PyQueryStream {
    columns: Vec<String>,
    stream: Mutex<Option<NativeStream>>,
}

impl PyQueryStream {
    fn slot(&self) -> PyResult<std::sync::MutexGuard<'_, Option<NativeStream>>> {
        self.stream
            .lock()
            .map_err(|_| LoraQueryError::new_err("query stream lock poisoned"))
    }
}

#[pymethods]
impl PyQueryStream {
    fn columns(&self) -> PyResult<Vec<String>> {
        match self.slot()?.as_ref() {
            Some(_) => Ok(self.columns.clone()),
            None => Err(LoraQueryError::new_err("query stream is closed")),
        }
    }

    /// Close the stream. An unfinished mutating stream rolls back and
    /// releases the writer lock before this returns.
    fn close(&self) -> PyResult<()> {
        let stream = self.slot()?.take();
        drop(stream);
        Ok(())
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        let mut slot = self.slot()?;
        let Some(stream) = slot.as_mut() else {
            return Ok(None);
        };
        // Pulling keeps the GIL: it never waits for a lock (a mutating
        // stream's actor took the writer lock when it opened and needs no
        // GIL to answer), and releasing the GIL per row would make every
        // row queue for it behind busy Python threads.
        match stream.next() {
            Ok(Some(row)) => Ok(Some(row_to_py_dict(py, &row)?)),
            Ok(None) => {
                slot.take();
                Ok(None)
            }
            Err(e) => {
                slot.take();
                Err(lora_query_err_from_anyhow(e))
            }
        }
    }
}

// ============================================================================
// Snapshot / open helpers
// ============================================================================

fn save_snapshot_to_vec(
    py: Python<'_>,
    db: Arc<InnerDatabase<InMemoryGraph>>,
    options: SnapshotOptions,
) -> PyResult<(Vec<u8>, lora_database::SnapshotMeta)> {
    let result = py.allow_threads(move || {
        db.save_snapshot_to_bytes_with_options(&options)
            .map(|(bytes, info)| (bytes, snapshot_info_to_meta(info)))
    });
    result.map_err(lora_query_err_from_anyhow)
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
