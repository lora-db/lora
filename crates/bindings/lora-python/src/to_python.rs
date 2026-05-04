//! `LoraValue` → Python conversion helpers for the PyO3 bindings.
//!
//! Mirrors the tagged-dict shape used by `lora-node` and `lora-wasm`:
//! primitives pass through as Python natives, while graph, temporal,
//! and spatial values become `dict`s carrying a `"kind"` discriminator
//! that the pure-Python wrapper layer can decode.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use lora_database::{LoraValue, Row, SnapshotInfo, SnapshotMeta};
use lora_store::{LoraBinary, LoraPoint, LoraVector, VectorValues};

pub(crate) fn snapshot_info_to_meta(info: SnapshotInfo) -> SnapshotMeta {
    SnapshotMeta {
        format_version: info.format_version,
        node_count: info.node_count,
        relationship_count: info.relationship_count,
        wal_lsn: info.wal_lsn,
    }
}

pub(crate) fn snapshot_meta_to_py<'py>(
    py: Python<'py>,
    meta: SnapshotMeta,
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

pub(crate) fn row_arrays_to_py<'py>(
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

pub(crate) fn row_to_py_dict<'py>(py: Python<'py>, row: &Row) -> PyResult<Bound<'py, PyDict>> {
    let out = PyDict::new_bound(py);
    for (_, name, value) in row.iter_named() {
        out.set_item(name.as_ref(), lora_value_to_py(py, value)?)?;
    }
    Ok(out)
}

pub(crate) fn lora_value_to_py<'py>(
    py: Python<'py>,
    value: &LoraValue,
) -> PyResult<Bound<'py, PyAny>> {
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
