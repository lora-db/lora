//! Bridge between [`lora_store::MutationRecorder`] (the storage-side
//! observer hook) and [`crate::Wal`] (the durable log handle).
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
//! 4. On Ok: `recorder.commit()` writes `TxBegin`, one `MutationBatch`,
//!    and `TxCommit`, then the host runs `recorder.flush()` (per the
//!    configured `SyncMode`) **only** when `commit()` returned
//!    `WroteCommit::Yes`. A read-only query returns `WroteCommit::No`
//!    and the host skips the flush entirely.
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

use std::path::Path;
use std::sync::{Arc, Mutex};

use lora_store::{MutationEvent, MutationRecorder};

use crate::error::WalError;
use crate::lsn::Lsn;
use crate::wal::Wal;

/// Whether [`WalRecorder::commit`] actually wrote a `TxCommit` to the
/// log. Read-only queries — those that never trigger
/// `MutationRecorder::record` — return [`WroteCommit::No`] so the host
/// can skip the surrounding `flush()` and avoid a per-query `fsync`
/// just to record an empty transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WroteCommit {
    /// A `TxBegin` had been lazily allocated and was paired with a
    /// matching `TxCommit`. Caller should `flush()` (under PerCommit).
    Yes,
    /// No mutation events fired during the query, so neither `TxBegin`
    /// nor `TxCommit` was appended. Caller can skip `flush()` entirely.
    No,
}

/// Optional side-effect after the WAL has successfully flushed.
///
/// The core WAL stays directory/segment based for append performance. Higher
/// layers can install a mirror to copy that durable directory into another
/// representation, such as the portable `.loradb` archive file used by named
/// databases.
pub trait WalMirror: Send + Sync {
    fn persist(&self, wal_dir: &Path) -> Result<(), WalError>;
}

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
        let mut state = self.state.lock().unwrap();
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
        let mut state = self.state.lock().unwrap();
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

    /// Append a `TxAbort` for the active transaction (if any) and
    /// clear the armed/active state. Returns `Ok(true)` when the live graph
    /// may have observed mutations and should be quarantined, `Ok(false)` when
    /// the query never mutated anything.
    pub fn abort(&self) -> Result<bool, WalError> {
        let mut state = self.state.lock().unwrap();
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
        let mut state = self.state.lock().unwrap();
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
        let mut state = self.state.lock().unwrap();
        if state.poisoned.is_some() {
            return Err(WalError::Poisoned);
        }
        self.wal.force_fsync().inspect_err(|e| {
            state.poisoned = Some(e.to_string());
        })?;
        if let Some(mirror) = &self.mirror {
            mirror.persist(self.wal.dir()).inspect_err(|e| {
                state.poisoned = Some(e.to_string());
            })?;
        }
        Ok(())
    }

    /// Append a `Checkpoint` marker. Used by the checkpoint admin
    /// path after a successful snapshot rename — the marker doubles
    /// as the log-side fence the next replay will trust.
    pub fn checkpoint_marker(&self, snapshot_lsn: Lsn) -> Result<Lsn, WalError> {
        let mut state = self.state.lock().unwrap();
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
            mirror.persist(self.wal.dir())?;
            return Ok(());
        }
        self.wal.truncate_up_to(fence_lsn)?;
        Ok(())
    }

    /// True iff the recorder has already failed an append, **or** the
    /// background flusher has latched a failure. Cheap to poll under
    /// the store lock.
    pub fn is_poisoned(&self) -> bool {
        if self.state.lock().unwrap().poisoned.is_some() {
            return true;
        }
        self.wal.bg_failure().is_some()
    }

    /// Quarantine the recorder after the host detects that the live
    /// in-memory graph may no longer match durable state. Once poisoned,
    /// future query arms fail until the database is restarted from a
    /// snapshot + WAL.
    pub fn poison(&self, reason: impl Into<String>) {
        let mut state = self.state.lock().unwrap();
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
        let mut state = self.state.lock().unwrap();
        state.poisoned = None;
        state.active_tx = None;
        state.armed = false;
        state.buffer.clear();
    }
}

impl MutationRecorder for WalRecorder {
    fn record(&self, event: &MutationEvent) {
        let mut state = self.state.lock().unwrap();
        if state.poisoned.is_some() {
            return;
        }
        if !state.armed {
            state.poisoned.get_or_insert_with(|| {
                "MutationRecorder::record fired outside an armed query".into()
            });
            return;
        }
        state.buffer.push(event.clone());
    }

    fn poisoned(&self) -> Option<String> {
        // Surface a latched bg-flusher failure too — the recorder is
        // the host's single point of contact for "is the WAL still
        // safe to commit through?".
        let state = self.state.lock().unwrap();
        if let Some(msg) = state.poisoned.clone() {
            return Some(msg);
        }
        self.wal.bg_failure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use lora_store::{GraphStorageMut, InMemoryGraph, MutationEvent, Properties, PropertyValue};

    use crate::config::SyncMode;
    use crate::testing::TmpDir;
    use crate::Wal;

    fn open_wal(dir: &std::path::Path) -> Arc<Wal> {
        let (wal, replay) =
            Wal::open(dir, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
        assert!(replay.is_empty());
        wal
    }

    #[test]
    fn record_outside_arm_poisons() {
        let dir = TmpDir::new("no-arm");
        let recorder = WalRecorder::new(open_wal(&dir.path));
        recorder.record(&MutationEvent::Clear);
        assert!(recorder.is_poisoned());
        let msg = recorder.poisoned().unwrap();
        assert!(msg.contains("outside an armed query"));
    }

    #[test]
    fn arm_record_commit_round_trip_via_in_memory_graph() {
        let dir = TmpDir::new("happy");
        let recorder: Arc<WalRecorder> = Arc::new(WalRecorder::new(open_wal(&dir.path)));

        let mut g = InMemoryGraph::new();
        g.set_mutation_recorder(Some(recorder.clone()));

        recorder.arm().unwrap();
        let mut props = Properties::new();
        props.insert("v".into(), PropertyValue::Int(1));
        g.create_node(vec!["N".into()], props);
        let mut props2 = Properties::new();
        props2.insert("v".into(), PropertyValue::Int(2));
        g.create_node(vec!["N".into()], props2);
        let outcome = recorder.commit().unwrap();
        assert_eq!(outcome, WroteCommit::Yes);
        recorder.flush().unwrap();

        assert!(!recorder.is_poisoned());

        // Drop every recorder clone before re-opening the directory,
        // otherwise we'd race with our own live WAL handle.
        g.set_mutation_recorder(None);
        drop(recorder);

        let (_wal, events) =
            Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], MutationEvent::CreateNode { id: 0, .. }));
        assert!(matches!(events[1], MutationEvent::CreateNode { id: 1, .. }));
    }

    #[test]
    fn arm_then_commit_with_no_mutations_writes_nothing() {
        let dir = TmpDir::new("ro");
        let recorder = WalRecorder::new(open_wal(&dir.path));

        // Simulate a read-only query: arm + commit without any
        // intervening `record` calls.
        let next_before = recorder.wal().next_lsn();
        recorder.arm().unwrap();
        let outcome = recorder.commit().unwrap();
        assert_eq!(outcome, WroteCommit::No);
        let next_after = recorder.wal().next_lsn();
        assert_eq!(
            next_before, next_after,
            "read-only commit must not allocate any LSNs"
        );
    }

    #[test]
    fn abort_drops_in_flight_events_on_replay() {
        let dir = TmpDir::new("abort");
        let recorder: Arc<WalRecorder> = Arc::new(WalRecorder::new(open_wal(&dir.path)));

        let mut g = InMemoryGraph::new();
        g.set_mutation_recorder(Some(recorder.clone()));

        // Tx 1 commits.
        recorder.arm().unwrap();
        g.create_node(vec!["A".into()], Properties::new());
        let _ = recorder.commit().unwrap();
        recorder.flush().unwrap();

        // Tx 2 aborts: the in-memory mutation already happened (the
        // engine has no rollback) but the WAL marks it aborted, so
        // recovery from a fresh process must skip it.
        recorder.arm().unwrap();
        g.create_node(vec!["B".into()], Properties::new());
        let aborted = recorder.abort().unwrap();
        assert!(aborted, "abort after buffered mutations should quarantine");
        recorder.flush().unwrap();

        g.set_mutation_recorder(None);
        drop(recorder);

        let (_wal, events) =
            Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap();
        assert_eq!(events.len(), 1);
        if let MutationEvent::CreateNode { labels, .. } = &events[0] {
            assert_eq!(labels, &vec!["A".to_string()]);
        } else {
            panic!("expected CreateNode for label A, got {:?}", events[0]);
        }
    }

    #[test]
    fn arm_while_armed_poisons() {
        let dir = TmpDir::new("double-arm");
        let recorder = WalRecorder::new(open_wal(&dir.path));
        recorder.arm().unwrap();
        let err = recorder.arm().unwrap_err();
        assert!(matches!(err, WalError::Poisoned));
        assert!(recorder.is_poisoned());
    }

    #[test]
    fn poisoned_recorder_swallows_subsequent_records() {
        let dir = TmpDir::new("swallow");
        let recorder = WalRecorder::new(open_wal(&dir.path));

        // Poison it.
        recorder.record(&MutationEvent::Clear);
        assert!(recorder.is_poisoned());

        // After poisoning, further `record` calls must NOT touch the
        // WAL or panic — they're a no-op so the engine can finish
        // unwinding before the host observes `poisoned()` and fails
        // the query.
        for _ in 0..10 {
            recorder.record(&MutationEvent::Clear);
        }
        assert!(recorder.is_poisoned());
    }

    #[test]
    fn checkpoint_marker_through_recorder() {
        let dir = TmpDir::new("ckpt");
        let recorder = WalRecorder::new(open_wal(&dir.path));

        recorder.arm().unwrap();
        recorder.record(&MutationEvent::Clear);
        assert_eq!(recorder.commit().unwrap(), WroteCommit::Yes);
        recorder.force_fsync().unwrap();
        let snapshot_lsn = recorder.wal().durable_lsn();

        // Exercise the marker path via the recorder's shim after a
        // real durable fence exists.
        let marker_lsn = recorder.checkpoint_marker(snapshot_lsn).unwrap();
        recorder.force_fsync().unwrap();
        assert!(marker_lsn >= Lsn::new(1));

        let outcome = crate::replay::replay_dir(&dir.path, Lsn::ZERO).unwrap();
        assert_eq!(outcome.checkpoint_lsn_observed, Some(snapshot_lsn));
    }
}
