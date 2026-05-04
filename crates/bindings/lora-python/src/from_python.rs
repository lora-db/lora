//! Python → Rust conversion helpers for the PyO3 bindings.
//!
//! Splits naturally into three flows used by `lib.rs::Database`:
//!
//! * **Params** — `py_object_to_params` walks a Python `dict` keyed by
//!   parameter name and lowers each value into a [`LoraValue`] using the
//!   shared tagged-dict contract (date/time/point/vector/binary
//!   carriers).
//! * **Open / snapshot options** — `py_database_open_options`,
//!   `py_snapshot_options`, `py_snapshot_credentials` accept a
//!   `dict | None` and emit the matching `lora_database` config struct.
//!   Both `snake_case` and `camelCase` keys are honoured so callers can
//!   share JSON shapes with the Node binding.
//! * **Misc utilities** — `py_fspath`, `py_base64_encode/decode`,
//!   `has_attr`, plus the transaction-statement parser used by
//!   `Database.transaction`.

use std::collections::BTreeMap;

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyAny, PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString};

use lora_database::{
    snapshot_credentials_from_json, snapshot_options_from_json, DatabaseOpenOptions, LoraValue,
    SnapshotCredentials, SnapshotOptions, TransactionMode,
};
use lora_store::{
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType,
};

use crate::errors::InvalidParamsError;

// ============================================================================
// Database open / snapshot options
// ============================================================================

pub(crate) struct PyDatabaseOpenOptions {
    pub(crate) named: DatabaseOpenOptions,
    pub(crate) has_database_dir: bool,
    pub(crate) wal_dir: Option<String>,
    pub(crate) snapshot_dir: Option<String>,
    pub(crate) snapshot_every_commits: Option<u64>,
    pub(crate) snapshot_keep_old: Option<usize>,
    pub(crate) has_snapshot_codec: bool,
    pub(crate) snapshot_codec: SnapshotOptions,
}

impl PyDatabaseOpenOptions {
    pub(crate) fn has_explicit_wal_options(&self) -> bool {
        self.wal_dir.is_some()
            || self.snapshot_dir.is_some()
            || self.snapshot_every_commits.is_some()
            || self.snapshot_keep_old.is_some()
            || self.has_snapshot_codec
    }

    pub(crate) fn has_snapshot_tuning_options(&self) -> bool {
        self.snapshot_every_commits.is_some()
            || self.snapshot_keep_old.is_some()
            || self.has_snapshot_codec
    }
}

pub(crate) fn py_database_open_options(
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
        out.snapshot_codec = snapshot_options_from_json(Some(py_to_json(&value)?))
            .map_err(|e| InvalidParamsError::new_err(format!("invalid snapshot options: {e}")))?;
    }
    Ok(out)
}

pub(crate) fn py_snapshot_options(options: Option<&Bound<'_, PyAny>>) -> PyResult<SnapshotOptions> {
    let json = py_optional_to_json(options)?;
    snapshot_options_from_json(json)
        .map_err(|e| InvalidParamsError::new_err(format!("invalid snapshot options: {e}")))
}

pub(crate) fn py_snapshot_credentials(
    options: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<SnapshotCredentials>> {
    let json = py_optional_to_json(options)?;
    snapshot_credentials_from_json(json)
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

// ============================================================================
// Path / bytes / dispatcher helpers
// ============================================================================

pub(crate) fn has_attr(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<bool> {
    obj.hasattr(name)
}

pub(crate) fn py_fspath(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    let os = obj.py().import_bound("os")?;
    let path = os.getattr("fspath")?.call1((obj,))?;
    path.extract::<String>().map_err(|_| {
        PyTypeError::new_err("snapshot path must be str, os.PathLike[str], bytes, or a stream")
    })
}

pub(crate) fn py_base64_encode(py: Python<'_>, bytes: &[u8]) -> PyResult<Py<PyAny>> {
    let base64 = py.import_bound("base64")?;
    let encoded = base64
        .getattr("b64encode")?
        .call1((PyBytes::new_bound(py, bytes),))?;
    let text = encoded.call_method1("decode", ("ascii",))?;
    Ok(text.unbind())
}

pub(crate) fn py_base64_decode(py: Python<'_>, source: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let base64 = py.import_bound("base64")?;
    let decoded = base64.getattr("b64decode")?.call1((source,))?;
    decoded
        .extract::<PyBackedBytes>()
        .map(|bytes| bytes.as_ref().to_vec())
        .map_err(|e| PyTypeError::new_err(format!("invalid base64 snapshot: {e}")))
}

// ============================================================================
// Transaction statement parsing
// ============================================================================

pub(crate) struct TransactionStatement {
    pub(crate) query: String,
    pub(crate) params: BTreeMap<String, LoraValue>,
}

pub(crate) fn parse_transaction_mode(mode: &str) -> PyResult<TransactionMode> {
    match mode {
        "read_write" | "readwrite" | "rw" => Ok(TransactionMode::ReadWrite),
        "read_only" | "readonly" | "ro" => Ok(TransactionMode::ReadOnly),
        other => Err(InvalidParamsError::new_err(format!(
            "unknown transaction mode '{other}'"
        ))),
    }
}

pub(crate) fn py_statements_to_transaction(
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

// ============================================================================
// Params (Python → LoraValue)
// ============================================================================

pub(crate) fn py_object_to_params(obj: &Bound<'_, PyAny>) -> PyResult<BTreeMap<String, LoraValue>> {
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
            "unknown vector coordinate type `{coordinate_type_name}`"
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
