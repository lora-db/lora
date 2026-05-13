//! `value.*` — polymorphic operations that work over multiple value
//! types. Dispatch is by argument type, not by namespace, so callers
//! don't have to pick `node.keys` vs `map.keys` vs `edge.keys` when
//! they have a value of unknown shape. Runtime type inspection lives in
//! `type.*`; conversion lives in `cast.*`.

use lora_store::GraphStorage;

use super::super::expr::EvalContext;
use crate::value::LoraValue;

pub(super) fn dispatch<S: GraphStorage>(
    op: &str,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> Option<LoraValue> {
    Some(match op {
        "size" => size(args),
        "keys" => keys(args, ctx),
        "properties" => properties(args, ctx),
        "reverse" => reverse(args),
        "coalesce" | "first_non_null" => coalesce(args),
        "is_null" => is_null(args),
        "is_not_null" => is_not_null(args),
        "id" => id(args),
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn known(op: &str) -> Option<()> {
    matches!(
        op,
        "size" | "keys" | "properties" | "reverse" | "coalesce" | "is_null" | "is_not_null" | "id"
    )
    .then_some(())
}

fn id(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Node(id)) => LoraValue::Int(*id as i64),
        Some(LoraValue::Relationship(id)) => LoraValue::Int(*id as i64),
        _ => LoraValue::Null,
    }
}

fn size(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::List(l)) => LoraValue::Int(l.len() as i64),
        Some(LoraValue::String(s)) => LoraValue::Int(s.chars().count() as i64),
        Some(LoraValue::Map(m)) => LoraValue::Int(m.len() as i64),
        Some(LoraValue::Path(p)) => LoraValue::Int(p.rels.len() as i64),
        Some(LoraValue::Vector(v)) => LoraValue::Int(v.dimension as i64),
        Some(LoraValue::Binary(b)) => LoraValue::Int(b.len() as i64),
        _ => LoraValue::Null,
    }
}

fn keys<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
        Some(LoraValue::Map(m)) => {
            LoraValue::List(m.keys().cloned().map(LoraValue::String).collect())
        }
        Some(LoraValue::Node(id)) => ctx
            .storage
            .with_node(*id, |n| {
                LoraValue::List(
                    n.properties
                        .keys()
                        .map(|k| LoraValue::String(k.clone()))
                        .collect(),
                )
            })
            .unwrap_or(LoraValue::Null),
        Some(LoraValue::Relationship(id)) => ctx
            .storage
            .with_relationship(*id, |r| {
                LoraValue::List(
                    r.properties
                        .keys()
                        .map(|k| LoraValue::String(k.clone()))
                        .collect(),
                )
            })
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn properties<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
        Some(LoraValue::Map(m)) => LoraValue::Map(m.clone()),
        Some(LoraValue::Node(id)) => ctx
            .storage
            .with_node(*id, |n| {
                LoraValue::Map(
                    n.properties
                        .iter()
                        .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                        .collect(),
                )
            })
            .unwrap_or(LoraValue::Null),
        Some(LoraValue::Relationship(id)) => ctx
            .storage
            .with_relationship(*id, |r| {
                LoraValue::Map(
                    r.properties
                        .iter()
                        .map(|(k, v)| (k.clone(), LoraValue::from(v)))
                        .collect(),
                )
            })
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn reverse(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::List(l)) => LoraValue::List(l.iter().rev().cloned().collect()),
        Some(LoraValue::String(s)) => LoraValue::String(s.chars().rev().collect()),
        _ => LoraValue::Null,
    }
}

fn coalesce(args: &[LoraValue]) -> LoraValue {
    for arg in args {
        if !matches!(arg, LoraValue::Null) {
            return arg.clone();
        }
    }
    LoraValue::Null
}

fn is_null(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Null) | None => LoraValue::Bool(true),
        Some(_) => LoraValue::Bool(false),
    }
}

fn is_not_null(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Null) | None => LoraValue::Bool(false),
        Some(_) => LoraValue::Bool(true),
    }
}
