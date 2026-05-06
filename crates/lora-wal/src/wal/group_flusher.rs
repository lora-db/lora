//! Group-mode background flusher.
//!
//! Owns the OS thread that periodically `fsync`s the WAL under
//! `SyncMode::Group`. Held inside the [`Wal`] itself so dropping the
//! last `Arc<Wal>` runs the handle's `Drop`, signals shutdown, and
//! joins before the underlying state is destroyed.
//!
//! Not compiled on `wasm32-unknown-unknown`: the target has no real
//! filesystem durability boundary and `std::thread::spawn` is not
//! available there. Group mode in wasm falls back to the cooperative
//! drop-time flush in [`Wal::drop`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::wal::{FlushKind, Wal};

pub(super) struct GroupFlusherHandle {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for GroupFlusherHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            // `let _ = ...` because the thread can only fail by
            // panicking; even then, the Wal itself is being dropped
            // and there is nothing useful to do with the panic at
            // teardown.
            let _ = h.join();
        }
    }
}

pub(super) fn spawn_group_flusher(weak: Weak<Wal>, interval: Duration) -> GroupFlusherHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let handle = thread::spawn(move || {
        // Sleep first so a shortlived Wal that opens-and-closes
        // immediately doesn't pay for an extra wakeup. We re-check
        // the shutdown flag at every iteration so a Drop signal
        // racing with a sleep wakes up at most one interval late.
        while !shutdown_clone.load(Ordering::Acquire) {
            // Break the sleep into ~50 ms slices so shutdown can be
            // observed without waiting up to a full `interval` at
            // teardown. This matters for tests, which want fast
            // join times.
            let slice = Duration::from_millis(50).min(interval);
            let mut elapsed = Duration::ZERO;
            while elapsed < interval && !shutdown_clone.load(Ordering::Acquire) {
                thread::sleep(slice);
                elapsed += slice;
            }
            if shutdown_clone.load(Ordering::Acquire) {
                break;
            }
            match weak.upgrade() {
                Some(wal) => {
                    // Latch any fsync failure into `bg_failure` and
                    // stop the flusher. Subsequent commits / flushes
                    // see the latch via `check_healthy` and start
                    // returning `WalError::Poisoned`, which
                    // `WalRecorder` propagates to the host as a
                    // durability error. Operators recover by
                    // restarting from the last consistent
                    // snapshot + WAL.
                    if let Err(err) = wal.flush_inner(FlushKind::ForceFsync) {
                        let mut slot = wal.bg_failure_slot().lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(format!("bg fsync failed: {err}"));
                        }
                        break;
                    }
                }
                None => break,
            }
        }
    });
    GroupFlusherHandle {
        shutdown,
        handle: Some(handle),
    }
}
