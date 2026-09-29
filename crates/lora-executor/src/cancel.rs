//! Cooperative query cancellation.
//!
//! Queries already carry an optional deadline that the executor checks at
//! operator boundaries and inside hot loops. Cancellation rides on that
//! mechanism instead of threading a second token through every operator:
//! a cancellable query is given a *unique* deadline instant, and cancelling
//! it records that instant in a small process-wide set. The deadline check
//! consults the set, so a cancelled query stops at its next check point on
//! whichever thread is running it (including parallel workers), and then
//! unwinds through the normal timeout path: the error propagates, WAL
//! transactions abort, and locks are released.
//!
//! The set is only consulted while at least one cancellation is pending,
//! so queries that never use cancellation pay one relaxed atomic load.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use web_time::Instant;

/// How far out an "untimed" cancellable deadline sits. Far enough that
/// it never fires as a timeout, near enough that `Instant` arithmetic
/// cannot overflow on any platform.
const NO_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24 * 365);

static PENDING: AtomicUsize = AtomicUsize::new(0);
static SEQ: AtomicU64 = AtomicU64::new(0);

fn cancelled() -> &'static Mutex<HashSet<Instant>> {
    static SET: OnceLock<Mutex<HashSet<Instant>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A deadline that can also be cancelled before it expires.
///
/// Pass [`Self::deadline`] wherever a query deadline is accepted. Call
/// [`Self::cancel`] from any thread to stop the query at its next check
/// point; it then fails exactly as if its deadline had passed. Dropping
/// the handle forgets the cancellation.
#[derive(Debug)]
pub struct CancellableDeadline {
    deadline: Instant,
    registered: bool,
}

impl CancellableDeadline {
    /// A deadline `timeout` from now (or effectively never when `None`)
    /// that can be cancelled early.
    pub fn new(timeout: Option<Duration>) -> Self {
        let base = Instant::now()
            .checked_add(timeout.unwrap_or(NO_TIMEOUT))
            .unwrap_or_else(Instant::now);
        // Offset by a per-handle count of nanoseconds so two queries that
        // start in the same instant never share a deadline value.
        let nudge = Duration::from_nanos(SEQ.fetch_add(1, Ordering::Relaxed) % 1_000_000);
        Self {
            deadline: base.checked_add(nudge).unwrap_or(base),
            registered: false,
        }
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Request cancellation. Idempotent.
    pub fn cancel(&mut self) {
        if self.registered {
            return;
        }
        let mut set = cancelled().lock().unwrap_or_else(|p| p.into_inner());
        if set.insert(self.deadline) {
            PENDING.fetch_add(1, Ordering::Release);
        }
        self.registered = true;
    }

    pub fn is_cancelled(&self) -> bool {
        self.registered
    }
}

impl Drop for CancellableDeadline {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        let mut set = cancelled().lock().unwrap_or_else(|p| p.into_inner());
        if set.remove(&self.deadline) {
            PENDING.fetch_sub(1, Ordering::Release);
        }
    }
}

/// Whether the query owning `deadline` has been cancelled.
#[inline]
pub(crate) fn is_cancelled(deadline: Instant) -> bool {
    if PENDING.load(Ordering::Acquire) == 0 {
        return false;
    }
    cancelled()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(&deadline)
}

thread_local! {
    static ACTIVE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Makes a query's deadline visible to everything that runs on this
/// thread while the query executes, including pull pipelines that
/// operators build lazily mid-query (`OPTIONAL MATCH`, `CALL {}`) and
/// buffered sub-executors. Executors enter one at each public entry
/// point; nesting keeps the outer deadline when the inner has none, and
/// dropping the scope restores whatever was active before.
pub(crate) struct DeadlineScope {
    prev: Option<Instant>,
}

impl DeadlineScope {
    pub(crate) fn enter(deadline: Option<Instant>) -> Self {
        let prev = ACTIVE.with(|a| a.get());
        ACTIVE.with(|a| a.set(deadline.or(prev)));
        Self { prev }
    }
}

impl Drop for DeadlineScope {
    fn drop(&mut self) {
        let prev = self.prev;
        ACTIVE.with(|a| a.set(prev));
    }
}

/// The deadline of the query currently executing on this thread, if any.
pub(crate) fn active_deadline() -> Option<Instant> {
    ACTIVE.with(|a| a.get())
}

/// True once `deadline` has passed or its query was cancelled.
#[inline]
pub fn deadline_reached(deadline: Instant) -> bool {
    Instant::now() >= deadline || is_cancelled(deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_visible_and_forgotten_on_drop() {
        let mut a = CancellableDeadline::new(None);
        let b = CancellableDeadline::new(None);
        assert_ne!(a.deadline(), b.deadline());
        assert!(!deadline_reached(a.deadline()));
        a.cancel();
        assert!(deadline_reached(a.deadline()));
        assert!(!deadline_reached(b.deadline()));
        let d = a.deadline();
        drop(a);
        assert!(!is_cancelled(d));
    }

    #[test]
    fn timeout_still_fires() {
        let a = CancellableDeadline::new(Some(Duration::ZERO));
        std::thread::sleep(Duration::from_millis(1));
        assert!(deadline_reached(a.deadline()));
    }
}
