//! `index.*` — query a full-text or vector index from inside a query.
//!
//! These back `CALL db.index.{fulltext,vector}.query{Nodes,Relationships}
//! (...) YIELD ...` used as a clause: the analyzer rewrites the call into
//! `UNWIND index.<op>(...) AS hit` followed by a projection of the
//! yielded fields, so the hits flow into the rest of the query like any
//! other bound rows. Each hit is a map `{node|relationship, score}`,
//! highest score first (ties broken by id, matching standalone CALL).

use std::collections::{BTreeMap, BTreeSet};

use lora_store::{
    GraphStorage, IndexConfigValue, LoraVector, RawCoordinate, StoredIndexEntity, StoredIndexKind,
    VectorCoordinateType,
};

use super::super::errors::set_eval_error;
use super::super::expr::EvalContext;
use crate::value::LoraValue;

pub(super) fn dispatch<S: GraphStorage>(
    op: &str,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> Option<LoraValue> {
    let result = match op {
        "fulltext_nodes" => fulltext(args, ctx, StoredIndexEntity::Node),
        "fulltext_relationships" => fulltext(args, ctx, StoredIndexEntity::Relationship),
        "vector_nodes" => vector(args, ctx, StoredIndexEntity::Node),
        "vector_relationships" => vector(args, ctx, StoredIndexEntity::Relationship),
        _ => return None,
    };
    Some(result.unwrap_or_else(|msg| {
        set_eval_error(msg);
        LoraValue::Null
    }))
}

#[cfg(test)]
pub(super) fn known(op: &str) -> Option<()> {
    matches!(
        op,
        "fulltext_nodes" | "fulltext_relationships" | "vector_nodes" | "vector_relationships"
    )
    .then_some(())
}

fn fulltext<S: GraphStorage>(
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
    entity: StoredIndexEntity,
) -> Result<LoraValue, String> {
    let name = string_arg(args, 0, "indexName")?;
    let query = string_arg(args, 1, "queryString")?;
    check_index(ctx, &name, StoredIndexKind::Fulltext, entity)?;
    Ok(hits(
        ctx.storage.fulltext_search(&name, &query),
        None,
        entity,
    ))
}

fn vector<S: GraphStorage>(
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
    entity: StoredIndexEntity,
) -> Result<LoraValue, String> {
    let name = string_arg(args, 0, "indexName")?;
    let k = match args.get(1) {
        Some(LoraValue::Int(n)) if *n > 0 => *n as usize,
        other => return Err(format!("k must be a positive integer, got {other:?}")),
    };
    let query = match args.get(2) {
        Some(LoraValue::Vector(v)) => v.clone(),
        Some(LoraValue::List(items)) => float32_vector(items)?,
        other => {
            return Err(format!(
                "query must be a VECTOR or LIST<NUMBER>, got {other:?}"
            ))
        }
    };
    let restrict_to = match args.get(3) {
        None | Some(LoraValue::Null) => None,
        Some(LoraValue::Map(options)) => restrict_to(options)?,
        Some(other) => return Err(format!("vector options must be a MAP, got {other:?}")),
    };
    let def = check_index(ctx, &name, StoredIndexKind::Vector, entity)?;
    if let Some(IndexConfigValue::Integer(dim)) = def.options.get("vector.dimensions") {
        if query.dimension as i64 != *dim {
            return Err(format!(
                "query vector has dimension {} but index `{name}` expects {dim}",
                query.dimension
            ));
        }
    }
    let scored = ctx
        .storage
        .vector_search(&name, &query, k, restrict_to.as_ref());
    Ok(hits(scored, Some(k), entity))
}

fn check_index<S: GraphStorage>(
    ctx: &EvalContext<'_, S>,
    name: &str,
    kind: StoredIndexKind,
    entity: StoredIndexEntity,
) -> Result<lora_store::IndexDefinition, String> {
    let def = ctx
        .storage
        .get_index(name)
        .ok_or_else(|| format!("no {} index named `{name}`", kind.as_str()))?;
    if def.kind != kind {
        return Err(format!(
            "index `{name}` is not a {} index (kind={})",
            kind.as_str(),
            def.kind.as_str()
        ));
    }
    if def.entity != entity {
        return Err(format!(
            "index `{name}` is on {} entities; procedure expects {}",
            def.entity.as_str(),
            entity.as_str()
        ));
    }
    Ok(def)
}

fn hits(mut scored: Vec<(u64, f64)>, limit: Option<usize>, entity: StoredIndexEntity) -> LoraValue {
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    if let Some(limit) = limit {
        scored.truncate(limit);
    }
    let (key, wrap): (&str, fn(u64) -> LoraValue) = match entity {
        StoredIndexEntity::Node => ("node", LoraValue::Node),
        StoredIndexEntity::Relationship => ("relationship", LoraValue::Relationship),
    };
    LoraValue::List(
        scored
            .into_iter()
            .map(|(id, score)| {
                LoraValue::Map(BTreeMap::from([
                    (key.to_string(), wrap(id)),
                    ("score".to_string(), LoraValue::Float(score)),
                ]))
            })
            .collect(),
    )
}

fn string_arg(args: &[LoraValue], idx: usize, label: &str) -> Result<String, String> {
    match args.get(idx) {
        Some(LoraValue::String(s)) => Ok(s.clone()),
        other => Err(format!("{label} must be a string, got {other:?}")),
    }
}

fn float32_vector(items: &[LoraValue]) -> Result<LoraVector, String> {
    let raw = items
        .iter()
        .map(|item| match item {
            LoraValue::Int(n) => Ok(RawCoordinate::Int(*n)),
            LoraValue::Float(f) => Ok(RawCoordinate::Float(*f)),
            other => Err(format!(
                "query list elements must be numbers, got {other:?}"
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dim = raw.len() as i64;
    LoraVector::try_new(raw, dim, VectorCoordinateType::Float32)
        .map_err(|e| format!("invalid query vector: {e}"))
}

fn restrict_to(options: &BTreeMap<String, LoraValue>) -> Result<Option<BTreeSet<u64>>, String> {
    let mut out = None;
    for (key, value) in options {
        match key.as_str() {
            "restrictTo" => {
                let LoraValue::List(items) = value else {
                    return Err(format!("`restrictTo` must be a LIST of ids, got {value:?}"));
                };
                let mut ids = BTreeSet::new();
                for item in items {
                    match item {
                        LoraValue::Int(n) if *n >= 0 => {
                            ids.insert(*n as u64);
                        }
                        LoraValue::Node(id) | LoraValue::Relationship(id) => {
                            ids.insert(*id);
                        }
                        other => {
                            return Err(format!("`restrictTo` entries must be ids, got {other:?}"))
                        }
                    }
                }
                out = Some(ids);
            }
            other => {
                return Err(format!(
                    "unknown vector option `{other}` (known: `restrictTo`)"
                ))
            }
        }
    }
    Ok(out)
}
