//! `edge.*` — operations whose primary input is a RELATIONSHIP.

use lora_store::GraphStorage;

use super::super::expr::EvalContext;
use crate::value::LoraValue;

pub(super) fn dispatch<S: GraphStorage>(
    op: &str,
    args: &[LoraValue],
    ctx: &EvalContext<'_, S>,
) -> Option<LoraValue> {
    Some(match op {
        "id" => id(args),
        "type" => edge_type(args, ctx),
        "keys" => keys(args, ctx),
        "properties" => properties(args, ctx),
        "start" => start(args, ctx),
        "end" => end(args, ctx),
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn known(op: &str) -> Option<()> {
    matches!(op, "id" | "type" | "keys" | "properties" | "start" | "end").then_some(())
}

fn id(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Relationship(id)) => LoraValue::Int(*id as i64),
        _ => LoraValue::Null,
    }
}

fn edge_type<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
        Some(LoraValue::Relationship(id)) => ctx
            .storage
            .with_relationship(*id, |r| LoraValue::String(r.rel_type.clone()))
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn keys<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
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

fn start<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
        Some(LoraValue::Relationship(id)) => ctx
            .storage
            .with_relationship(*id, |r| LoraValue::Node(r.src))
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn end<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
        Some(LoraValue::Relationship(id)) => ctx
            .storage
            .with_relationship(*id, |r| LoraValue::Node(r.dst))
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}
