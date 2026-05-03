//! Thread-local "current eval error" slot.
//!
//! `eval_expr` returns `LoraValue::Null` on internal failures rather than
//! a `Result`, so call sites that need to distinguish "expression
//! evaluated to null" from "expression failed" thread the error
//! through this slot. The pattern is:
//!
//! 1. [`clear_eval_error`] before evaluating.
//! 2. Call `eval_expr` (and let it call `set_eval_error(...)` if
//!    something fails — see the helpers in `binops` and `functions`).
//! 3. [`take_eval_error`] afterwards. If it returns `Some(msg)`, the
//!    expression failed and `msg` is the error.
//!
//! [`eval_expr_result`] in `expr.rs` wraps the three steps in a single
//! `Result`-returning call, which is what most call sites use.

thread_local! {
    static EVAL_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn set_eval_error(msg: String) {
    EVAL_ERROR.with(|e| *e.borrow_mut() = Some(msg));
}

pub fn clear_eval_error() {
    EVAL_ERROR.with(|e| *e.borrow_mut() = None);
}

pub(super) fn take_eval_error() -> Option<String> {
    EVAL_ERROR.with(|e| e.borrow_mut().take())
}
