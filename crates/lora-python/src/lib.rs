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
use pyo3::types::{PyAny, PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

use lora_database::{
    Database as InnerDatabase, ExecuteOptions, InMemoryGraph, LoraValue, QueryResult, ResultFormat,
};
use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint, LoraTime,
    LoraVector, RawCoordinate, VectorCoordinateType, VectorValues,
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

/// Synchronous in-memory Lora graph database handle.
///
/// Query execution runs with the GIL released so other Python threads —
/// notably `asyncio.to_thread` workers — can progress in parallel. Concurrent
/// calls against the same `Database` serialise on an internal mutex but do
/// not hold the GIL.
#[pyclass(module = "lora_python._native")]
pub struct Database {
    store: Arc<Mutex<InMemoryGraph>>,
}

#[pymethods]
impl Database {
    #[new]
    fn py_new() -> Self {
        Self {
            store: Arc::new(Mutex::new(InMemoryGraph::new())),
        }
    }

    /// Factory mirroring the async API shape. Returns `self` for symmetry
    /// with `AsyncDatabase.create()`.
    #[staticmethod]
    fn create() -> Self {
        Self::py_new()
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

        let store = Arc::clone(&self.store);
        // Release the GIL for the duration of engine work.
        let exec_result = py.allow_threads(move || {
            let db = InnerDatabase::new(store);
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

    /// Drop every node and relationship. Constant-time.
    fn clear(&self) {
        let mut guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        *guard = InMemoryGraph::new();
    }

    /// Number of nodes currently in the graph.
    #[getter]
    fn node_count(&self) -> u64 {
        use lora_store::GraphStorage;
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        guard.node_count() as u64
    }

    /// Number of relationships currently in the graph.
    #[getter]
    fn relationship_count(&self) -> u64 {
        use lora_store::GraphStorage;
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        guard.relationship_count() as u64
    }

    fn __repr__(&self) -> String {
        use lora_store::GraphStorage;
        let guard = self.store.lock().unwrap_or_else(|p| p.into_inner());
        format!(
            "<lora_python.Database nodes={} relationships={}>",
            guard.node_count(),
            guard.relationship_count(),
        )
    }
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
