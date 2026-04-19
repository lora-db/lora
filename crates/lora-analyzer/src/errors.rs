use thiserror::Error;

#[derive(Debug, Error)]
pub enum SemanticError {
    #[error("Unknown variable `{0}`")]
    UnknownVariable(String),

    #[error("Duplicate variable `{0}` in the same scope")]
    DuplicateVariable(String),

    #[error("Unknown label `:{0}`")]
    UnknownLabel(String),

    #[error("Unknown relationship type `:{0}`")]
    UnknownRelationshipType(String),

    #[error("Unknown property `{0}`")]
    UnknownProperty(String),

    #[error("Unknown property `{0}` at {1}..{2}")]
    UnknownPropertyAt(String, usize, usize),

    #[error("Type mismatch in expression")]
    TypeMismatch,

    #[error("Invalid aggregation usage")]
    InvalidAggregation,

    #[error("Duplicate map key `{0}`")]
    DuplicateMapKey(String),

    #[error("Duplicate projection alias `{0}`")]
    DuplicateProjectionAlias(String),

    #[error("Expected a property map but found another expression at {0}..{1}")]
    ExpectedPropertyMap(usize, usize),

    #[error("Invalid relationship length range {0}..{1} at {2}..{3}")]
    InvalidRange(u64, u64, usize, usize),

    #[error("Unknown function `{0}` at {1}..{2}")]
    UnknownFunction(String, usize, usize),

    #[error("Wrong number of arguments for `{0}`: expected {1}, got {2}")]
    WrongArity(String, String, usize),

    #[error("Aggregation functions are not allowed in WHERE clause")]
    AggregationInWhere,

    #[error("All UNION branches must return the same number of columns: expected {0}, got {1}")]
    UnionColumnCountMismatch(usize, usize),

    #[error("All UNION branches must return the same column names: expected `{0}`, got `{1}`")]
    UnionColumnNameMismatch(String, String),

    #[error("Unsupported feature: {0}")]
    UnsupportedFeature(String),
}
