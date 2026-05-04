//! Regression baseline for `ExecutorError` `Display` output.
//!
//! These messages flow out through `LoraError::message()` and end
//! up in user-visible exceptions on the binding side. Pinning each
//! variant catches wording drift before it changes user-visible
//! behaviour.

use lora_executor::ExecutorError;

#[test]
fn read_only_create() {
    let err = ExecutorError::ReadOnlyCreate { node_id: 7 };
    assert_eq!(
        err.to_string(),
        "this query modifies the graph, but it was executed in read-only mode (CREATE at plan node 7)"
    );
}

#[test]
fn read_only_merge() {
    let err = ExecutorError::ReadOnlyMerge { node_id: 1 };
    assert_eq!(
        err.to_string(),
        "this query modifies the graph, but it was executed in read-only mode (MERGE at plan node 1)"
    );
}

#[test]
fn read_only_delete() {
    let err = ExecutorError::ReadOnlyDelete { node_id: 2 };
    assert_eq!(
        err.to_string(),
        "this query modifies the graph, but it was executed in read-only mode (DELETE at plan node 2)"
    );
}

#[test]
fn read_only_set() {
    let err = ExecutorError::ReadOnlySet { node_id: 3 };
    assert_eq!(
        err.to_string(),
        "this query modifies the graph, but it was executed in read-only mode (SET at plan node 3)"
    );
}

#[test]
fn read_only_remove() {
    let err = ExecutorError::ReadOnlyRemove { node_id: 4 };
    assert_eq!(
        err.to_string(),
        "this query modifies the graph, but it was executed in read-only mode (REMOVE at plan node 4)"
    );
}

#[test]
fn expected_node_for_expand() {
    let err = ExecutorError::ExpectedNodeForExpand {
        var: "n".into(),
        found: "int".into(),
    };
    assert_eq!(
        err.to_string(),
        "expected variable n to contain a node before expanding a relationship, but found int"
    );
}

#[test]
fn expected_property_map() {
    let err = ExecutorError::ExpectedPropertyMap {
        found: "string".into(),
    };
    assert_eq!(
        err.to_string(),
        "expected a map value for properties, but found string"
    );
}

#[test]
fn group_by_not_lowered() {
    assert_eq!(
        ExecutorError::GroupByNotLowered.to_string(),
        "grouping could not be executed because the group-by expression was not lowered to a variable"
    );
}

#[test]
fn aggregate_not_lowered() {
    assert_eq!(
        ExecutorError::AggregateNotLowered.to_string(),
        "aggregation could not be executed because the aggregate expression was not lowered to a variable"
    );
}

#[test]
fn unsupported_create_relationship_range() {
    assert_eq!(
        ExecutorError::UnsupportedCreateRelationshipRange.to_string(),
        "variable-length relationships are not supported in CREATE"
    );
}

#[test]
fn missing_relationship_type() {
    assert_eq!(
        ExecutorError::MissingRelationshipType.to_string(),
        "a CREATE relationship is missing its relationship type"
    );
}

#[test]
fn relationship_create_failed() {
    let err = ExecutorError::RelationshipCreateFailed {
        src: 1,
        dst: 2,
        rel_type: "KNOWS".into(),
    };
    assert_eq!(
        err.to_string(),
        "failed to create relationship `KNOWS` from node 1 to node 2"
    );
}

#[test]
fn delete_node_with_relationships() {
    let err = ExecutorError::DeleteNodeWithRelationships { node_id: 42 };
    assert_eq!(
        err.to_string(),
        "cannot delete node 42 because it still has relationships; use DETACH DELETE to remove the node and its relationships"
    );
}

#[test]
fn delete_relationship_failed() {
    let err = ExecutorError::DeleteRelationshipFailed { rel_id: 9 };
    assert_eq!(err.to_string(), "failed to delete relationship 9");
}

#[test]
fn invalid_delete_target() {
    let err = ExecutorError::InvalidDeleteTarget {
        found: "int".into(),
    };
    assert_eq!(
        err.to_string(),
        "DELETE can only be used with nodes, relationships, or lists of them, but found int"
    );
}

#[test]
fn expected_node_for_remove_labels() {
    let err = ExecutorError::ExpectedNodeForRemoveLabels {
        found: "string".into(),
    };
    assert_eq!(
        err.to_string(),
        "REMOVE label can only be applied to a node, but found string"
    );
}

#[test]
fn unbound_variable_for_remove() {
    let err = ExecutorError::UnboundVariableForRemove { var: "n".into() };
    assert_eq!(
        err.to_string(),
        "REMOVE referenced variable n, but that variable is not bound in the current row"
    );
}

#[test]
fn expected_node_for_set_labels() {
    let err = ExecutorError::ExpectedNodeForSetLabels {
        found: "int".into(),
    };
    assert_eq!(
        err.to_string(),
        "SET label can only be applied to a node, but found int"
    );
}

#[test]
fn unbound_variable_for_set() {
    let err = ExecutorError::UnboundVariableForSet { var: "n".into() };
    assert_eq!(
        err.to_string(),
        "SET referenced variable n, but that variable is not bound in the current row"
    );
}

#[test]
fn invalid_set_target() {
    let err = ExecutorError::InvalidSetTarget {
        found: "string".into(),
    };
    assert_eq!(
        err.to_string(),
        "SET target must be a node or relationship, but found string"
    );
}

#[test]
fn unsupported_remove_target() {
    assert_eq!(
        ExecutorError::UnsupportedRemoveTarget.to_string(),
        "REMOVE property currently only supports direct property expressions like n.name or r.weight"
    );
}

#[test]
fn invalid_remove_target() {
    let err = ExecutorError::InvalidRemoveTarget {
        found: "list".into(),
    };
    assert_eq!(
        err.to_string(),
        "REMOVE target must be a node or relationship property, but found list"
    );
}

#[test]
fn unsupported_set_target() {
    assert_eq!(
        ExecutorError::UnsupportedSetTarget.to_string(),
        "SET property currently only supports direct property expressions like n.name or r.weight"
    );
}

#[test]
fn expected_relationship_for_expand() {
    let err = ExecutorError::ExpectedRelationshipForExpand {
        var: "r".into(),
        found: "node".into(),
    };
    assert_eq!(
        err.to_string(),
        "expected variable r to contain a relationship during EXPAND, but found node"
    );
}

#[test]
fn query_timeout() {
    assert_eq!(
        ExecutorError::QueryTimeout.to_string(),
        "query deadline exceeded"
    );
}

#[test]
fn runtime_error_passes_message_through() {
    let err = ExecutorError::RuntimeError("boom".into());
    assert_eq!(err.to_string(), "boom");
}
