//! Regression baseline for `SemanticError` `Display` output.
//!
//! Wording reaches users through `LoraError::message()`; this file
//! pins each variant so wording drift gets caught in CI before it
//! reaches a binding's user-visible message.

use lora_analyzer::SemanticError;

#[test]
fn unknown_variable() {
    let err = SemanticError::UnknownVariable("n".into());
    assert_eq!(err.to_string(), "unknown variable `n`");
}

#[test]
fn duplicate_variable() {
    let err = SemanticError::DuplicateVariable("x".into());
    assert_eq!(err.to_string(), "duplicate variable `x` in the same scope");
}

#[test]
fn unknown_label() {
    let err = SemanticError::UnknownLabel("Person".into());
    assert_eq!(err.to_string(), "unknown label `:Person`");
}

#[test]
fn unknown_relationship_type() {
    let err = SemanticError::UnknownRelationshipType("KNOWS".into());
    assert_eq!(err.to_string(), "unknown relationship type `:KNOWS`");
}

#[test]
fn unknown_property() {
    let err = SemanticError::UnknownProperty("name".into());
    assert_eq!(err.to_string(), "unknown property `name`");
}

#[test]
fn unknown_property_at() {
    let err = SemanticError::UnknownPropertyAt("name".into(), 4, 8);
    assert_eq!(err.to_string(), "unknown property `name` at 4..8");
}

#[test]
fn type_mismatch() {
    assert_eq!(
        SemanticError::TypeMismatch.to_string(),
        "type mismatch in expression"
    );
}

#[test]
fn invalid_aggregation() {
    assert_eq!(
        SemanticError::InvalidAggregation.to_string(),
        "invalid aggregation usage"
    );
}

#[test]
fn duplicate_map_key() {
    let err = SemanticError::DuplicateMapKey("k".into());
    assert_eq!(err.to_string(), "duplicate map key `k`");
}

#[test]
fn duplicate_projection_alias() {
    let err = SemanticError::DuplicateProjectionAlias("a".into());
    assert_eq!(err.to_string(), "duplicate projection alias `a`");
}

#[test]
fn expected_property_map() {
    let err = SemanticError::ExpectedPropertyMap(3, 7);
    assert_eq!(
        err.to_string(),
        "expected a property map but found another expression at 3..7"
    );
}

#[test]
fn invalid_range() {
    let err = SemanticError::InvalidRange(5, 2, 10, 20);
    assert_eq!(
        err.to_string(),
        "invalid relationship length range 5..2 at 10..20"
    );
}

#[test]
fn unknown_function() {
    let err = SemanticError::UnknownFunction("foo".into(), 1, 4);
    assert_eq!(err.to_string(), "unknown function `foo` at 1..4");
}

#[test]
fn wrong_arity() {
    let err = SemanticError::WrongArity("size".into(), "1".into(), 2);
    assert_eq!(
        err.to_string(),
        "wrong number of arguments for `size`: expected 1, got 2"
    );
}

#[test]
fn aggregation_in_where() {
    assert_eq!(
        SemanticError::AggregationInWhere.to_string(),
        "aggregation functions are not allowed in WHERE clause"
    );
}

#[test]
fn union_column_count_mismatch() {
    let err = SemanticError::UnionColumnCountMismatch(3, 4);
    assert_eq!(
        err.to_string(),
        "all UNION branches must return the same number of columns: expected 3, got 4"
    );
}

#[test]
fn union_column_name_mismatch() {
    let err = SemanticError::UnionColumnNameMismatch("a".into(), "b".into());
    assert_eq!(
        err.to_string(),
        "all UNION branches must return the same column names: expected `a`, got `b`"
    );
}

#[test]
fn unsupported_feature() {
    let err = SemanticError::UnsupportedFeature("recursive CTE".into());
    assert_eq!(err.to_string(), "unsupported feature: recursive CTE");
}
