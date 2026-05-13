//! `path.*` — operations on PATH values.

use lora_store::GraphStorage;

use super::super::expr::EvalContext;
use crate::value::LoraValue;

pub(super) fn dispatch<S: GraphStorage>(
    op: &str,
    args: &[LoraValue],
    _ctx: &EvalContext<'_, S>,
) -> Option<LoraValue> {
    Some(match op {
        "nodes" => nodes(args),
        "edges" => edges(args),
        "length" => length(args),
        "first" => first(args),
        "last" => last(args),
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn known(op: &str) -> Option<()> {
    matches!(op, "nodes" | "edges" | "length" | "first" | "last").then_some(())
}

fn nodes(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Path(p)) => {
            LoraValue::List(p.nodes.iter().map(|id| LoraValue::Node(*id)).collect())
        }
        _ => LoraValue::Null,
    }
}

fn edges(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Path(p)) => LoraValue::List(
            p.rels
                .iter()
                .map(|id| LoraValue::Relationship(*id))
                .collect(),
        ),
        _ => LoraValue::Null,
    }
}

fn length(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Path(p)) => LoraValue::Int(p.rels.len() as i64),
        _ => LoraValue::Null,
    }
}

fn first(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Path(p)) => p
            .nodes
            .first()
            .map(|id| LoraValue::Node(*id))
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn last(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Path(p)) => p
            .nodes
            .last()
            .map(|id| LoraValue::Node(*id))
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}
