//! Lookup helpers for the `LoraRuby::Error` exception hierarchy.
//!
//! The exception classes themselves are registered in `lib.rs::init` so
//! Ruby owns their lifetime; these helpers re-find them by name when a
//! method needs to raise. `unwrap_or_else(|_| ruby.exception_standard_error())`
//! keeps us safe even if a constant is shadowed at runtime — we still
//! raise *something* descended from `StandardError`.

use lora_database::{LoraError, LoraErrorCode};
use magnus::{prelude::*, Error as MagnusError, ExceptionClass, RModule, Ruby};

pub(crate) fn lora_module(ruby: &Ruby) -> RModule {
    ruby.class_object()
        .const_get::<_, RModule>("LoraRuby")
        .expect("LoraRuby module is defined by `init` before any method runs")
}

pub(crate) fn lora_error_class(ruby: &Ruby, name: &str) -> ExceptionClass {
    // `const_get::<_, ExceptionClass>` converts the stored RClass into
    // an ExceptionClass — this is the sound path, because our subclasses
    // of StandardError retain the exception-class trait on the Ruby
    // side even though `define_class` typed them as RClass.
    lora_module(ruby)
        .const_get::<_, ExceptionClass>(name)
        .unwrap_or_else(|_| ruby.exception_standard_error())
}

/// Raise a `LoraRuby::QueryError`, prefixing the message with the
/// precise wire code from [`LoraErrorCode`] so Ruby callers can route
/// past the exception class via `e.message.split(': ', 2)`.
pub(crate) fn query_error(ruby: &Ruby, msg: impl Into<String>) -> MagnusError {
    let raw: String = msg.into();
    let body = if has_code_prefix(&raw) {
        raw
    } else {
        format!("{}: {raw}", LoraErrorCode::Internal.as_str())
    };
    MagnusError::new(lora_error_class(ruby, "QueryError"), body)
}

/// Raise a `LoraRuby::InvalidParamsError`, prefixed with
/// `LORA_INVALID_PARAMS:` so callers can route uniformly with the
/// other bindings.
pub(crate) fn invalid_params(ruby: &Ruby, msg: impl Into<String>) -> MagnusError {
    let raw: String = msg.into();
    let body = if has_code_prefix(&raw) {
        raw
    } else {
        format!("{}: {raw}", LoraErrorCode::InvalidParams.as_str())
    };
    MagnusError::new(lora_error_class(ruby, "InvalidParamsError"), body)
}

/// Build a `LoraRuby::QueryError` from any error convertible into
/// [`LoraError`]. Accepts both the engine's typed `LoraError` and the
/// binding-internal `anyhow::Error` (via `From<anyhow::Error>`), so
/// query and admin paths share one helper.
#[allow(dead_code)]
pub(crate) fn query_error_from_anyhow(ruby: &Ruby, err: impl Into<LoraError>) -> MagnusError {
    let lora = err.into();
    let body = format!("{}: {}", lora.code().as_str(), lora.message());
    MagnusError::new(lora_error_class(ruby, "QueryError"), body)
}

fn has_code_prefix(s: &str) -> bool {
    // Detect `LORA_<UPPER_SNAKE>:` so callers that already produced a
    // coded message (e.g. by passing through `query_error_from_anyhow`)
    // are not double-prefixed.
    let Some(colon) = s.find(':') else {
        return false;
    };
    let head = &s[..colon];
    head.starts_with("LORA_") && head.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
}
