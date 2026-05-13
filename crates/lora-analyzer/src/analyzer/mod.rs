//! Semantic analyzer: lower a parsed [`lora_ast::Document`] into a
//! [`crate::resolved::ResolvedQuery`] with variables resolved against scopes
//! and labels / relationship types / function arities validated against the
//! catalog.
//!
//! Layout:
//! - `state` — the [`Analyzer`] struct, its constructor, and the top-level
//!   `analyze` / `analyze_query` entry points plus query-shape lowerings
//!   (single-query / multi-part / single-part dispatch). Also hosts the
//!   small bookkeeping helpers (variable declaration, scope manipulation,
//!   label / relationship-type / property-key catalog checks) that every
//!   sibling reuses.
//! - `clauses` — clause-level analysis (MATCH, UNWIND, CALL, CREATE,
//!   MERGE, DELETE, SET, REMOVE, WITH, RETURN, projection body). Also
//!   owns `projection_name` and the internal `ExportedAlias` /
//!   `AnalyzedProjectionBody` carrier types.
//! - `patterns` — pattern analysis (pattern, pattern part, pattern
//!   element, node, relationship) plus `collect_node_var_labels` and
//!   `format_label_groups` for duplicate-variable detection.
//! - `expressions` — expression analysis (`analyze_expr`,
//!   `analyze_expr_with_aliases`), function-name / arity validation,
//!   builtin enum/type literal rewrites, and `expr_contains_aggregate`
//!   used by WHERE to reject aggregations.
//! - `tests` — analyzer unit tests.

mod builtin_signatures;
mod clauses;
mod expressions;
mod patterns;
mod state;

#[cfg(test)]
mod tests;

pub use builtin_signatures::{
    accepts_enum_literal, accepts_type_literal, builtin_spec, namespaced_arity, resolve_function,
    AggregateFunction, FunctionId, BUILTIN_SPECS,
};
pub use state::Analyzer;
