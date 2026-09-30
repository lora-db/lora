//! Fail fast instead of trapping when a write meets an open write stream.
//!
//! WebAssembly runs every call on one thread. The engine serializes writers
//! with a `std::sync::Mutex`, and on `wasm32-unknown-unknown` (no atomics)
//! that mutex cannot wait: locking it while it is held panics with "cannot
//! recursively acquire mutex", which aborts into `RuntimeError: unreachable`.
//!
//! Most calls take and release the writer lock inside one synchronous call,
//! so they never meet each other. The exception is a mutating stream
//! (`openStream` / `openExport` of a write query): it runs in a hidden
//! transaction that holds the writer lock until the stream is read to the
//! end, fails, or is closed, i.e. across JS turns. A write started while
//! such a stream is open (auto-commit, `transaction()`, another write
//! stream, an import, `clear()`, `loadSnapshot()`) would lock the mutex a
//! second time on the same thread, and there is no other thread that could
//! ever release it.
//!
//! [`WriterState`] records whether a write stream is open, so those calls
//! fail with `LORA_TRANSACTION` instead. Reads never take the writer lock
//! and keep working while a write stream is open.

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::JsError;

use crate::json::js_error;

const WRITE_STREAM_OPEN: &str = "a write stream is still open on this database: \
     read it to the end or close() it before starting another write";

/// Whether a write stream currently holds the database's writer lock.
/// Shared by one `WasmDatabase` and the cursors it opens.
#[derive(Clone, Default)]
pub(crate) struct WriterState(Rc<Cell<bool>>);

impl WriterState {
    pub(crate) fn held(&self) -> bool {
        self.0.get()
    }

    /// Error if a write stream holds the writer lock.
    pub(crate) fn ensure_free(&self) -> Result<(), JsError> {
        if self.held() {
            Err(js_error("LORA_TRANSACTION", WRITE_STREAM_OPEN))
        } else {
            Ok(())
        }
    }

    /// Mark the writer lock as held by a write stream until the returned
    /// claim is dropped. Drop the claim only after the stream itself.
    pub(crate) fn claim(&self) -> WriterClaim {
        self.0.set(true);
        WriterClaim(self.0.clone())
    }
}

/// Held by a write stream next to its native cursor.
pub(crate) struct WriterClaim(Rc<Cell<bool>>);

impl Drop for WriterClaim {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
