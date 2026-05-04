//! Error helpers and stable error-code prefixes for the Node bindings.
//!
//! Every `NapiError` that crosses the JS boundary carries a `LORA_*:`
//! prefix in its message, drawn from
//! `lora_database::LoraErrorCode::as_str`. The JS wrapper splits on the
//! first colon to recover the precise code; existing class-routing
//! based on `LORA_ERROR` / `INVALID_PARAMS` continues to work via the
//! deprecated [`LORA_ERROR_CODE`] / [`INVALID_PARAMS_CODE`] aliases for
//! call sites that don't yet have a typed error to inspect.

use lora_database::{LoraError, LoraErrorCode};

/// Deprecated umbrella code retained so binding-level call sites that
/// only know they failed (e.g. `database is closed`) keep emitting a
/// recognisable prefix. New code paths should derive the prefix from
/// `LoraError::code().as_str()` via [`format_lora_error`].
pub(crate) const LORA_ERROR_CODE: &str = "LORA_INTERNAL";
/// Wire string for binding-level parameter validation failures.
pub(crate) const INVALID_PARAMS_CODE: &str = "LORA_INVALID_PARAMS";

/// Format a [`LoraError`] as `<code>: <message>` where `<code>` is the
/// precise [`LoraErrorCode`] (e.g. `LORA_PARSE`, `LORA_TIMEOUT`,
/// `LORA_WAL_POISONED`, …) so the JS wrapper can route on it.
pub(crate) fn format_lora_error(err: &LoraError) -> String {
    format!("{}: {}", err.code().as_str(), err.message())
}

/// Format a binding-level error using a precomputed code (used for
/// invalid-params validation, vector type rejection, etc.).
#[allow(dead_code)]
pub(crate) fn format_coded_error(code: LoraErrorCode, message: &str) -> String {
    format!("{}: {}", code.as_str(), message)
}

pub(crate) fn closed_error_message() -> String {
    format!("{LORA_ERROR_CODE}: database is closed")
}
