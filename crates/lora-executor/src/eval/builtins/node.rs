//! `node.*` — operations whose primary input is a NODE.

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
        "labels" => labels(args, ctx),
        "has_label" => has_label(args, ctx),
        "keys" => keys(args, ctx),
        "properties" => properties(args, ctx),
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn known(op: &str) -> Option<()> {
    matches!(op, "id" | "labels" | "has_label" | "keys" | "properties").then_some(())
}

fn id(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Node(id)) => LoraValue::Int(*id as i64),
        _ => LoraValue::Null,
    }
}

fn labels<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
        Some(LoraValue::Node(id)) => ctx
            .storage
            .with_node(*id, |n| {
                LoraValue::List(
                    n.labels
                        .iter()
                        .map(|s| LoraValue::String(s.clone()))
                        .collect(),
                )
            })
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn has_label<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match (args.first(), args.get(1)) {
        (Some(LoraValue::Node(id)), Some(LoraValue::String(label))) => ctx
            .storage
            .with_node(*id, |n| {
                LoraValue::Bool(n.labels.iter().any(|l| l == label))
            })
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn keys<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
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
        _ => LoraValue::Null,
    }
}

fn properties<S: GraphStorage>(args: &[LoraValue], ctx: &EvalContext<'_, S>) -> LoraValue {
    match args.first() {
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
        _ => LoraValue::Null,
    }
}
