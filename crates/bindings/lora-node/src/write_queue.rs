//! Auto-commit writes that may wait for a writer lock JS code holds.
//!
//! While an interactive transaction or a mutating stream exists (see
//! [`crate::stream::js_bound_writers`]), a write may wait for the writer
//! lock for as long as JS code keeps that transaction open. It cannot wait
//! on the libuv pool (see [`crate::tasks::spawn`]), and waiting on an OS
//! thread of its own meant N writes behind a long transaction started N
//! threads. So such writes wait here instead: a first-in, first-out queue
//! per database handle, run one at a time by one thread. Writes serialize
//! on the writer lock anyway, so running them one by one loses nothing.
//!
//! Nothing can deadlock on the queue: every job in it needs this handle's
//! engine's writer lock, so a job queued behind one that waits for the
//! lock could not have run before the lock is released anyway. Operations
//! that do not need the lock, or need it only briefly (`sync()`, reads,
//! snapshot saves), never go through the queue.
//!
//! A queued job still honours its deadline and its cancellation: a second
//! thread, running only while some queued job has a deadline, rejects
//! each one whose deadline passes (or whose query is cancelled, see
//! [`WriteQueue::poke`]) while it is still queued. Time spent queued
//! counts against the deadline. Closing the handle rejects every queued
//! job (see [`WriteQueue::reject_all`]).

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use lora_database::{LoraError, LoraErrorCode};

use crate::actor::Done;
use crate::errors::{closed_error_message, format_lora_error, LORA_ERROR_CODE};
use crate::SharedCancel;

/// When a queued job gives up.
pub(crate) enum Expiry {
    Never,
    At(Instant),
    /// A cancellable deadline shared with JS through a token.
    Cancel(SharedCancel),
}

impl Expiry {
    /// The instant it expires by itself, if any.
    fn deadline(&self) -> Option<Instant> {
        match self {
            Expiry::Never => None,
            Expiry::At(at) => Some(*at),
            Expiry::Cancel(handle) => {
                Some(handle.lock().unwrap_or_else(|p| p.into_inner()).deadline())
            }
        }
    }

    fn expired(&self, now: Instant) -> bool {
        match self {
            Expiry::Never => false,
            Expiry::At(at) => now >= *at,
            Expiry::Cancel(handle) => {
                let handle = handle.lock().unwrap_or_else(|p| p.into_inner());
                handle.is_cancelled() || now >= handle.deadline()
            }
        }
    }
}

/// A queued job: called with `Ok(())` to run it, with an error to reject
/// it. Dropped uncalled, it rejects as closed.
pub(crate) type Job = Done<()>;

/// Build a [`Job`].
pub(crate) fn job(callback: impl FnOnce(Result<(), String>) + Send + 'static) -> Job {
    Done::new(closed_error_message, callback)
}

struct Queued {
    expiry: Expiry,
    job: Job,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Queued>,
    /// The runner thread is alive.
    running: bool,
    /// The timer thread is alive.
    timing: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Wakes the timer: a job was queued, taken or cancelled.
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// One database handle's queue of waiting auto-commit writes.
#[derive(Default)]
pub(crate) struct WriteQueue(Arc<Shared>);

/// The timer sleeps at most this long between checks.
const MAX_SLEEP: Duration = Duration::from_secs(60);

impl WriteQueue {
    /// Queue `job`, to run once every job queued before it has.
    pub(crate) fn push(&self, expiry: Expiry, job: Job) {
        let timed = !matches!(expiry, Expiry::Never);
        let (start_runner, start_timer) = {
            let mut state = self.0.lock();
            state.queue.push_back(Queued { expiry, job });
            let start_runner = !std::mem::replace(&mut state.running, true);
            let start_timer = timed && !std::mem::replace(&mut state.timing, true);
            (start_runner, start_timer)
        };
        self.0.wake.notify_all();
        if start_runner {
            let shared = self.0.clone();
            if let Err(e) = lora_binding_buffer::stream::run_on_actor_thread(move || run(shared)) {
                // Nothing runs the queue: fail what is in it.
                let jobs = {
                    let mut state = self.0.lock();
                    state.running = false;
                    std::mem::take(&mut state.queue)
                };
                let message = format!("{LORA_ERROR_CODE}: could not start a query thread: {e}");
                for queued in jobs {
                    queued.job.call(Err(message.clone()));
                }
            }
        }
        if start_timer {
            let shared = self.0.clone();
            if lora_binding_buffer::stream::run_on_actor_thread(move || time(shared)).is_err() {
                // Only late: a job past its deadline still fails when it runs.
                self.0.lock().timing = false;
            }
        }
    }

    /// Check queued jobs for cancellation now (a query was cancelled).
    pub(crate) fn poke(&self) {
        self.0.wake.notify_all();
    }

    /// Reject every queued job with `message`. One already running is not
    /// affected.
    pub(crate) fn reject_all(&self, message: String) {
        let jobs = std::mem::take(&mut self.0.lock().queue);
        self.0.wake.notify_all();
        for queued in jobs {
            queued.job.call(Err(message.clone()));
        }
    }

    #[cfg(test)]
    fn threads(&self) -> (bool, bool) {
        let state = self.0.lock();
        (state.running, state.timing)
    }
}

/// Take the queued jobs that expired.
fn take_expired(state: &mut State) -> Vec<Job> {
    let now = Instant::now();
    if !state.queue.iter().any(|q| q.expiry.expired(now)) {
        return Vec::new();
    }
    let (expired, kept) = std::mem::take(&mut state.queue)
        .into_iter()
        .partition::<Vec<_>, _>(|q| q.expiry.expired(now));
    state.queue = kept.into();
    expired.into_iter().map(|q| q.job).collect()
}

fn reject_expired(jobs: Vec<Job>) {
    for job in jobs {
        job.call(Err(format_lora_error(&LoraError::new(
            LoraErrorCode::Timeout,
            "query deadline exceeded waiting for the writer lock: another write or open transaction holds it",
        ))));
    }
}

/// The runner: run queued jobs in order until none is left.
fn run(shared: Arc<Shared>) {
    loop {
        let (next, expired) = {
            let mut state = shared.lock();
            let expired = take_expired(&mut state);
            let next = state.queue.pop_front();
            if next.is_none() {
                state.running = false;
            }
            (next, expired)
        };
        // The timer may now have nothing left to wait for.
        shared.wake.notify_all();
        reject_expired(expired);
        let Some(next) = next else {
            return;
        };
        // A job settles its own promise, panics included; never let one
        // stop the queue.
        let _ = catch_unwind(AssertUnwindSafe(|| next.job.call(Ok(()))));
    }
}

/// The timer: reject queued jobs as they expire, while any has a deadline.
fn time(shared: Arc<Shared>) {
    let mut state = shared.lock();
    loop {
        let expired = take_expired(&mut state);
        if !expired.is_empty() {
            drop(state);
            reject_expired(expired);
            state = shared.lock();
            continue;
        }
        let Some(next) = state.queue.iter().filter_map(|q| q.expiry.deadline()).min() else {
            state.timing = false;
            return;
        };
        let sleep = next
            .saturating_duration_since(Instant::now())
            .min(MAX_SLEEP);
        state = shared
            .wake
            .wait_timeout(state, sleep)
            .unwrap_or_else(|p| p.into_inner())
            .0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    type Outcome = (u32, Result<(), String>);

    fn recorder() -> (mpsc::Sender<Outcome>, mpsc::Receiver<Outcome>) {
        mpsc::channel()
    }

    fn wait_idle(queue: &WriteQueue) {
        let until = Instant::now() + Duration::from_secs(10);
        while queue.threads() != (false, false) {
            assert!(Instant::now() < until, "queue threads did not stop");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn runs_jobs_in_order_and_expires_waiting_ones() {
        let queue = WriteQueue::default();
        let (tx, rx) = recorder();
        let (release, blocked) = mpsc::channel::<()>();
        let first = tx.clone();
        queue.push(
            Expiry::Never,
            job(move |r| {
                let _ = blocked.recv();
                first.send((0, r)).unwrap();
            }),
        );
        for i in 1..4 {
            let tx = tx.clone();
            let expiry = if i == 2 {
                Expiry::At(Instant::now() + Duration::from_millis(20))
            } else {
                Expiry::Never
            };
            queue.push(expiry, job(move |r| tx.send((i, r)).unwrap()));
        }
        // Job 2 expires while job 0 still blocks the runner.
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(i, 2);
        assert!(r.unwrap_err().starts_with("LORA_TIMEOUT: "));
        release.send(()).unwrap();
        let rest: Vec<_> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap())
            .collect();
        assert_eq!(rest.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 3]);
        assert!(rest.iter().all(|(_, r)| r.is_ok()));
        wait_idle(&queue);
    }

    #[test]
    fn rejects_queued_jobs_on_close() {
        let queue = WriteQueue::default();
        let (tx, rx) = recorder();
        let (release, blocked) = mpsc::channel::<()>();
        let (running, started) = mpsc::channel::<()>();
        let first = tx.clone();
        queue.push(
            Expiry::Never,
            job(move |r| {
                running.send(()).unwrap();
                let _ = blocked.recv();
                first.send((0, r)).unwrap();
            }),
        );
        let second = tx.clone();
        queue.push(Expiry::Never, job(move |r| second.send((1, r)).unwrap()));
        // Job 0 runs (blocking the runner); job 1 is queued.
        started.recv_timeout(Duration::from_secs(10)).unwrap();
        queue.reject_all(closed_error_message());
        let (i, r) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(i, 1);
        assert_eq!(r.unwrap_err(), closed_error_message());
        release.send(()).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap().1.is_ok());
        wait_idle(&queue);
    }

    #[test]
    fn a_panicking_job_does_not_stop_the_queue() {
        let queue = WriteQueue::default();
        let (tx, rx) = recorder();
        queue.push(Expiry::Never, job(|_| panic!("job panicked")));
        queue.push(Expiry::Never, job(move |r| tx.send((1, r)).unwrap()));
        assert!(rx.recv_timeout(Duration::from_secs(10)).unwrap().1.is_ok());
        wait_idle(&queue);
    }
}
