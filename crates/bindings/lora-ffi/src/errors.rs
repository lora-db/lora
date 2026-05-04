//! Status codes and error formatting for the C ABI.
//!
//! Every exported function returns a [`LoraStatus`] discriminant and
//! writes a heap-allocated error string into its `out_error` parameter
//! on the failure paths. The message is always prefixed with one of the
//! stable `LORA_*` codes from [`lora_database::LoraErrorCode`] so
//! out-of-tree consumers (`crates/bindings/lora-go`, third-party
//! bindings) can map engine errors to their own typed exceptions
//! without parsing free-form text.
//!
//! Two prefix sources cooperate:
//!
//! - Engine errors: the prefix is taken from
//!   `LoraError::from_anyhow(e).code().as_str()` so it carries the
//!   precise code (`LORA_PARSE`, `LORA_TIMEOUT`, `LORA_WAL_POISONED`,
//!   …). Use [`write_lora_error`].
//! - Binding-level argument validation (invalid UTF-8, missing fields,
//!   bad JSON shape): the prefix is the constant
//!   [`INVALID_PARAMS_PREFIX`] = `LORA_INVALID_PARAMS`, set via
//!   [`write_error`].

use std::ffi::c_char;

use lora_database::{LoraError, LoraErrorCode};

use crate::to_c_string;

/// Status codes returned by every FFI entry point.
///
/// The numeric values are part of the stable ABI — do not reorder.
/// `LoraError` is a *category* discriminant; the precise per-code
/// classification (parse / timeout / wal-poisoned / …) is carried in
/// the message prefix produced by [`write_lora_error`].
#[repr(C)]
pub enum LoraStatus {
    /// The call succeeded. Any out-pointers are populated.
    Ok = 0,
    /// Parse / analyze / execute / IO / WAL / snapshot failure. The
    /// out-error string starts with the precise `LORA_*` code from
    /// [`LoraErrorCode`].
    LoraError = 1,
    /// A parameter value could not be mapped to a Lora value. The
    /// out-error string starts with `LORA_INVALID_PARAMS: `.
    InvalidParams = 2,
    /// A required pointer argument was null.
    NullPointer = 3,
    /// The provided UTF-8 input was invalid.
    InvalidUtf8 = 4,
    /// Rust panicked inside the FFI. The out-error captures the panic
    /// message when one could be recovered.
    Panic = 5,
}

/// Wire string used for binding-level parameter validation failures
/// (UTF-8, missing fields, malformed JSON). Engine-level invalid-params
/// failures use the same wire string via [`write_lora_error`] when the
/// underlying code is [`LoraErrorCode::InvalidParams`].
pub(crate) const INVALID_PARAMS_PREFIX: &str = "LORA_INVALID_PARAMS";

/// Wire string used for panic recovery — present so the message body
/// always starts with a recognisable `LORA_*` token even when the
/// engine never produced a typed error.
pub(crate) const PANIC_PREFIX: &str = "LORA_PANIC";

/// Write a `<prefix>: <message>` string into `*out_error`. No-ops when
/// the caller passed a null `out_error`. The allocation must be freed
/// with `lora_string_free`.
pub(crate) unsafe fn write_error(out_error: *mut *mut c_char, prefix: &str, message: &str) {
    if out_error.is_null() {
        return;
    }
    let full = format!("{prefix}: {message}");
    let ptr = to_c_string(full);
    *out_error = ptr;
}

/// Convert an `anyhow::Error` from the engine into a typed
/// [`LoraError`], then write `<code>: <message>` into `*out_error` so
/// the host binding can route on the wire string.
pub(crate) unsafe fn write_lora_error(out_error: *mut *mut c_char, err: anyhow::Error) {
    let lora = LoraError::from_anyhow(err);
    write_error(out_error, lora.code().as_str(), lora.message());
}

/// Write a `<code>: <message>` for a precomputed [`LoraErrorCode`]
/// (used when the call site already knows the category — e.g. binding
/// snapshot-options parsing reports [`LoraErrorCode::InvalidParams`]
/// directly).
pub(crate) unsafe fn write_coded_error(
    out_error: *mut *mut c_char,
    code: LoraErrorCode,
    message: &str,
) {
    write_error(out_error, code.as_str(), message);
}

/// Best-effort recovery of a panic payload's message. Used by the
/// `catch_unwind` harnesses so a Rust panic surfaces as a
/// [`LoraStatus::Panic`] with a captured message instead of unwinding
/// across the FFI boundary.
pub(crate) fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<String>() {
        format!("panic: {s}")
    } else if let Some(s) = panic.downcast_ref::<&'static str>() {
        format!("panic: {s}")
    } else {
        "panic: (unrecoverable message)".to_string()
    }
}
