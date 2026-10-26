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
//!
//! Each actor is an unpooled OS thread, and queuing actors instead could
//! deadlock (a transaction waiting in the queue for a thread while the JS
//! code that would let one go waits for it). So a database caps how many
//! it runs at once ([`OpenSlots`]) and rejects one past the cap at once
//! with `LORA_TOO_MANY_OPEN`. An actor's [`Slot`] lives in its
//! [`Lifecycle`] and is released when the actor ends: before it settles
//! its last command, or when its thread exits (a panic included), so a
//! caller that hears the transaction finished can open the next one.

use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Default for `maxOpenTransactions`.
pub(crate) const DEFAULT_MAX_OPEN: u32 = 1024;

/// Wire code of the error an actor past the cap is rejected with.
pub(crate) const TOO_MANY_OPEN_CODE: &str = "LORA_TOO_MANY_OPEN";

/// One database's budget of actors (interactive transactions, write
/// batches and mutating streams on their own thread).
pub(crate) struct OpenSlots {
    open: AtomicUsize,
    max: usize,
}

impl OpenSlots {
    pub(crate) fn new(max: u32) -> Arc<Self> {
        Arc::new(Self {
            open: AtomicUsize::new(0),
            max: max as usize,
        })
    }

    /// Take a slot, or fail with `LORA_TOO_MANY_OPEN` when all are in use.
    pub(crate) fn acquire(self: &Arc<Self>) -> Result<Slot, String> {
        // Taken (and handed back on drop) first, so a full budget is never
        // overrun; a caller racing a full budget may be refused while
        // another gives a slot back, never let in past it.
        let slot = Slot(self.clone());
        if self.open.fetch_add(1, Ordering::SeqCst) < self.max {
            return Ok(slot);
        }
        drop(slot);
        Err(format!(
            "{TOO_MANY_OPEN_CODE}: too many open transactions and write streams on this database (`maxOpenTransactions` is {}); finish or close one first",
            self.max
        ))
    }

    #[cfg(test)]
    fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// One taken slot of an [`OpenSlots`], given back on drop.
pub(crate) struct Slot(Arc<OpenSlots>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::SeqCst);
    }
}

struct State {
    phase: Phase,
    closed: bool,
    /// The actor's slot, released when it ends.
    slot: Option<Slot>,
}

/// Where an actor is in its life, shared between the actor and its handle.
#[derive(Clone)]
pub(crate) struct Lifecycle(Arc<Mutex<State>>);

impl Lifecycle {
    pub(crate) fn new(slot: Slot) -> Self {
        Self(Arc::new(Mutex::new(State {
            phase: Phase::Opening,
            closed: false,
            slot: Some(slot),
        })))
    }

    /// Actor: hold for the thread's whole run. Dropped when the thread
    /// exits, normally or by a panic, it marks the actor ending and so
    /// releases its slot even on a path that did not call [`Self::ending`].
    pub(crate) fn exit_guard(&self) -> ExitGuard {
        ExitGuard(self.clone())
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
    /// handle dropped in reaction to it joins the thread, and a new actor
    /// started in reaction to it finds the slot free.
    pub(crate) fn ending(&self) {
        let slot = {
            let mut state = self.lock();
            state.phase = Phase::Ending;
            state.slot.take()
        };
        drop(slot);
    }

    /// Handle: stop the actor. Returns whether joining it is prompt (it is
    /// idle or ending, and no command will run from now on).
    pub(crate) fn close(&self) -> bool {
        let mut state = self.lock();
        state.closed = true;
        matches!(state.phase, Phase::Idle | Phase::Ending)
    }
}

/// See [`Lifecycle::exit_guard`].
pub(crate) struct ExitGuard(Lifecycle);

impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.0.ending();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_capped_and_given_back() {
        let slots = OpenSlots::new(2);
        let a = slots.acquire().unwrap();
        let b = slots.acquire().unwrap();
        let err = slots.acquire().err().unwrap();
        assert!(err.starts_with("LORA_TOO_MANY_OPEN: "), "{err}");
        drop(a);
        let c = slots.acquire().unwrap();
        drop((b, c));
        assert_eq!(slots.open(), 0);
    }

    #[test]
    fn an_actor_gives_its_slot_back_when_it_ends_or_its_thread_exits() {
        let slots = OpenSlots::new(1);
        let lifecycle = Lifecycle::new(slots.acquire().unwrap());
        lifecycle.ending();
        assert_eq!(slots.open(), 0);

        let lifecycle = Lifecycle::new(slots.acquire().unwrap());
        let exit = lifecycle.exit_guard();
        let thread = std::thread::spawn(move || {
            let _exit = exit;
            panic!("actor panicked");
        });
        assert!(thread.join().is_err());
        assert_eq!(slots.open(), 0);
        // The handle still holds the lifecycle: the slot is free anyway.
        drop(lifecycle);
    }
}
