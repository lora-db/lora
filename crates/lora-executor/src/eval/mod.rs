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
//!   (`cmp_numeric_or_string`), and the arithmetic value combinators
//!   (`add_values`, `sub_values`, `mul_values`, `div_values`,
//!   `mod_values`, `pow_values`, `substring_by_chars`).
//! - `functions` — `eval_function` dispatcher over the built-in
//!   function namespace: scalar / list / string / math / temporal /
//!   spatial / vector functions.
//! - `point` — the `point()` map decoder and the named-timezone
//!   offset table used by `datetime({ timezone: "..." })`.
//! - `vector` — the `vector()` constructor and the
//!   `vector.similarity.*` / `vector_distance` / `vector_norm`
//!   helpers, including the list-and-vector coercions they share.
//! - `errors` — the thread-local "current eval error" slot exposed
//!   via `set_eval_error`, [`clear_eval_error`], and `take_eval_error`.
//!   Used to thread fallible diagnostics out of the otherwise
//!   infallible `eval_expr` return type.

mod binops;
mod errors;
mod expr;
mod functions;
mod point;
mod vector;

pub use errors::clear_eval_error;
pub use expr::{eval_expr, eval_expr_result, eval_truthy_result, EvalContext};
