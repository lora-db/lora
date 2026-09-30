//! Releasing the GIL around engine calls.

use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::marker::Ungil;
use pyo3::Python;

/// Engine calls running with the GIL released right now, process-wide.
static RELEASED: AtomicUsize = AtomicUsize::new(0);

/// `py.allow_threads(f)`, counted in [`RELEASED`] while it runs. Every
/// engine call that releases the GIL goes through here.
pub(crate) fn without_gil<T, F>(py: Python<'_>, f: F) -> T
where
    F: Ungil + FnOnce() -> T,
    T: Ungil,
{
    let _released = Released::enter();
    py.allow_threads(f)
}

/// Whether the writer lock of an in-memory database is free and stays free
/// until the caller takes it. Call it holding the GIL: then no other Python
/// thread runs, so the lock can only be held or about to be taken by a
/// thread inside a GIL-free engine call (counted in [`RELEASED`]) or by a
/// mutating stream's actor (counted by `lora_binding_buffer`). A persistent
/// database is not covered.
pub(crate) fn writer_lock_free() -> bool {
    RELEASED.load(Ordering::SeqCst) == 0 && !lora_binding_buffer::stream::write_streams_active()
}

struct Released;

impl Released {
    fn enter() -> Self {
        RELEASED.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for Released {
    fn drop(&mut self) {
        RELEASED.fetch_sub(1, Ordering::SeqCst);
    }
}
