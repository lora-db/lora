use thiserror::Error;

#[derive(Debug, Error)]
pub enum SemanticError {
    #[error("unknown variable `{0}`")]
    UnknownVariable(String),

    #[error("duplicate variable `{0}` in the same scope")]
    DuplicateVariable(String),

    #[error("unknown property `{0}` at {1}..{2}")]
    UnknownPropertyAt(String, usize, usize),

    #[error("type mismatch in expression")]
    TypeMismatch,

    #[error("invalid aggregation usage")]
    InvalidAggregation,

    #[error("duplicate map key `{0}`")]
    DuplicateMapKey(String),

    #[error("duplicate projection alias `{0}`")]
    DuplicateProjectionAlias(String),

    #[error("expected a property map but found another expression at {0}..{1}")]
    ExpectedPropertyMap(usize, usize),

    #[error("invalid relationship length range {0}..{1} at {2}..{3}")]
    InvalidRange(u64, u64, usize, usize),

    #[error("unknown function `{0}` at {1}..{2}")]
    UnknownFunction(String, usize, usize),

    #[error("wrong number of arguments for `{0}`: expected {1}, got {2}")]
    WrongArity(String, String, usize),

    #[error("aggregation functions are not allowed in WHERE clause")]
    AggregationInWhere,

    #[error("`{0}` aggregates, so outside its aggregate functions it can only use grouping keys; project that value as its own column")]
    ImplicitGroupingKey(String),

    #[error("after DISTINCT, ORDER BY can only use the projected columns; project that value as its own column")]
    OrderByAfterDistinct,

    #[error("ORDER BY can't aggregate here: {0}")]
    AggregationInOrderBy(String),

    #[error("all UNION branches must return the same number of columns: expected {0}, got {1}")]
    UnionColumnCountMismatch(usize, usize),

    #[error("all UNION branches must return the same column names: expected `{0}`, got `{1}`")]
    UnionColumnNameMismatch(String, String),

    #[error("unsupported feature: {0}")]
    UnsupportedFeature(String),
}
