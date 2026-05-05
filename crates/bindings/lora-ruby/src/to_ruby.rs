//! `LoraValue` → Ruby conversion.
//!
//! Primitives map to Ruby natives (`Integer`, `Float`, `String`, …);
//! graph, temporal, and spatial values become tagged `Hash`es with a
//! `"kind"` discriminator, matching the shared cross-binding contract.

use magnus::{prelude::*, Error as MagnusError, RHash, Ruby, Value};

use lora_database::{LoraValue, PlanTreeNode, QueryPlan, QueryProfile};
use lora_store::{LoraBinary, LoraPoint, LoraVector, VectorValues};

pub(crate) fn query_plan_to_ruby(ruby: &Ruby, plan: &QueryPlan) -> Result<RHash, MagnusError> {
    let out = ruby.hash_new();
    out.aset(ruby.str_new("query"), ruby.str_new(&plan.query))?;
    out.aset(ruby.str_new("shape"), ruby.str_new(plan.shape.as_str()))?;
    let cols = ruby.ary_new();
    for c in &plan.result_columns {
        cols.push(ruby.str_new(c))?;
    }
    out.aset(ruby.str_new("result_columns"), cols)?;
    out.aset(
        ruby.str_new("tree"),
        plan_tree_node_to_ruby(ruby, &plan.tree.root)?,
    )?;
    Ok(out)
}

pub(crate) fn query_profile_to_ruby(
    ruby: &Ruby,
    profile: &QueryProfile,
) -> Result<RHash, MagnusError> {
    let out = ruby.hash_new();
    out.aset(
        ruby.str_new("plan"),
        query_plan_to_ruby(ruby, &profile.plan)?,
    )?;

    let metrics = ruby.hash_new();
    metrics.aset(
        ruby.str_new("total_elapsed_ns"),
        profile.metrics.total_elapsed_ns,
    )?;
    metrics.aset(ruby.str_new("total_rows"), profile.metrics.total_rows)?;
    metrics.aset(ruby.str_new("mutated"), profile.metrics.mutated)?;

    let per_op = ruby.hash_new();
    for (id, op) in &profile.metrics.per_operator {
        let entry = ruby.hash_new();
        entry.aset(ruby.str_new("rows"), op.rows)?;
        entry.aset(ruby.str_new("db_hits"), op.db_hits)?;
        entry.aset(ruby.str_new("elapsed_ns"), op.elapsed_ns)?;
        entry.aset(ruby.str_new("next_calls"), op.next_calls)?;
        per_op.aset(*id as u64, entry)?;
    }
    metrics.aset(ruby.str_new("per_operator"), per_op)?;

    out.aset(ruby.str_new("metrics"), metrics)?;
    Ok(out)
}

fn plan_tree_node_to_ruby(ruby: &Ruby, node: &PlanTreeNode) -> Result<Value, MagnusError> {
    let out = ruby.hash_new();
    out.aset(ruby.str_new("id"), node.id as u64)?;
    out.aset(ruby.str_new("operator"), ruby.str_new(&node.operator))?;
    let details = ruby.hash_new();
    for (k, v) in &node.details {
        details.aset(ruby.str_new(k), ruby.str_new(v))?;
    }
    out.aset(ruby.str_new("details"), details)?;
    match node.estimated_rows {
        Some(r) => out.aset(ruby.str_new("estimated_rows"), r)?,
        None => out.aset(ruby.str_new("estimated_rows"), ruby.qnil())?,
    }
    let children = ruby.ary_new();
    for child in &node.children {
        children.push(plan_tree_node_to_ruby(ruby, child)?)?;
    }
    out.aset(ruby.str_new("children"), children)?;
    Ok(out.as_value())
}

pub(crate) fn lora_value_to_ruby(ruby: &Ruby, value: &LoraValue) -> Result<Value, MagnusError> {
    match value {
        LoraValue::Null => Ok(ruby.qnil().as_value()),
        LoraValue::Bool(b) => Ok(if *b {
            ruby.qtrue().as_value()
        } else {
            ruby.qfalse().as_value()
        }),
        LoraValue::Int(i) => Ok(ruby.integer_from_i64(*i).as_value()),
        LoraValue::Float(f) => Ok(ruby.float_from_f64(*f).as_value()),
        LoraValue::String(s) => Ok(ruby.str_new(s).as_value()),
        LoraValue::List(items) => {
            let arr = ruby.ary_new();
            for item in items {
                arr.push(lora_value_to_ruby(ruby, item)?)?;
            }
            Ok(arr.as_value())
        }
        LoraValue::Map(m) => {
            let h = ruby.hash_new();
            for (k, v) in m {
                h.aset(ruby.str_new(k), lora_value_to_ruby(ruby, v)?)?;
            }
            Ok(h.as_value())
        }
        LoraValue::Node(id) => {
            let h = ruby.hash_new();
            h.aset(ruby.str_new("kind"), ruby.str_new("node"))?;
            h.aset(ruby.str_new("id"), ruby.integer_from_i64(*id as i64))?;
            h.aset(ruby.str_new("labels"), ruby.ary_new())?;
            h.aset(ruby.str_new("properties"), ruby.hash_new())?;
            Ok(h.as_value())
        }
        LoraValue::Relationship(id) => {
            let h = ruby.hash_new();
            h.aset(ruby.str_new("kind"), ruby.str_new("relationship"))?;
            h.aset(ruby.str_new("id"), ruby.integer_from_i64(*id as i64))?;
            Ok(h.as_value())
        }
        LoraValue::Path(p) => {
            let h = ruby.hash_new();
            h.aset(ruby.str_new("kind"), ruby.str_new("path"))?;
            let nodes = ruby.ary_new();
            for n in &p.nodes {
                nodes.push(ruby.integer_from_i64(*n as i64))?;
            }
            let rels = ruby.ary_new();
            for r in &p.rels {
                rels.push(ruby.integer_from_i64(*r as i64))?;
            }
            h.aset(ruby.str_new("nodes"), nodes)?;
            h.aset(ruby.str_new("rels"), rels)?;
            Ok(h.as_value())
        }
        LoraValue::Date(v) => tagged_iso(ruby, "date", v.to_string()),
        LoraValue::Time(v) => tagged_iso(ruby, "time", v.to_string()),
        LoraValue::LocalTime(v) => tagged_iso(ruby, "localtime", v.to_string()),
        LoraValue::DateTime(v) => tagged_iso(ruby, "datetime", v.to_string()),
        LoraValue::LocalDateTime(v) => tagged_iso(ruby, "localdatetime", v.to_string()),
        LoraValue::Duration(v) => tagged_iso(ruby, "duration", v.to_string()),
        LoraValue::Point(p) => point_to_ruby(ruby, p),
        LoraValue::Vector(v) => vector_to_ruby(ruby, v),
        LoraValue::Binary(v) => binary_to_ruby(ruby, v),
    }
}

fn binary_to_ruby(ruby: &Ruby, value: &LoraBinary) -> Result<Value, MagnusError> {
    let h = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new("binary"))?;
    h.aset(
        ruby.str_new("length"),
        ruby.integer_from_i64(value.len() as i64),
    )?;
    let segments = ruby.ary_new();
    for segment in value.segments() {
        segments.push(ruby.str_from_slice(segment))?;
    }
    h.aset(ruby.str_new("segments"), segments)?;
    Ok(h.as_value())
}

fn vector_to_ruby(ruby: &Ruby, v: &LoraVector) -> Result<Value, MagnusError> {
    let h = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new("vector"))?;
    h.aset(
        ruby.str_new("dimension"),
        ruby.integer_from_i64(v.dimension as i64),
    )?;
    h.aset(
        ruby.str_new("coordinateType"),
        ruby.str_new(v.coordinate_type().as_str()),
    )?;

    let values = ruby.ary_new();
    match &v.values {
        VectorValues::Float64(vs) => {
            for x in vs {
                values.push(ruby.float_from_f64(*x))?;
            }
        }
        VectorValues::Float32(vs) => {
            for x in vs {
                values.push(ruby.float_from_f64(*x as f64))?;
            }
        }
        VectorValues::Integer64(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x))?;
            }
        }
        VectorValues::Integer32(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x as i64))?;
            }
        }
        VectorValues::Integer16(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x as i64))?;
            }
        }
        VectorValues::Integer8(vs) => {
            for x in vs {
                values.push(ruby.integer_from_i64(*x as i64))?;
            }
        }
    }
    h.aset(ruby.str_new("values"), values)?;
    Ok(h.as_value())
}

fn tagged_iso(ruby: &Ruby, kind: &str, iso: String) -> Result<Value, MagnusError> {
    let h: RHash = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new(kind))?;
    h.aset(ruby.str_new("iso"), ruby.str_new(&iso))?;
    Ok(h.as_value())
}

/// Render a `LoraPoint` into the canonical external point shape — kept
/// 1:1 aligned with the `LoraPoint` union emitted by `lora-node` /
/// `lora-wasm` / `lora-python`.
fn point_to_ruby(ruby: &Ruby, p: &LoraPoint) -> Result<Value, MagnusError> {
    let h = ruby.hash_new();
    h.aset(ruby.str_new("kind"), ruby.str_new("point"))?;
    h.aset(ruby.str_new("srid"), ruby.integer_from_i64(p.srid as i64))?;
    h.aset(ruby.str_new("crs"), ruby.str_new(p.crs_name()))?;
    h.aset(ruby.str_new("x"), ruby.float_from_f64(p.x))?;
    h.aset(ruby.str_new("y"), ruby.float_from_f64(p.y))?;
    if let Some(z) = p.z {
        h.aset(ruby.str_new("z"), ruby.float_from_f64(z))?;
    }
    if p.is_geographic() {
        h.aset(
            ruby.str_new("longitude"),
            ruby.float_from_f64(p.longitude()),
        )?;
        h.aset(ruby.str_new("latitude"), ruby.float_from_f64(p.latitude()))?;
        if let Some(height) = p.height() {
            h.aset(ruby.str_new("height"), ruby.float_from_f64(height))?;
        }
    }
    Ok(h.as_value())
}
