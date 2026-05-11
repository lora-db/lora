//! GroupSync background flusher.
//!
//! Owns the OS thread that periodically `fsync`s the WAL under
//! `SyncMode::GroupSync`. Held inside the [`Wal`] itself so dropping the
//! last `Arc<Wal>` runs the handle's `Drop`, signals shutdown, and
//! joins before the underlying state is destroyed.
//!
//! Not compiled on `wasm32-unknown-unknown`: the target has no real
//! filesystem durability boundary and `std::thread::spawn` is not
//! available there. GroupSync mode in wasm falls back to the cooperative
//! drop-time flush in [`Wal::drop`].

use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::wal::{FlushKind, Wal};

pub(super) struct GroupFlusherHandle {
    shutdown: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for GroupFlusherHandle {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shutdown;
        if let Ok(mut shutdown) = lock.lock() {
            *shutdown = true;
            cv.notify_one();
        }
        if let Some(h) = self.handle.take() {
            if h.thread().id() == thread::current().id() {
                return;
            }
            // `let _ = ...` because the thread can only fail by
            // panicking; even then, the Wal itself is being dropped
            // and there is nothing useful to do with the panic at
            // teardown.
            let _ = h.join();
        }
    }
}

pub(super) fn spawn_group_flusher(weak: Weak<Wal>, interval: Duration) -> GroupFlusherHandle {
    let shutdown = Arc::new((Mutex::new(false), Condvar::new()));
    let shutdown_clone = Arc::clone(&shutdown);
    let handle = thread::spawn(move || {
        // Sleep first so a short-lived Wal that opens-and-closes
        // immediately doesn't pay for an extra wakeup. The condvar
        // lets Drop wake the thread immediately instead of waiting
        // up to a full interval at teardown.
        loop {
            let (lock, cv) = &*shutdown_clone;
            let shutdown = match lock.lock() {
                Ok(guard) => guard,
                Err(_) => break,
            };
            let (shutdown, _) = match cv.wait_timeout_while(shutdown, interval, |s| !*s) {
                Ok(pair) => pair,
                Err(_) => break,
            };
            if *shutdown {
                break;
            }
            drop(shutdown);

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
                        let mut slot = wal
                            .bg_failure_slot()
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
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
