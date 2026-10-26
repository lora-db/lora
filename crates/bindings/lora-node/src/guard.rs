//! Panic containment for everything that runs inside an N-API callback.
//!
//! napi-rs calls into this crate from `extern "C"` functions: the JS
//! method trampolines, the libuv pool's `execute` and `complete` hooks of
//! an `AsyncTask`, and the threadsafe-function callback that settles a
//! deferred. A panic unwinding out of any of them aborts the whole Node
//! process. So every entry point runs its body under [`guard`], and every
//! pool task is wrapped in [`Guarded`]: a panic becomes a `LORA_INTERNAL`
//! error on that one call instead.
//!
//! After a caught panic the binding's own mutexes may be poisoned. They
//! only guard registries (open streams, transactions, feeds, cancel
//! tokens) whose maps stay consistent across a panic, so [`lock`] recovers
//! them rather than failing every later call. The engine already recovers
//! its writer lock the same way, and a panicking write never published its
//! staged graph.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard};

use napi::bindgen_prelude::Result;
use napi::{Env, Error as NapiError, Status, Task};

use crate::errors::LORA_ERROR_CODE;

/// The message of a caught panic.
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// The error a caught panic is reported as.
pub(crate) fn panic_error(payload: &(dyn Any + Send)) -> NapiError {
    NapiError::new(
        Status::GenericFailure,
        format!(
            "{LORA_ERROR_CODE}: internal error (panic in the native binding): {}",
            panic_message(payload)
        ),
    )
}

/// Run `f`, turning a panic into a `LORA_INTERNAL` error.
pub(crate) fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| Err(panic_error(&*payload)))
}

/// Lock a binding mutex, recovering it if a caught panic poisoned it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A pool task whose `compute` (on a libuv worker) and `resolve` (on the
/// JS thread) cannot panic out of napi's `extern "C"` hooks.
pub struct Guarded<T>(pub(crate) T);

impl<T: Task> Task for Guarded<T> {
    type Output = T::Output;
    type JsValue = T::JsValue;

    fn compute(&mut self) -> Result<Self::Output> {
        guard(|| self.0.compute())
    }

    fn resolve(&mut self, env: Env, output: Self::Output) -> Result<Self::JsValue> {
        guard(|| self.0.resolve(env, output))
    }

    fn reject(&mut self, env: Env, err: NapiError) -> Result<Self::JsValue> {
        guard(|| self.0.reject(env, err))
    }

    fn finally(&mut self, env: Env) -> Result<()> {
        guard(|| self.0.finally(env))
    }
}

/// Wrap the closure that settles a deferred promise on the JS thread (from
/// napi's threadsafe-function callback) so it cannot panic out of it.
pub(crate) fn settle<T>(f: impl FnOnce(Env) -> Result<T> + Send + 'static) -> crate::Resolver<T> {
    Box::new(move |env| guard(|| f(env)))
}
