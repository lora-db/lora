//! Exception type registrations for the Python bindings.
//!
//! `pyo3::create_exception!` generates the per-class machinery
//! (`new_err`, `type_object`, `From<Self> for PyErr`, etc.). The types
//! are registered with the module by `lib.rs::_native` at import time so
//! Python callers see `lora_python.LoraError` and friends.
//!
//! Engine errors are surfaced with the message body prefixed by the
//! precise wire code from [`lora_database::LoraErrorCode`] (e.g.
//! `"LORA_PARSE: parse error at 0..5: ..."`). Callers who care about
//! routing past the exception class can split on the first colon to
//! recover the code.

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::PyErr;

use lora_database::{LoraError as EngineLoraError, LoraErrorCode};

create_exception!(
    lora_python,
    LoraError,
    PyException,
    "Base class for Lora engine errors. Exception messages start with a stable `LORA_*` code."
);
create_exception!(
    lora_python,
    LoraQueryError,
    LoraError,
    "Parse / analyze / execute / IO failure. Exception message starts with a precise `LORA_*` code."
);
create_exception!(
    lora_python,
    InvalidParamsError,
    LoraError,
    "A parameter value could not be mapped to a Lora value. Message starts with `LORA_INVALID_PARAMS:`."
);

/// Build a [`LoraQueryError`] from an `anyhow::Error`, prefixing the
/// message with the precise wire code so the Python caller can route
/// past the exception class.
pub(crate) fn lora_query_err_from_anyhow(err: anyhow::Error) -> PyErr {
    let lora = EngineLoraError::from_anyhow(err);
    LoraQueryError::new_err(format!("{}: {}", lora.code().as_str(), lora.message()))
}

/// Borrowed-by-reference variant of [`lora_query_err_from_anyhow`] for
/// call sites that don't have an owned `anyhow::Error`.
#[allow(dead_code)]
pub(crate) fn lora_query_err_from_anyhow_ref(err: &anyhow::Error) -> PyErr {
    let lora = EngineLoraError::from_anyhow_ref(err);
    LoraQueryError::new_err(format!("{}: {}", lora.code().as_str(), lora.message()))
}

/// Build a [`LoraQueryError`] for a binding-side message that doesn't
/// carry a downcastable error chain (e.g. `database is closed`,
/// `database lock poisoned`). Tagged with [`LoraErrorCode::Internal`].
#[allow(dead_code)]
pub(crate) fn lora_query_err_internal(message: &str) -> PyErr {
    LoraQueryError::new_err(format!("{}: {message}", LoraErrorCode::Internal.as_str()))
}

/// Build an [`InvalidParamsError`] tagged with `LORA_INVALID_PARAMS`.
#[allow(dead_code)]
pub(crate) fn invalid_params_err(message: impl AsRef<str>) -> PyErr {
    InvalidParamsError::new_err(format!(
        "{}: {}",
        LoraErrorCode::InvalidParams.as_str(),
        message.as_ref()
    ))
}
