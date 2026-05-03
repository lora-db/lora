//! Exception type registrations for the Python bindings.
//!
//! `pyo3::create_exception!` generates the per-class machinery
//! (`new_err`, `type_object`, `From<Self> for PyErr`, etc.). The types
//! are registered with the module by `lib.rs::_native` at import time so
//! Python callers see `lora_python.LoraError` and friends.

use pyo3::create_exception;
use pyo3::exceptions::PyException;

create_exception!(
    lora_python,
    LoraError,
    PyException,
    "Base class for Lora engine errors."
);
create_exception!(
    lora_python,
    LoraQueryError,
    LoraError,
    "Parse / analyze / execute failure."
);
create_exception!(
    lora_python,
    InvalidParamsError,
    LoraError,
    "A parameter value could not be mapped to a Lora value."
);
