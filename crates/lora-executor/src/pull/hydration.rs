use lora_store::GraphStorage;

use crate::errors::ExecResult;
use crate::executor::{hydrate_node_record, hydrate_relationship_record};
use crate::value::{LoraValue, Row};

use super::RowSource;

/// Top-of-pipeline hydration. Replaces node / relationship id
/// references in each emitted row with their full hydrated map form,
/// matching the buffered executor's post-execution hydration step.
pub(crate) struct HydratingSource<'a, S: GraphStorage> {
    upstream: Box<dyn RowSource + 'a>,
    storage: &'a S,
}

impl<'a, S: GraphStorage> HydratingSource<'a, S> {
    pub(crate) fn new(upstream: Box<dyn RowSource + 'a>, storage: &'a S) -> Self {
        Self { upstream, storage }
    }
}

impl<'a, S: GraphStorage> RowSource for HydratingSource<'a, S> {
    fn next_row(&mut self) -> ExecResult<Option<Row>> {
        match self.upstream.next_row()? {
            None => Ok(None),
            Some(row) => Ok(Some(hydrate_row(row, self.storage))),
        }
    }
}

/// Replace node / relationship id references in `row` with their full
/// hydrated form (labels, type, properties), as every query result does.
pub fn hydrate_row<S: GraphStorage>(row: Row, storage: &S) -> Row {
    if !row_holds_entity(&row) {
        return row;
    }
    let mut out = Row::new();
    for (var, name, value) in row.into_iter_named() {
        out.insert_named_inline(var, name, hydrate_value(value, storage));
    }
    out
}

/// Whether hydration would change `row`: it holds a node or relationship
/// reference, possibly nested in a list or map. A plan that may return
/// entities still returns many rows that hold none (scalar columns, a
/// null from OPTIONAL MATCH); those pass through untouched.
pub(crate) fn row_holds_entity(row: &Row) -> bool {
    fn holds(value: &LoraValue) -> bool {
        match value {
            LoraValue::Node(_) | LoraValue::Relationship(_) => true,
            LoraValue::List(values) => values.iter().any(holds),
            LoraValue::Map(map) => map.values().any(holds),
            _ => false,
        }
    }
    row.iter().any(|(_, value)| holds(value))
}

pub(crate) fn hydrate_value<S: GraphStorage>(value: LoraValue, storage: &S) -> LoraValue {
    match value {
        LoraValue::Node(id) => storage
            .with_node(id, |n| hydrate_node_record(n))
            .unwrap_or(LoraValue::Null),
        LoraValue::Relationship(id) => storage
            .with_relationship(id, |r| hydrate_relationship_record(r))
            .unwrap_or(LoraValue::Null),
        LoraValue::List(values) => LoraValue::List(
            values
                .into_iter()
                .map(|v| hydrate_value(v, storage))
                .collect(),
        ),
        LoraValue::Map(map) => LoraValue::Map(
            map.into_iter()
                .map(|(k, v)| (k, hydrate_value(v, storage)))
                .collect(),
        ),
        other => other,
    }
}
