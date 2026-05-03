//! [`WalRecorder`] — adapter from `MutationRecorder` to the durable
//! [`Wal`].
//!
//! Lifecycle, viewed from `lora-database::Database::execute_with_params`:
//!
//! 1. Acquire the store write lock.
//! 2. `recorder.arm()` — marks the recorder as inside-a-query but
//!    appends nothing to the WAL yet. A pure read query that fires
//!    no `MutationEvent` therefore touches the WAL zero times: no
//!    `TxBegin`, no `TxCommit`, no `flush`, no `fsync`.
//! 3. Run analyze + compile + execute. The executor mutates the
//!    in-memory store, which fires `MutationRecorder::record` for each
//!    primitive mutation. The adapter buffers those events in memory.
//! 4. On Ok: `recorder.commit_and_flush_if_needed()` writes `TxBegin`,
//!    one `MutationBatch`, and `TxCommit`, then flushes only when
//!    `commit()` returned `WroteCommit::Yes`. A read-only query returns
//!    `WroteCommit::No` and skips the flush entirely.
//! 5. On Err / panic: `recorder.abort()`. If any mutation events were
//!    buffered, the host quarantines the live handle because the engine
//!    has no rollback. Durable recovery stays atomic because the failed
//!    query never writes a committed batch to the WAL.
//! 6. Before returning, the host inspects `recorder.poisoned()` once.
//!    If `Some`, the query fails loudly with a durability error so
//!    the caller can act on it; the WAL is now refusing further
//!    appends until the operator restarts the database, which
//!    recovers from the last consistent snapshot + WAL.
//!
//! ### Hot-path cost
//!
//! `record` is called once per primitive mutation. It now takes only the
//! recorder mutex and pushes a clone into a query-local buffer; the WAL mutex,
//! framing, checksum, and segment append work happen once at commit time.
//!
//! ### When `record` fires after a failed in-memory mutation
//!
//! `InMemoryGraph::emit` only calls the recorder *after* the mutation
//! has been committed to the in-memory state. If the subsequent WAL
//! append fails, the live in-memory store is briefly ahead of disk:
//! the next query sees the partial state, but the next query also
//! observes `poisoned() = Some(_)` and is rejected. Recovery from a
//! snapshot + WAL after operator restart will not include the failed
//! mutation, so durable state stays consistent. The cost is "the live
//! process is wrong until the next restart"; the gain is that the
//! storage trait does not need to learn about durability.

use std::sync::{Arc, Mutex, MutexGuard};

use lora_store::{MutationEvent, MutationRecorder};

use super::errors::{WalBufferedCommitError, WalCommitError, WalPoisonError, WroteCommit};
use super::mirror::WalMirror;
use crate::errors::WalError;
use crate::lsn::Lsn;
use crate::wal::Wal;

#[derive(Default)]
struct RecorderState {
    /// True between `arm()` and the matching `commit()` / `abort()`.
    /// Marks the host's critical section without committing the WAL
    /// to a transaction yet — the actual `Wal::begin` happens lazily
    /// on the first mutation event.
    armed: bool,
    /// LSN of the currently-open WAL transaction, if any. Normally this is
    /// only set inside `commit()` while the buffered batch is being written.
    active_tx: Option<Lsn>,
    /// Query-local mutation buffer. This lets write-heavy statements commit
    /// as one `MutationBatch` record instead of one framed record per event.
    buffer: Vec<MutationEvent>,
    /// Sticky failure flag. Once set, [`MutationRecorder::record`]
    /// becomes a no-op (we cannot append safely) and `poisoned`
    /// surfaces the message.
    poisoned: Option<String>,
}

/// Adapter that lets a [`Wal`] act as a [`MutationRecorder`] on
/// [`lora_store::InMemoryGraph::set_mutation_recorder`].
pub struct WalRecorder {
    wal: Arc<Wal>,
    mirror: Option<Arc<dyn WalMirror>>,
    state: Mutex<RecorderState>,
}

impl WalRecorder {
    pub fn new(wal: Arc<Wal>) -> Self {
        Self::new_with_mirror(wal, None)
    }

    pub fn new_with_mirror(wal: Arc<Wal>, mirror: Option<Arc<dyn WalMirror>>) -> Self {
        Self {
            wal,
            mirror,
            state: Mutex::new(RecorderState::default()),
        }
    }

    /// Underlying log handle. Exposed so admin paths
    /// (`Database::checkpoint_to`, `truncate_up_to`) can hit the WAL
    /// directly without going through the recorder's transaction
    /// state machine.
    pub fn wal(&self) -> &Arc<Wal> {
        &self.wal
    }

    fn state_lock(&self) -> MutexGuard<'_, RecorderState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Mark the recorder as inside a query critical section. No WAL
    /// I/O happens here — `Wal::begin` is deferred until the first
    /// mutation event fires. A pure read query that never produces a
    /// `MutationEvent` therefore costs the WAL nothing: no record
    /// allocation, no buffer drain, no `fsync`.
    ///
    /// Errors with [`WalError::Poisoned`] if a prior failure has
    /// poisoned the recorder, or if the host is double-arming
    /// (`arm` already in effect).
    pub fn arm(&self) -> Result<(), WalError> {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        if state.armed {
            state.poisoned = Some("WalRecorder::arm called while already armed".into());
            return Err(WalError::Poisoned);
        }
        state.armed = true;
        state.buffer.clear();
        Ok(())
    }

    /// Append a `TxCommit` for the active transaction (if any) and
    /// clear the armed/active state.
    ///
    /// Returns:
    /// - [`WroteCommit::Yes`] when a lazy `TxBegin` had been issued
    ///   and a matching `TxCommit` was now appended. The host should
    ///   `flush()` next under `SyncMode::PerCommit`.
    /// - [`WroteCommit::No`] when no mutations fired during the query
    ///   and no records were written. The host should skip `flush()`.
    pub fn commit(&self) -> Result<WroteCommit, WalError> {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        if !state.armed {
            state.poisoned = Some("WalRecorder::commit called without an armed query".into());
            return Err(WalError::Poisoned);
        }
        state.armed = false;
        if state.buffer.is_empty() && state.active_tx.is_none() {
            return Ok(WroteCommit::No);
        }

        let events = std::mem::take(&mut state.buffer);
        let tx = match state.active_tx {
            Some(tx) => tx,
            None => self.wal.begin().inspect_err(|e| {
                state.poisoned = Some(e.to_string());
            })?,
        };
        state.active_tx = Some(tx);

        self.wal.append_batch(tx, events).inspect_err(|e| {
            state.poisoned = Some(e.to_string());
        })?;
        self.wal.commit(tx).inspect_err(|e| {
            state.poisoned = Some(e.to_string());
        })?;
        state.active_tx = None;
        Ok(WroteCommit::Yes)
    }

    /// Commit the currently armed recorder and flush only when a commit record
    /// was written. This is the normal durable boundary for query-scoped writes.
    pub fn commit_and_flush_if_needed(&self) -> Result<WroteCommit, WalCommitError> {
        let wrote_commit = self.commit().map_err(WalCommitError::Commit)?;
        if wrote_commit.wrote() {
            self.flush().map_err(WalCommitError::Flush)?;
        }
        Ok(wrote_commit)
    }

    /// Commit an explicit transaction's buffered mutation events as one durable
    /// WAL transaction. The recorder is armed only for this replay window.
    pub fn commit_events(
        &self,
        events: impl IntoIterator<Item = MutationEvent>,
    ) -> Result<WroteCommit, WalBufferedCommitError> {
        let mut events = events.into_iter().peekable();
        if events.peek().is_none() {
            self.ensure_not_poisoned()
                .map_err(|e| WalBufferedCommitError::Poisoned(e.reason().to_string()))?;
            return Ok(WroteCommit::No);
        }

        self.arm().map_err(WalBufferedCommitError::Arm)?;
        for event in events {
            self.record(event);
            if let Some(reason) = self.poisoned_reason() {
                return Err(WalBufferedCommitError::ReplayPoisoned(reason));
            }
        }

        self.commit_and_flush_if_needed().map_err(Into::into)
    }

    /// Append a `TxAbort` for the active transaction (if any) and
    /// clear the armed/active state. Returns `Ok(true)` when the live graph
    /// may have observed mutations and should be quarantined, `Ok(false)` when
    /// the query never mutated anything.
    pub fn abort(&self) -> Result<bool, WalError> {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        // Tolerate "abort without arm" — the host calls abort in
        // unwind paths and we'd rather no-op than poison.
        state.armed = false;
        let had_buffered_events = !state.buffer.is_empty();
        state.buffer.clear();
        match state.active_tx.take() {
            Some(tx) => {
                self.wal.abort(tx).inspect_err(|e| {
                    state.poisoned = Some(e.to_string());
                })?;
                Ok(true)
            }
            None => Ok(had_buffered_events),
        }
    }

    /// Flush the WAL — write the pending buffer to the OS and
    /// (under `SyncMode::PerCommit`) `fsync`.
    pub fn flush(&self) -> Result<(), WalError> {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        self.wal.flush().inspect_err(|e| {
            state.poisoned = Some(e.to_string());
        })?;
        if let Some(mirror) = &self.mirror {
            mirror.persist(self.wal.dir()).inspect_err(|e| {
                state.poisoned = Some(e.to_string());
            })?;
        }
        Ok(())
    }

    /// Force the underlying WAL to write, `fsync`, and advance its
    /// durable fence regardless of the configured sync mode. Admin
    /// paths use this when they need a durability point immediately.
    pub fn force_fsync(&self) -> Result<(), WalError> {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        self.wal.force_fsync().inspect_err(|e| {
            state.poisoned = Some(e.to_string());
        })?;
        if let Some(mirror) = &self.mirror {
            mirror.persist_force(self.wal.dir()).inspect_err(|e| {
                state.poisoned = Some(e.to_string());
            })?;
        }
        Ok(())
    }

    /// Append a `Checkpoint` marker. Used by the checkpoint admin
    /// path after a successful snapshot rename — the marker doubles
    /// as the log-side fence the next replay will trust.
    pub fn checkpoint_marker(&self, snapshot_lsn: Lsn) -> Result<Lsn, WalError> {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        self.wal.checkpoint_marker(snapshot_lsn).inspect_err(|e| {
            state.poisoned = Some(e.to_string());
        })
    }

    /// Drop sealed segments at or below `fence_lsn`. Forwards to
    /// [`Wal::truncate_up_to`].
    pub fn truncate_up_to(&self, fence_lsn: Lsn) -> Result<(), WalError> {
        // Archive-backed databases must stay self-contained. Until snapshot
        // checkpoint payloads are stored inside the archive too, preserving the
        // full WAL history is the only safe way to let the archive recover by
        // itself after a checkpoint marker.
        if let Some(mirror) = &self.mirror {
            mirror.persist_force(self.wal.dir())?;
            return Ok(());
        }
        self.wal.truncate_up_to(fence_lsn)?;
        Ok(())
    }

    /// True iff the recorder has already failed an append, **or** the
    /// background flusher has latched a failure. Cheap to poll under
    /// the store lock.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned_reason().is_some()
    }

    pub fn poisoned_reason(&self) -> Option<String> {
        let state = self.state_lock();
        if let Some(msg) = state.poisoned.clone() {
            return Some(msg);
        }
        self.wal.bg_failure()
    }

    pub fn ensure_not_poisoned(&self) -> Result<(), WalPoisonError> {
        if let Some(reason) = self.poisoned_reason() {
            return Err(WalPoisonError { reason });
        }
        Ok(())
    }

    /// Quarantine the recorder after the host detects that the live
    /// in-memory graph may no longer match durable state. Once poisoned,
    /// future query arms fail until the database is restarted from a
    /// snapshot + WAL.
    pub fn poison(&self, reason: impl Into<String>) {
        let mut state = self.state_lock();
        state.poisoned.get_or_insert_with(|| reason.into());
        state.active_tx = None;
        state.armed = false;
        state.buffer.clear();
    }

    /// Test helper: clear the poisoned flag and reset the active
    /// transaction. Production code should not call this — once the
    /// WAL is poisoned the right move is to fail loudly and let the
    /// operator restart from the last snapshot + WAL.
    #[doc(hidden)]
    pub fn clear_poisoned_for_tests(&self) {
        let mut state = self.state_lock();
        state.poisoned = None;
        state.active_tx = None;
        state.armed = false;
        state.buffer.clear();
    }
}

impl MutationRecorder for WalRecorder {
    fn record(&self, event: MutationEvent) {
        let mut state = self.state_lock();
        if state.poisoned.is_some() {
            return;
        }
        if !state.armed {
            state.poisoned.get_or_insert_with(|| {
                "MutationRecorder::record fired outside an armed query".into()
            });
            return;
        }
        state.buffer.push(event);
    }

    fn poisoned(&self) -> Option<String> {
        // Surface a latched bg-flusher failure too — the recorder is
        // the host's single point of contact for "is the WAL still
        // safe to commit through?".
        self.poisoned_reason()
    }
}
