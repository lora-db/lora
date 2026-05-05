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
            Some(row) => {
                let mut out = Row::new();
                for (var, name, value) in row.into_iter_named() {
                    out.insert_named(var, name, hydrate_value(value, self.storage));
                }
                Ok(Some(out))
            }
        }
    }
}

pub(crate) fn hydrate_value<S: GraphStorage>(value: LoraValue, storage: &S) -> LoraValue {
    match value {
        LoraValue::Node(id) => storage
            .with_node(id, hydrate_node_record)
            .unwrap_or(LoraValue::Null),
        LoraValue::Relationship(id) => storage
            .with_relationship(id, hydrate_relationship_record)
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
