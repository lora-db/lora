//! Direct napi builders for the *non-bulk* paths.
//!
//! `execute()` and `transaction()` route through [`crate::encode`] —
//! they ship a single `Buffer` to JS. The plan / profile / row paths
//! here build napi values directly because their results are small,
//! tree-shaped, and tower-of-objects rather than tabular; encoding
//! and re-decoding them would be net-negative.

use napi::bindgen_prelude::Result;
use napi::{Env, JsObject, JsUnknown};

use lora_database::{LoraValue, PlanShape, PlanTreeNode, QueryPlan, QueryProfile, Row};
use lora_store::{LoraBinary, LoraPoint, LoraVector, VectorValues};

pub(crate) fn plan_to_napi(env: &Env, plan: &QueryPlan) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("query", env.create_string(&plan.query)?)?;
    obj.set_named_property("shape", env.create_string(plan_shape_str(plan.shape))?)?;
    obj.set_named_property(
        "resultColumns",
        strings_to_array(env, &plan.result_columns)?,
    )?;
    obj.set_named_property("tree", plan_tree_node_to_napi(env, &plan.tree.root)?)?;
    Ok(obj)
}

pub(crate) fn profile_to_napi(env: &Env, profile: &QueryProfile) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("plan", plan_to_napi(env, &profile.plan)?)?;

    let mut metrics = env.create_object()?;
    metrics.set_named_property(
        "totalElapsedNs",
        env.create_double(profile.metrics.total_elapsed_ns as f64)?,
    )?;
    metrics.set_named_property(
        "totalRows",
        env.create_double(profile.metrics.total_rows as f64)?,
    )?;
    metrics.set_named_property("mutated", env.get_boolean(profile.metrics.mutated)?)?;

    let mut per_op = env.create_object()?;
    for (id, m) in &profile.metrics.per_operator {
        let mut entry = env.create_object()?;
        entry.set_named_property("rows", env.create_double(m.rows as f64)?)?;
        entry.set_named_property("dbHits", env.create_double(m.db_hits as f64)?)?;
        entry.set_named_property("elapsedNs", env.create_double(m.elapsed_ns as f64)?)?;
        entry.set_named_property("nextCalls", env.create_double(m.next_calls as f64)?)?;
        per_op.set_named_property(&id.to_string(), entry)?;
    }
    metrics.set_named_property("perOperator", per_op)?;
    obj.set_named_property("metrics", metrics)?;
    Ok(obj)
}

/// Build the row object yielded by the synchronous `streamNext()` path.
pub(crate) fn row_to_napi(env: &Env, row: &Row) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    for (_, name, value) in row.iter_named() {
        obj.set_named_property(&name, lora_value_to_napi(env, value)?)?;
    }
    Ok(obj)
}

// ---------------------------------------------------------------------------
// LoraValue dispatch
// ---------------------------------------------------------------------------

pub(crate) fn lora_value_to_napi(env: &Env, value: &LoraValue) -> Result<JsUnknown> {
    Ok(match value {
        LoraValue::Null => env.get_null()?.into_unknown(),
        LoraValue::Bool(b) => env.get_boolean(*b)?.into_unknown(),
        LoraValue::Int(i) => env.create_int64(*i)?.into_unknown(),
        LoraValue::Float(f) => env.create_double(*f)?.into_unknown(),
        LoraValue::String(s) => env.create_string(s)?.into_unknown(),
        LoraValue::Binary(b) => binary_to_napi(env, b)?.into_unknown(),
        LoraValue::List(items) => list_to_napi(env, items)?.into_unknown(),
        LoraValue::Map(m) => {
            let mut obj = env.create_object()?;
            for (k, v) in m {
                obj.set_named_property(k, lora_value_to_napi(env, v)?)?;
            }
            obj.into_unknown()
        }
        LoraValue::Node(id) => node_handle_to_napi(env, *id)?.into_unknown(),
        LoraValue::Relationship(id) => rel_handle_to_napi(env, *id)?.into_unknown(),
        LoraValue::Path(p) => {
            let mut obj = env.create_object()?;
            obj.set_named_property("kind", env.create_string("path")?)?;
            obj.set_named_property("nodes", ids_to_array(env, p.nodes.iter().copied())?)?;
            obj.set_named_property("rels", ids_to_array(env, p.rels.iter().copied())?)?;
            obj.into_unknown()
        }
        LoraValue::Date(d) => tagged_iso(env, "date", &d.to_string())?.into_unknown(),
        LoraValue::Time(t) => tagged_iso(env, "time", &t.to_string())?.into_unknown(),
        LoraValue::LocalTime(t) => tagged_iso(env, "localtime", &t.to_string())?.into_unknown(),
        LoraValue::DateTime(dt) => tagged_iso(env, "datetime", &dt.to_string())?.into_unknown(),
        LoraValue::LocalDateTime(dt) => {
            tagged_iso(env, "localdatetime", &dt.to_string())?.into_unknown()
        }
        LoraValue::Duration(d) => tagged_iso(env, "duration", &d.to_string())?.into_unknown(),
        LoraValue::Point(p) => point_to_napi(env, p)?.into_unknown(),
        LoraValue::Vector(v) => vector_to_napi(env, v)?.into_unknown(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn list_to_napi(env: &Env, items: &[LoraValue]) -> Result<JsObject> {
    let mut arr = env.create_array_with_length(items.len())?;
    for (i, item) in items.iter().enumerate() {
        arr.set_element(i as u32, lora_value_to_napi(env, item)?)?;
    }
    Ok(arr)
}

fn strings_to_array(env: &Env, strs: &[String]) -> Result<JsObject> {
    let mut arr = env.create_array_with_length(strs.len())?;
    for (i, s) in strs.iter().enumerate() {
        arr.set_element(i as u32, env.create_string(s)?)?;
    }
    Ok(arr)
}

fn ids_to_array(env: &Env, ids: impl ExactSizeIterator<Item = u64>) -> Result<JsObject> {
    let mut arr = env.create_array_with_length(ids.len())?;
    for (i, id) in ids.enumerate() {
        arr.set_element(i as u32, env.create_int64(id as i64)?)?;
    }
    Ok(arr)
}

fn tagged_iso(env: &Env, kind: &str, iso: &str) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("kind", env.create_string(kind)?)?;
    obj.set_named_property("iso", env.create_string(iso)?)?;
    Ok(obj)
}

fn node_handle_to_napi(env: &Env, id: u64) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("kind", env.create_string("node")?)?;
    obj.set_named_property("id", env.create_int64(id as i64)?)?;
    obj.set_named_property("labels", env.create_array_with_length(0)?)?;
    obj.set_named_property("properties", env.create_object()?)?;
    Ok(obj)
}

fn rel_handle_to_napi(env: &Env, id: u64) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("kind", env.create_string("relationship")?)?;
    obj.set_named_property("id", env.create_int64(id as i64)?)?;
    Ok(obj)
}

fn binary_to_napi(env: &Env, b: &LoraBinary) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("kind", env.create_string("binary")?)?;
    obj.set_named_property("length", env.create_double(b.len() as f64)?)?;
    let segments = b.segments();
    let mut arr = env.create_array_with_length(segments.len())?;
    for (i, seg) in segments.iter().enumerate() {
        let mut bytes = env.create_array_with_length(seg.len())?;
        for (j, &byte) in seg.iter().enumerate() {
            bytes.set_element(j as u32, env.create_int64(byte as i64)?)?;
        }
        arr.set_element(i as u32, bytes)?;
    }
    obj.set_named_property("segments", arr)?;
    Ok(obj)
}

fn point_to_napi(env: &Env, p: &LoraPoint) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("kind", env.create_string("point")?)?;
    obj.set_named_property("srid", env.create_int64(p.srid as i64)?)?;
    obj.set_named_property("crs", env.create_string(p.crs_name())?)?;
    obj.set_named_property("x", env.create_double(p.x)?)?;
    obj.set_named_property("y", env.create_double(p.y)?)?;
    if let Some(z) = p.z {
        obj.set_named_property("z", env.create_double(z)?)?;
    }
    if p.is_geographic() {
        obj.set_named_property("longitude", env.create_double(p.longitude())?)?;
        obj.set_named_property("latitude", env.create_double(p.latitude())?)?;
        if let Some(h) = p.height() {
            obj.set_named_property("height", env.create_double(h)?)?;
        }
    }
    Ok(obj)
}

fn vector_to_napi(env: &Env, v: &LoraVector) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("kind", env.create_string("vector")?)?;
    obj.set_named_property("dimension", env.create_int64(v.dimension as i64)?)?;
    obj.set_named_property(
        "coordinateType",
        env.create_string(v.coordinate_type().as_str())?,
    )?;
    let values = match &v.values {
        VectorValues::Float64(vs) => {
            let mut arr = env.create_array_with_length(vs.len())?;
            for (i, x) in vs.iter().enumerate() {
                arr.set_element(i as u32, env.create_double(*x)?)?;
            }
            arr
        }
        VectorValues::Float32(vs) => {
            let mut arr = env.create_array_with_length(vs.len())?;
            for (i, x) in vs.iter().enumerate() {
                arr.set_element(i as u32, env.create_double(*x as f64)?)?;
            }
            arr
        }
        VectorValues::Integer64(vs) => {
            let mut arr = env.create_array_with_length(vs.len())?;
            for (i, x) in vs.iter().enumerate() {
                arr.set_element(i as u32, env.create_int64(*x)?)?;
            }
            arr
        }
        VectorValues::Integer32(vs) => {
            let mut arr = env.create_array_with_length(vs.len())?;
            for (i, x) in vs.iter().enumerate() {
                arr.set_element(i as u32, env.create_int64(*x as i64)?)?;
            }
            arr
        }
        VectorValues::Integer16(vs) => {
            let mut arr = env.create_array_with_length(vs.len())?;
            for (i, x) in vs.iter().enumerate() {
                arr.set_element(i as u32, env.create_int64(*x as i64)?)?;
            }
            arr
        }
        VectorValues::Integer8(vs) => {
            let mut arr = env.create_array_with_length(vs.len())?;
            for (i, x) in vs.iter().enumerate() {
                arr.set_element(i as u32, env.create_int64(*x as i64)?)?;
            }
            arr
        }
    };
    obj.set_named_property("values", values)?;
    Ok(obj)
}

fn plan_tree_node_to_napi(env: &Env, node: &PlanTreeNode) -> Result<JsObject> {
    let mut obj = env.create_object()?;
    obj.set_named_property("id", env.create_double(node.id as f64)?)?;
    obj.set_named_property("operator", env.create_string(&node.operator)?)?;

    let mut details = env.create_object()?;
    for (k, v) in &node.details {
        details.set_named_property(k, env.create_string(v)?)?;
    }
    obj.set_named_property("details", details)?;

    match node.estimated_rows {
        Some(r) => obj.set_named_property("estimatedRows", env.create_double(r as f64)?)?,
        None => obj.set_named_property("estimatedRows", env.get_null()?)?,
    }

    let mut children = env.create_array_with_length(node.children.len())?;
    for (i, child) in node.children.iter().enumerate() {
        children.set_element(i as u32, plan_tree_node_to_napi(env, child)?)?;
    }
    obj.set_named_property("children", children)?;
    Ok(obj)
}

fn plan_shape_str(shape: PlanShape) -> &'static str {
    shape.as_str()
}
