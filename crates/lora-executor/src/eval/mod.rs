//! Expression-level evaluation against a `Row` and a borrowed
//! [`EvalContext`].
//!
//! Layout:
//! - `expr` — the [`EvalContext`] struct, the [`eval_expr`] dispatcher
//!   over [`ResolvedExpr`], the result-form entry points
//!   ([`eval_expr_result`], [`eval_truthy_result`]), the literal / property
//!   lookup helpers, and the EXISTS / pattern-comprehension pattern matchers.
//! - `binops` — unary and binary operator evaluation: `eval_unary`,
//!   `eval_binary`, structural equality (`value_eq`), comparison
//!   (`compare_values`), and the arithmetic value combinators
//!   (`add_values`, `sub_values`, `mul_values`, `div_values`,
//!   `mod_values`, `pow_values`, `substring_by_chars`).
//! - `functions` — `eval_function` dispatcher over the built-in
//!   function namespace: scalar / list / string / math / temporal /
//!   spatial / vector functions.
//! - `point` — the `point()` map decoder and the named-timezone
//!   offset table used by `datetime({ timezone: "..." })`.
//! - `regex` — cached full-match regex evaluation for the `=~`
//!   predicate operator.
//! - `vector` — the `vector()` constructor and the
//!   `vector.similarity.*` / `vector_distance` / `vector_norm`
//!   helpers, including the list-and-vector coercions they share.
//! - `fold` — [`fold_call`], which evaluates a builtin call on constant
//!   arguments when the plan is built, for the calls whose value cannot
//!   change between rows or runs.
//! - `errors` — the thread-local "current eval error" slot exposed
//!   via `set_eval_error`, [`clear_eval_error`], and `take_eval_error`.
//!   Used to thread fallible diagnostics out of the otherwise
//!   infallible `eval_expr` return type.

mod binops;
mod builtins;
mod errors;
mod expr;
mod fold;
mod functions;
mod point;
mod regex;
mod vector;

pub(crate) use binops::compare_values;
pub use builtins::FulltextPage;
pub use errors::clear_eval_error;
pub(crate) use errors::take_eval_error;
pub use expr::{
    eval_expr, eval_expr_result, eval_projected_result, eval_truthy_result, passes_through,
    EvalContext, Projected, DEADLINE_EXCEEDED,
};
pub(crate) use expr::{eval_ref, eval_status};
pub use fold::fold_call;
