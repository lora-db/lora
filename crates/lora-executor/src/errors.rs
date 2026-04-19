use crate::value::LoraValue;
use lora_compiler::physical::PhysicalNodeId;
use lora_store::{NodeId, RelationshipId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("this query modifies the graph, but it was executed in read-only mode (CREATE at plan node {node_id:?})")]
    ReadOnlyCreate { node_id: PhysicalNodeId },

    #[error("this query modifies the graph, but it was executed in read-only mode (MERGE at plan node {node_id:?})")]
    ReadOnlyMerge { node_id: PhysicalNodeId },

    #[error("this query modifies the graph, but it was executed in read-only mode (DELETE at plan node {node_id:?})")]
    ReadOnlyDelete { node_id: PhysicalNodeId },

    #[error("this query modifies the graph, but it was executed in read-only mode (SET at plan node {node_id:?})")]
    ReadOnlySet { node_id: PhysicalNodeId },

    #[error("this query modifies the graph, but it was executed in read-only mode (REMOVE at plan node {node_id:?})")]
    ReadOnlyRemove { node_id: PhysicalNodeId },

    #[error("expected variable {var} to contain a node before expanding a relationship, but found {found}")]
    ExpectedNodeForExpand { var: String, found: String },

    #[error("expected a map value for properties, but found {found}")]
    ExpectedPropertyMap { found: String },

    #[error("grouping could not be executed because the group-by expression was not lowered to a variable")]
    GroupByNotLowered,

    #[error("aggregation could not be executed because the aggregate expression was not lowered to a variable")]
    AggregateNotLowered,

    #[error("variable-length relationships are not supported in CREATE")]
    UnsupportedCreateRelationshipRange,

    #[error("a CREATE relationship is missing its relationship type")]
    MissingRelationshipType,

    #[error("failed to create relationship `{rel_type}` from node {src} to node {dst}")]
    RelationshipCreateFailed {
        src: u64,
        dst: u64,
        rel_type: String,
    },

    #[error("cannot delete node {node_id} because it still has relationships; use DETACH DELETE to remove the node and its relationships")]
    DeleteNodeWithRelationships { node_id: NodeId },

    #[error("failed to delete relationship {rel_id}")]
    DeleteRelationshipFailed { rel_id: RelationshipId },

    #[error(
        "DELETE can only be used with nodes, relationships, or lists of them, but found {found}"
    )]
    InvalidDeleteTarget { found: String },

    #[error("REMOVE label can only be applied to a node, but found {found}")]
    ExpectedNodeForRemoveLabels { found: String },

    #[error("REMOVE referenced variable {var}, but that variable is not bound in the current row")]
    UnboundVariableForRemove { var: String },

    #[error("SET label can only be applied to a node, but found {found}")]
    ExpectedNodeForSetLabels { found: String },

    #[error("SET referenced variable {var}, but that variable is not bound in the current row")]
    UnboundVariableForSet { var: String },

    #[error("SET target must be a node or relationship, but found {found}")]
    InvalidSetTarget { found: String },

    #[error("REMOVE property currently only supports direct property expressions like n.name or r.weight")]
    UnsupportedRemoveTarget,

    #[error("REMOVE target must be a node or relationship property, but found {found}")]
    InvalidRemoveTarget { found: String },

    #[error(
        "SET property currently only supports direct property expressions like n.name or r.weight"
    )]
    UnsupportedSetTarget,

    #[error("expected variable {var} to contain a relationship during EXPAND, but found {found}")]
    ExpectedRelationshipForExpand { var: String, found: String },

    #[error("{0}")]
    RuntimeError(String),
}

pub type ExecResult<T> = Result<T, ExecutorError>;

pub fn value_kind(value: &LoraValue) -> String {
    match value {
        LoraValue::Null => "null".into(),
        LoraValue::Bool(_) => "bool".into(),
        LoraValue::Int(_) => "int".into(),
        LoraValue::Float(_) => "float".into(),
        LoraValue::String(_) => "string".into(),
        LoraValue::List(_) => "list".into(),
        LoraValue::Map(_) => "map".into(),
        LoraValue::Node(_) => "node".into(),
        LoraValue::Relationship(_) => "relationship".into(),
        LoraValue::Path(_) => "path".into(),
        LoraValue::Date(_) => "date".into(),
        LoraValue::DateTime(_) => "datetime".into(),
        LoraValue::LocalDateTime(_) => "localdatetime".into(),
        LoraValue::Time(_) => "time".into(),
        LoraValue::LocalTime(_) => "localtime".into(),
        LoraValue::Duration(_) => "duration".into(),
        LoraValue::Point(_) => "point".into(),
    }
}
