//! Lookup helpers for the `LoraRuby::Error` exception hierarchy.
//!
//! The exception classes themselves are registered in `lib.rs::init` so
//! Ruby owns their lifetime; these helpers re-find them by name when a
//! method needs to raise. `unwrap_or_else(|_| ruby.exception_standard_error())`
//! keeps us safe even if a constant is shadowed at runtime — we still
//! raise *something* descended from `StandardError`.

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

pub(crate) fn query_error(ruby: &Ruby, msg: impl Into<String>) -> MagnusError {
    MagnusError::new(lora_error_class(ruby, "QueryError"), msg.into())
}

pub(crate) fn invalid_params(ruby: &Ruby, msg: impl Into<String>) -> MagnusError {
    MagnusError::new(lora_error_class(ruby, "InvalidParamsError"), msg.into())
}
