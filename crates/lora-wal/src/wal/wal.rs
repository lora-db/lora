//! `Wal` — the durable log handle.
//!
//! Owns a WAL directory of the shape:
//!
//! ```text
//! <dir>/
//!   0000000001.wal      sealed segment
//!   0000000002.wal      sealed segment
//!   0000000003.wal      active segment
//! ```
//!
//! The active segment is identified by the highest numeric file name —
//! we deliberately do **not** keep a separate `CURRENT` pointer file.
//! A pointer would be a second source of truth that crashes can
//! desynchronise from the directory listing without buying anything:
//! the file names already encode their ordering, and segment headers
//! are self-describing.
//!
//! Lifecycle is `[`Wal::open`] → acquire the directory lock → drain replay
//! events into the store → resume normal `begin` / `append` / `commit`
//! traffic. The directory lock is held until the `Wal` drops; a second
//! live `Wal::open` on the same directory returns [`WalError::AlreadyOpen`].
//!
//! All public methods take `&self` and serialise through an internal
//! [`Mutex`]. The store write lock already serialises query commits in
//! production, so the inner mutex is uncontested and effectively free.

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use lora_store::MutationEvent;

#[cfg(not(target_arch = "wasm32"))]
use super::group_flusher::{spawn_group_flusher, GroupFlusherHandle};
use crate::config::SyncMode;
use crate::dir::{SegmentDir, SegmentId};
use crate::errors::WalError;
use crate::lock::DirLock;
use crate::lsn::Lsn;
use crate::record::WalRecord;
use crate::recorder::WroteCommit;
use crate::replay::{replay_segments, ReplayOutcome};
use crate::segment::SegmentWriter;

/// State guarded by the inner `Mutex`. Nothing in this struct is
/// `Send`-unsafe; the lock is purely for `&self`-safe interior
/// mutation.
struct WalState {
    next_lsn: Lsn,
    durable_lsn: Lsn,
    active_segment_id: SegmentId,
    active_writer: SegmentWriter,
    /// Lowest segment id still on disk. Bumped by `truncate_up_to`.
    oldest_segment_id: SegmentId,
}

/// Reserved latch for durability failures that occur outside the immediate
/// caller path. Wrapped in a `Mutex` instead of an
/// `AtomicCell<Option<String>>` because failures are rare and we want the
/// message preserved verbatim for operator-facing reporting
/// (`/admin/wal/status` `bgFailure`). Once `Some`, every subsequent
/// commit/flush returns [`WalError::Poisoned`] and the operator is expected to
/// restart from the last consistent snapshot + WAL.
type BgFailure = Mutex<Option<String>>;

/// Selects the durability work that [`Wal::flush_inner`] actually does.
/// Centralising the three modes here means `flush` and `force_fsync`
/// share one code path and the call sites don't have to remember which
/// mode advances `durable_lsn` and which does not.
#[derive(Debug, Clone, Copy)]
pub(super) enum FlushKind {
    /// Honour the configured [`SyncMode`]. This is what the recorder's
    /// `flush()` calls into.
    PerConfiguredMode,
    /// Always write the buffer + fsync + advance `durable_lsn`, regardless of
    /// mode. Used by checkpoints, explicit sync, and clean Group-mode drop.
    ForceFsync,
}

/// Live, append-side WAL handle.
///
/// Construct via [`Wal::open`]. The returned tuple includes the list of
/// committed mutation events that need to be re-applied to the
/// in-memory store before any new traffic is accepted.
///
/// `Wal::open` returns `Arc<Self>` because the optional Group-mode
/// background flusher needs a `Weak<Wal>` to call back into without
/// taking a strong reference (which would prevent shutdown).
pub struct Wal {
    segments: SegmentDir,
    sync_mode: SyncMode,
    segment_target_bytes: u64,
    state: Mutex<WalState>,
    /// Latched durability failure; surfaced via [`Wal::bg_failure`] and
    /// propagated to commit/flush/force_fsync as [`WalError::Poisoned`].
    bg_failure: Arc<BgFailure>,
    /// Background flusher for `SyncMode::Group`. `Drop` joins the
    /// thread, so a `Wal` going out of scope is a clean shutdown
    /// signal. Absent on `wasm32`, where Group mode falls back to the
    /// drop-time flush.
    #[cfg(not(target_arch = "wasm32"))]
    flusher: Mutex<Option<GroupFlusherHandle>>,
    /// Held for the lifetime of the WAL so a second handle cannot append
    /// to the same active segment concurrently.
    _dir_lock: DirLock,
}

impl Wal {
    /// Open or create the WAL directory at `dir`.
    ///
    /// `checkpoint_lsn` is the LSN stamped into the most recent
    /// snapshot the caller is restoring from (or [`Lsn::ZERO`] if
    /// there is no snapshot). Replay skips records at or below this
    /// fence — they are already represented in the loaded state.
    ///
    /// Returns `(wal, committed_events)`. The caller is expected to
    /// apply every event in `committed_events` to its in-memory store
    /// in order before issuing any new `begin` / `append` calls.
    pub fn open(
        dir: impl Into<std::path::PathBuf>,
        sync_mode: SyncMode,
        segment_target_bytes: u64,
        checkpoint_lsn: Lsn,
    ) -> Result<(Arc<Self>, Vec<MutationEvent>), WalError> {
        let segments = SegmentDir::new(dir);
        fs::create_dir_all(segments.root())?;
        let dir_lock = DirLock::acquire(segments.root())?;

        let entries = segments.list()?;
        let (active_id, active_writer, replay) = if entries.is_empty() {
            Self::open_fresh(&segments)?
        } else {
            Self::open_existing(&segments, &entries, checkpoint_lsn)?
        };

        let next_lsn = if replay.max_lsn.is_zero() {
            Lsn::new(1)
        } else {
            replay.max_lsn.next()
        };
        // Treat everything readable at open time as the recovered
        // durability fence. This does not prove the bytes were
        // fsync-confirmed before the previous process died; it means
        // they survived to this open and future appends must start
        // after them.
        let durable_lsn = replay.max_lsn;

        let oldest_segment_id = entries.first().map(|e| e.id).unwrap_or(active_id);

        let state = WalState {
            next_lsn,
            durable_lsn,
            active_segment_id: active_id,
            active_writer,
            oldest_segment_id,
        };

        let wal = Arc::new(Self {
            segments,
            sync_mode,
            segment_target_bytes,
            state: Mutex::new(state),
            bg_failure: Arc::new(Mutex::new(None)),
            #[cfg(not(target_arch = "wasm32"))]
            flusher: Mutex::new(None),
            _dir_lock: dir_lock,
        });

        // Spawn the Group flusher *after* the Arc exists so it can hold a
        // `Weak<Wal>` that drops when the last strong ref does. The flusher's
        // own Drop joins the thread, so removing the field on `Wal::drop` is
        // a clean shutdown signal. Wasm has no real fsync boundary and no
        // thread support, so Group there relies on the drop-time flush.
        #[cfg(not(target_arch = "wasm32"))]
        if let SyncMode::Group { interval_ms } = sync_mode {
            let interval = Duration::from_millis(u64::from(interval_ms.max(1)));
            let handle = spawn_group_flusher(Arc::downgrade(&wal), interval);
            *wal.flusher.lock().unwrap() = Some(handle);
        }

        Ok((wal, replay.committed_events))
    }

    /// Brand-new WAL directory. Create segment 1 with `base_lsn = 1`
    /// so LSN 0 stays reserved for "empty / never written".
    fn open_fresh(
        segments: &SegmentDir,
    ) -> Result<(SegmentId, SegmentWriter, ReplayOutcome), WalError> {
        let id = SegmentId::FIRST;
        let writer = SegmentWriter::create(segments.path_for(id), Lsn::new(1))?;
        segments.sync_dir()?;
        let replay = ReplayOutcome {
            committed_events: Vec::new(),
            max_lsn: Lsn::ZERO,
            torn_tail: None,
            checkpoint_lsn_observed: None,
        };
        Ok((id, writer, replay))
    }

    /// Existing directory. Replay every segment to surface committed
    /// events + detect a torn tail; reopen the highest-id segment
    /// for append; truncate it if the torn tail is in *that* segment.
    fn open_existing(
        segments: &SegmentDir,
        entries: &[crate::dir::SegmentEntry],
        checkpoint_lsn: Lsn,
    ) -> Result<(SegmentId, SegmentWriter, ReplayOutcome), WalError> {
        let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
        let replay = replay_segments(&paths, checkpoint_lsn)?;

        // The active segment is whichever file has the highest
        // numeric id — segment file names are self-describing, so
        // there is no separate CURRENT pointer.
        let active = entries.last().expect("entries non-empty in open_existing");
        let (mut writer, _torn_from_writer) =
            SegmentWriter::open_for_append(segments.path_for(active.id))?;

        // A torn tail in a *sealed* segment is impossible (sealed
        // segments are never appended to), so we only need to handle
        // the active one.
        if let Some(t) = &replay.torn_tail {
            if t.segment_path == active.path {
                writer.truncate_to(t.last_good_offset)?;
            } else {
                return Err(WalError::Malformed(format!(
                    "torn tail found in sealed segment {}",
                    t.segment_path.display()
                )));
            }
        }

        Ok((active.id, writer, replay))
    }

    pub fn dir(&self) -> &Path {
        self.segments.root()
    }

    pub fn sync_mode(&self) -> SyncMode {
        self.sync_mode
    }

    pub fn durable_lsn(&self) -> Lsn {
        self.state.lock().unwrap().durable_lsn
    }

    /// Latched durability failure, if any. `None` means the WAL is healthy.
    /// Once set, every commit / flush / force_fsync starts returning
    /// [`WalError::Poisoned`] and the WAL stops accepting new
    /// transactions until the operator restarts from the last
    /// consistent snapshot + WAL.
    pub fn bg_failure(&self) -> Option<String> {
        self.bg_failure.lock().unwrap().clone()
    }

    /// Direct handle to the latched-failure mutex. Used by the bg
    /// flusher to record an fsync failure exactly once. Hidden from
    /// outside the module so the latch stays single-writer.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn bg_failure_slot(&self) -> &BgFailure {
        &self.bg_failure
    }

    fn check_healthy(&self) -> Result<(), WalError> {
        if self.bg_failure.lock().unwrap().is_some() {
            return Err(WalError::Poisoned);
        }
        Ok(())
    }

    /// LSN that the *next* `begin` / `append` call will allocate.
    /// Exposed for tests and for sanity checks at boot; not part of
    /// any durability contract.
    pub fn next_lsn(&self) -> Lsn {
        self.state.lock().unwrap().next_lsn
    }

    pub fn oldest_segment_id(&self) -> u64 {
        self.state.lock().unwrap().oldest_segment_id.raw()
    }

    pub fn active_segment_id(&self) -> u64 {
        self.state.lock().unwrap().active_segment_id.raw()
    }

    // -------------------------------------------------------------
    // Low-level record primitives.
    //
    // Production code does **not** use these directly — every commit
    // goes through [`Self::commit_tx`], which writes the begin/batch/
    // commit triple atomically and routes durability through the
    // configured single-thread flush policy. The methods below remain
    // `pub` for the crate's own integration tests and for the rare
    // admin path (`checkpoint_marker`) that needs to insert a single record.
    // Mixing them with `commit_tx` against the same WAL is supported
    // but unnecessary; if you find yourself calling `begin` /
    // `append` / `commit` from a new caller, prefer `commit_tx`
    // unless you specifically need the partial-write shape.
    // -------------------------------------------------------------

    /// Allocate a `TxBegin` record and return its LSN. *Test/admin
    /// primitive.* Production commits use [`Self::commit_tx`].
    ///
    /// Rotation happens here so a transaction is always wholly within
    /// one segment.
    pub fn begin(&self) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        self.maybe_rotate(&mut state)?;
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxBegin { lsn })
    }

    /// Append a single mutation to the active segment's pending
    /// buffer. *Test/admin primitive.* Not durable until `flush()`
    /// runs; production commits use [`Self::commit_tx`].
    pub fn append(&self, tx_begin_lsn: Lsn, event: &MutationEvent) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::Mutation {
            lsn,
            tx_begin_lsn,
            event: event.clone(),
        })
    }

    /// Append many mutations as one framed record. *Test/admin
    /// primitive.* Production commits use [`Self::commit_tx`], which
    /// writes the begin/batch/commit triple in a single critical
    /// section.
    pub fn append_batch(
        &self,
        tx_begin_lsn: Lsn,
        events: Vec<MutationEvent>,
    ) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        if events.is_empty() {
            return Err(WalError::Encode(
                "mutation batch must contain at least one event".into(),
            ));
        }
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::MutationBatch {
            lsn,
            tx_begin_lsn,
            events,
        })
    }

    /// Append a standalone `TxCommit` marker. *Test/admin primitive.*
    /// Production commits use [`Self::commit_tx`].
    pub fn commit(&self, tx_begin_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxCommit { lsn, tx_begin_lsn })
    }

    /// Append a `TxAbort` marker. *Test/admin primitive.* Production
    /// code never writes `TxAbort`: [`Self::commit_tx`] writes the
    /// begin/batch/commit triple atomically, so an aborted query has
    /// nothing on disk to mark as aborted.
    pub fn abort(&self, tx_begin_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxAbort { lsn, tx_begin_lsn })
    }

    /// One-shot transaction commit.
    ///
    /// Encodes `TxBegin` + `MutationBatch` + `TxCommit` as a single
    /// contiguous run inside one short critical section, then applies the
    /// configured flush policy. Compared to the legacy
    /// `begin → append_batch → commit → flush` sequence this collapses
    /// four separate state-lock acquisitions into one while preserving the
    /// release's single-writer execution model. Future concurrent commit
    /// plumbing can build around this one-shot boundary without changing the
    /// recorder contract.
    ///
    /// Returns [`WroteCommit::No`] for an empty event list (no records
    /// are written, no fsync is issued).
    pub fn commit_tx(&self, events: Vec<MutationEvent>) -> Result<WroteCommit, WalError> {
        self.check_healthy()?;
        if events.is_empty() {
            return Ok(WroteCommit::No);
        }

        // Phase 1: allocate the LSN window and encode all three
        // records into the active segment's pending buffer in one
        // critical section. Collapsing what was four separate state
        // lock acquisitions (begin / append_batch / commit / flush)
        // into one is the lock-side win that pairs with the
        // lock-free emit short-circuit on the recorder side.
        {
            let mut state = self.state.lock().unwrap();
            self.maybe_rotate(&mut state)?;
            let begin_lsn = state.next_lsn;
            let batch_lsn = begin_lsn.next();
            let commit_lsn = batch_lsn.next();
            state.next_lsn = commit_lsn.next();
            state
                .active_writer
                .append(&WalRecord::TxBegin { lsn: begin_lsn })?;
            state.active_writer.append(&WalRecord::MutationBatch {
                lsn: batch_lsn,
                tx_begin_lsn: begin_lsn,
                events,
            })?;
            state.active_writer.append(&WalRecord::TxCommit {
                lsn: commit_lsn,
                tx_begin_lsn: begin_lsn,
            })?;
        }

        // Phase 2: durability per sync mode. PerCommit fsyncs inline;
        // Group is cooperative in this single-threaded release (no bg
        // flusher thread); None just pushes bytes to the page cache.
        match self.sync_mode {
            SyncMode::PerCommit => self.flush_inner(FlushKind::ForceFsync)?,
            SyncMode::Group { .. } | SyncMode::None => {
                self.flush_inner(FlushKind::PerConfiguredMode)?;
            }
        }

        Ok(WroteCommit::Yes)
    }

    /// Append a `Checkpoint` marker. `snapshot_lsn` should equal the
    /// LSN written into the snapshot file's header — replay uses
    /// it to defend against the snapshot-rename-but-no-marker race.
    pub fn checkpoint_marker(&self, snapshot_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::Checkpoint {
            lsn,
            snapshot_lsn,
        })
    }

    /// Single-source-of-truth for "allocate the next LSN, build the
    /// record, push it onto the active segment's pending buffer".
    /// The five public append paths (`begin / append / commit / abort
    /// / checkpoint_marker`) all funnel through here so the LSN
    /// allocation never gets out of sync with the encoded record.
    #[inline]
    fn alloc_and_append(
        state: &mut WalState,
        build: impl FnOnce(Lsn) -> WalRecord,
    ) -> Result<Lsn, WalError> {
        let lsn = state.next_lsn;
        state.next_lsn = lsn.next();
        state.active_writer.append(&build(lsn))?;
        Ok(lsn)
    }

    /// Flush the active segment's pending buffer.
    ///
    /// What "flush" means depends on [`SyncMode`]:
    ///
    /// - `PerCommit` — write the buffer to the OS, `fsync`, and
    ///   advance `durable_lsn`. The strongest contract: every
    ///   record up to `next_lsn - 1` is on disk.
    /// - `Group` — write the buffer to the OS, but leave
    ///   `durable_lsn` unchanged until an explicit `force_fsync`,
    ///   checkpoint, sync, or clean drop.
    /// - `None` — write the buffer to the OS only, but advance
    ///   `durable_lsn` anyway. The mode opts out of crash
    ///   durability, so the checkpoint fence reports
    ///   "what's been written" instead of "what's actually safe".
    pub fn flush(&self) -> Result<(), WalError> {
        self.check_healthy()?;
        self.flush_inner(FlushKind::PerConfiguredMode)
    }

    /// Unconditionally write the buffer to the OS, `fsync`, and
    /// advance `durable_lsn`. Used by callers that need a durability
    /// point right now regardless of the configured cadence (e.g.
    /// checkpoint). Returns [`WalError::Poisoned`] if the WAL has already
    /// latched a durability failure.
    pub fn force_fsync(&self) -> Result<(), WalError> {
        self.check_healthy()?;
        self.flush_inner(FlushKind::ForceFsync)
    }

    /// Single source of truth for the flush state machine. Skips the
    /// `check_healthy` gate so clean shutdown can force a final Group-mode
    /// sync even if callers are otherwise done with the handle.
    pub(super) fn flush_inner(&self, kind: FlushKind) -> Result<(), WalError> {
        let mut state = self.state.lock().unwrap();
        let written_lsn = Lsn::new(state.next_lsn.raw().saturating_sub(1));

        // Decide whether this call is allowed to advance `durable_lsn`.
        // PerCommit and forced syncs advance after the fsync boundary. None
        // advances after write because the mode opts out of crash durability.
        // Group is cooperative for this release: normal `flush()` writes bytes
        // but leaves the durable fence alone until `force_fsync`, checkpoint,
        // sync(), or drop.
        let do_fsync = matches!(
            (kind, self.sync_mode),
            (FlushKind::ForceFsync, _) | (_, SyncMode::PerCommit)
        );
        let advance_durable = matches!(
            (kind, self.sync_mode),
            (FlushKind::ForceFsync, _) | (_, SyncMode::PerCommit) | (_, SyncMode::None)
        );

        if do_fsync {
            state.active_writer.flush_and_sync()?;
        } else {
            state.active_writer.flush_buffer()?;
        }
        if advance_durable {
            state.durable_lsn = written_lsn;
        }
        Ok(())
    }

    /// Drop sealed segments whose entire LSN range is at or below
    /// `fence_lsn`. Idempotent and safe to call repeatedly.
    ///
    /// The active segment is never deleted — even if every record in
    /// it predates the fence, it is still the rotation target for
    /// new appends. The segment immediately before the active one
    /// is also kept as a tombstone so a subsequent crash before the
    /// next checkpoint still finds a self-describing log start.
    pub fn truncate_up_to(&self, fence_lsn: Lsn) -> Result<(), WalError> {
        let mut state = self.state.lock().unwrap();
        let active_id = state.active_segment_id;
        let entries = self.segments.list()?;

        let mut to_drop: Vec<crate::dir::SegmentEntry> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            // Active segment and the one immediately preceding it
            // are kept by policy.
            if entry.id >= active_id.saturating_prev() {
                break;
            }
            // Segment `i` covers `[base_i, base_{i+1} - 1]`. We are
            // safe to drop only when `base_{i+1} - 1 <= fence_lsn`.
            let next = match entries.get(i + 1) {
                Some(n) => n,
                None => break,
            };
            let next_base = SegmentDir::base_lsn(&next.path)?;
            if next_base.raw().saturating_sub(1) <= fence_lsn.raw() {
                to_drop.push(entry.clone());
            }
        }

        for entry in to_drop {
            fs::remove_file(&entry.path)?;
            if entry.id >= state.oldest_segment_id {
                state.oldest_segment_id = entry.id.next();
            }
        }
        if state.oldest_segment_id != entries.first().map(|e| e.id).unwrap_or(active_id) {
            self.segments.sync_dir()?;
        }
        Ok(())
    }

    /// Rotate the active segment when it has grown past
    /// `segment_target_bytes`. Called from `begin()` so rotation only
    /// ever lands at a transaction boundary.
    fn maybe_rotate(&self, state: &mut WalState) -> Result<(), WalError> {
        if state.active_writer.bytes_written() < self.segment_target_bytes {
            return Ok(());
        }
        // Seal the current segment (forces a flush + fsync) and open
        // a fresh one with `base_lsn = next_lsn` so the segment file
        // names line up with the record LSNs they contain.
        state.active_writer.flush_and_sync()?;
        state.active_writer.seal()?;

        let next_id = state.active_segment_id.next();
        let writer = SegmentWriter::create(self.segments.path_for(next_id), state.next_lsn)?;
        self.segments.sync_dir()?;
        state.active_writer = writer;
        state.active_segment_id = next_id;
        Ok(())
    }
}

impl Drop for Wal {
    fn drop(&mut self) {
        if matches!(self.sync_mode, SyncMode::Group { .. }) {
            let _ = self.flush_inner(FlushKind::ForceFsync);
        }
        // Join the group flusher, if any, before the directory lock is
        // released. That keeps the "one live append owner" boundary intact
        // through shutdown.
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(slot) = self.flusher.get_mut() {
            let _ = slot.take();
        }
    }
}
