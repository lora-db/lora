//! GroupSync background flusher.
//!
//! Owns the OS thread that periodically `fsync`s the WAL under
//! `SyncMode::GroupSync`. [`GroupFlusherHandle::request_fsync`] asks for
//! an `fsync` without waiting for the interval to end, for a caller waiting
//! for durability (a change feed). The flusher still runs at most one
//! `fsync` per interval: a request right after an `fsync` waits for the
//! rest of the interval, so a busy writer sees the same `fsync` cadence as
//! without requests, and only a commit after a quiet spell is made durable
//! at once. Back-to-back `fsync`s cut commit throughput several times over
//! in measurements, because the disk flush competes with the writes. Held inside the [`Wal`] itself so dropping the
//! last `Arc<Wal>` runs the handle's `Drop`, signals shutdown, and
//! joins before the underlying state is destroyed.
//!
//! Not compiled on `wasm32-unknown-unknown`: the target has no real
//! filesystem durability boundary and `std::thread::spawn` is not
//! available there. GroupSync mode in wasm falls back to the cooperative
//! drop-time flush in [`Wal::drop`].

use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::wal::{FlushKind, Wal};

#[derive(Default)]
struct Signal {
    shutdown: bool,
    /// An `fsync` was requested before the interval ends.
    requested: bool,
}

pub(super) struct GroupFlusherHandle {
    signal: Arc<(Mutex<Signal>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl GroupFlusherHandle {
    /// Ask for an `fsync` as soon as the interval allows: at once when the
    /// last one is at least an interval old, otherwise when the interval
    /// since it ends.
    pub(super) fn request_fsync(&self) {
        let (lock, cv) = &*self.signal;
        if let Ok(mut signal) = lock.lock() {
            if !signal.requested {
                signal.requested = true;
                cv.notify_one();
            }
        }
    }
}

impl Drop for GroupFlusherHandle {
    fn drop(&mut self) {
        let (lock, cv) = &*self.signal;
        if let Ok(mut signal) = lock.lock() {
            signal.shutdown = true;
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
    let signal = Arc::new((Mutex::new(Signal::default()), Condvar::new()));
    let signal_clone = Arc::clone(&signal);
    let handle = thread::spawn(move || {
        // Sleep first so a short-lived Wal that opens-and-closes
        // immediately doesn't pay for an extra wakeup. The condvar
        // lets Drop wake the thread immediately instead of waiting
        // up to a full interval at teardown, and `request_fsync` wake it
        // early.
        // When the last real `fsync` ran; idle ticks that skip it do not
        // count, so the first commit after a quiet spell is not held back.
        let mut last_fsync = Instant::now().checked_sub(interval);
        loop {
            let (lock, cv) = &*signal_clone;
            let signal = match lock.lock() {
                Ok(guard) => guard,
                Err(_) => break,
            };
            let (signal, _) =
                match cv.wait_timeout_while(signal, interval, |s| !s.shutdown && !s.requested) {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
            // A request still keeps one interval between `fsync`s.
            let rest =
                last_fsync.map_or(Duration::ZERO, |at| interval.saturating_sub(at.elapsed()));
            let (mut signal, _) = match cv.wait_timeout_while(signal, rest, |s| !s.shutdown) {
                Ok(pair) => pair,
                Err(_) => break,
            };
            if signal.shutdown {
                break;
            }
            signal.requested = false;
            drop(signal);

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
                    let fsyncs = wal.fsync_count();
                    let started = Instant::now();
                    let flushed = wal.flush_inner(FlushKind::ForceFsync);
                    if wal.fsync_count() != fsyncs {
                        last_fsync = Some(started);
                    }
                    if let Err(err) = flushed {
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
        signal,
        handle: Some(handle),
    }
}
