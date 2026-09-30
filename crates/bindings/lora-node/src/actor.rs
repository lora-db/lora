//! Shared plumbing for the binding's actor threads (interactive
//! transactions and mutating row streams).
//!
//! An actor owns something that must stay on one thread (a writer lock
//! guard) and answers commands sent from the JS thread. Two rules keep the
//! JS thread from ever hanging on one:
//!
//! - Every command carries a [`Done`] that settles its JS promise. If the
//!   actor never runs the command (it had already stopped, it was closed,
//!   it panicked), dropping the `Done` settles the promise with a "closed"
//!   error, so no promise stays pending and no napi deferred keeps the
//!   process alive.
//! - The JS thread never waits for an actor that may itself be waiting: for
//!   the writer lock (which another transaction may hold while it waits for
//!   the JS thread to send its next command) or for a statement. Dropping a
//!   handle closes the actor and joins it only when it is idle or ending,
//!   which is prompt; otherwise the thread is detached and finishes by
//!   itself (it rolls back as soon as it has the lock or its statement
//!   ends).

use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

type Callback<T> = Box<dyn FnOnce(Result<T, String>) + Send + 'static>;

/// Called once with a command's outcome, usually on the actor's thread.
/// Dropped without being called, it reports `closed` instead.
pub(crate) struct Done<T> {
    callback: Option<Callback<T>>,
    closed: fn() -> String,
}

impl<T> Done<T> {
    /// `closed` builds the error reported if the callback is never called.
    pub(crate) fn new(
        closed: fn() -> String,
        callback: impl FnOnce(Result<T, String>) + Send + 'static,
    ) -> Self {
        Self {
            callback: Some(Box::new(callback)),
            closed,
        }
    }

    pub(crate) fn call(mut self, result: Result<T, String>) {
        if let Some(callback) = self.callback.take() {
            callback(result);
        }
    }
}

impl<T> Drop for Done<T> {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            callback(Err((self.closed)()));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Opening: possibly waiting for the writer lock.
    Opening,
    /// Open and waiting for a command.
    Idle,
    /// Running a command.
    Busy,
    /// About to return: joining it waits for nothing but its cleanup.
    Ending,
}

struct State {
    phase: Phase,
    closed: bool,
}

/// Where an actor is in its life, shared between the actor and its handle.
#[derive(Clone)]
pub(crate) struct Lifecycle(Arc<Mutex<State>>);

impl Lifecycle {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            phase: Phase::Opening,
            closed: false,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Actor: the resource is open. False once the handle closed it; the
    /// actor must then release it and exit without running anything.
    pub(crate) fn opened(&self) -> bool {
        let mut state = self.lock();
        if state.closed {
            return false;
        }
        state.phase = Phase::Idle;
        true
    }

    /// Actor: about to run a received command. False once the handle
    /// closed it; the actor must then drop the command (failing it) and
    /// exit.
    pub(crate) fn begin_command(&self) -> bool {
        let mut state = self.lock();
        if state.closed {
            return false;
        }
        state.phase = Phase::Busy;
        true
    }

    /// Actor: the command is done and the actor waits for the next one.
    pub(crate) fn idle(&self) {
        let mut state = self.lock();
        if state.phase == Phase::Busy {
            state.phase = Phase::Idle;
        }
    }

    /// Actor: about to return. Call before settling the last command, so a
    /// handle dropped in reaction to it joins the thread.
    pub(crate) fn ending(&self) {
        self.lock().phase = Phase::Ending;
    }

    /// Handle: stop the actor. Returns whether joining it is prompt (it is
    /// idle or ending, and no command will run from now on).
    pub(crate) fn close(&self) -> bool {
        let mut state = self.lock();
        state.closed = true;
        matches!(state.phase, Phase::Idle | Phase::Ending)
    }
}

/// Stop an actor's thread from its handle: join it if that is prompt,
/// otherwise let it finish by itself. Call after [`Lifecycle::close`] and
/// closing its channel.
pub(crate) fn finish(lifecycle: &Lifecycle, thread: Option<JoinHandle<()>>) {
    let prompt = lifecycle.close();
    if let Some(thread) = thread {
        // Never join from the actor's own thread (a handle dropped by a
        // callback it runs).
        if prompt && thread.thread().id() != std::thread::current().id() {
            let _ = thread.join();
        }
    }
}
