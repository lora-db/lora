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

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use lora_store::MutationEvent;

#[cfg(not(target_arch = "wasm32"))]
use super::group_flusher::{spawn_group_flusher, GroupFlusherHandle};
use crate::config::SyncMode;
use crate::dir::{SegmentDir, SegmentId};
use crate::errors::WalError;
use crate::io::sync_file;
use crate::lock::DirLock;
use crate::lsn::Lsn;
use crate::record::{encode_frame, framed_len, RecordKind, WalRecord};
use crate::recorder::WroteCommit;
use crate::replay::{replay_segments, CollectEvents, CorruptCut, ReplaySink, TornTailInfo};
use crate::segment::{SegmentReader, SegmentWriter, SEGMENT_HEADER_LEN};
use crate::{codec, record::MAX_RECORD_LEN};

/// Target size of one `MutationBatch` record. A transaction whose events
/// encode to more than this is written as several batch records between
/// its `TxBegin` and `TxCommit`, so no transaction is too large for one
/// record and the pending buffer holds about one batch at a time.
const BATCH_TARGET_BYTES: usize = 8 * 1024 * 1024;

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

/// Called with the new durable LSN each time an `fsync` advances it, on
/// the thread that ran the `fsync` (the GroupSync flusher, or a caller of
/// `force_fsync`, a checkpoint or drop). No WAL lock is held during the
/// call, but it must be quick and must not call back into the WAL.
pub type DurableListener = Arc<dyn Fn(Lsn) + Send + Sync>;

/// Called after a commit once [`Wal::bytes_since_checkpoint`] reaches the
/// threshold given to [`Wal::set_growth_listener`], on the committing
/// thread, which may hold the host's writer lock. It must return quickly
/// (hand the checkpoint to another thread) and must not call back into the
/// WAL's commit path.
pub type GrowthListener = Arc<dyn Fn() + Send + Sync>;

/// Selects the durability work that [`Wal::flush_inner`] actually does.
/// Centralising the normal write-only path and the forced fsync path keeps
/// the call sites from duplicating durable-LSN rules.
#[derive(Debug, Clone, Copy)]
pub(super) enum FlushKind {
    /// Write pending WAL bytes to the OS without forcing storage durability.
    /// This is what commits do under [`SyncMode::GroupSync`].
    PerConfiguredMode,
    /// Write pending WAL bytes, fsync, and advance `durable_lsn`. Used by
    /// checkpoints, explicit sync, the background flusher, and clean drop.
    ForceFsync,
}

/// Live, append-side WAL handle.
///
/// Construct via [`Wal::open`]. The returned tuple includes the list of
/// committed mutation events that need to be re-applied to the
/// in-memory store before any new traffic is accepted.
///
/// `Wal::open` returns `Arc<Self>` because the optional GroupSync
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
    /// Background flusher for `SyncMode::GroupSync`. `Drop` joins the
    /// thread, so a `Wal` going out of scope is a clean shutdown
    /// signal. Absent on `wasm32`, where GroupSync falls back to the
    /// drop-time flush.
    #[cfg(not(target_arch = "wasm32"))]
    flusher: Mutex<Option<GroupFlusherHandle>>,
    /// See [`BATCH_TARGET_BYTES`]; a field so tests can shrink it.
    batch_target_bytes: AtomicUsize,
    /// Number of `fsync`s issued by [`Self::flush_inner`]. Diagnostic.
    fsyncs: AtomicU64,
    /// See [`Self::set_durable_listener`].
    durable_listener: Mutex<Option<DurableListener>>,
    /// Bytes of transactions appended since open. With `checkpoint_mark`
    /// it gives [`Self::bytes_since_checkpoint`].
    appended_bytes: AtomicU64,
    /// `appended_bytes` at the last [`Self::mark_checkpoint`].
    checkpoint_mark: AtomicU64,
    /// See [`Self::set_growth_listener`]; 0 when no listener is set.
    growth_threshold: AtomicU64,
    growth_listener: Mutex<Option<GrowthListener>>,
    /// Test hook: while set, `fsync`s are skipped and `durable_lsn` stays
    /// put, as if the disk had not confirmed anything since.
    fsync_held: AtomicBool,
    /// Where this open cut a corrupt log, when it was opened with
    /// `truncate_corrupt_tail` and needed to.
    corrupt_cut: Option<CorruptCut>,
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
    ///
    /// This holds every replayed event at once; a long log should be
    /// opened with [`Self::open_replaying`].
    pub fn open(
        dir: impl Into<std::path::PathBuf>,
        sync_mode: SyncMode,
        segment_target_bytes: u64,
        checkpoint_lsn: Lsn,
    ) -> Result<(Arc<Self>, Vec<MutationEvent>), WalError> {
        let mut collected = CollectEvents::default();
        let wal = Self::open_replaying(
            dir,
            sync_mode,
            segment_target_bytes,
            checkpoint_lsn,
            &mut |step| collected.step(step),
        )?;
        Ok((wal, collected.events))
    }

    /// [`Self::open`], handing each committed transaction's events to
    /// `apply` as replay reads its commit (see [`crate::ReplayStep`]), so
    /// memory holds at most [`crate::PENDING_BUDGET_BYTES`] of an open
    /// transaction rather than the whole log. An error from `apply` stops
    /// the open and is returned.
    pub fn open_replaying(
        dir: impl Into<std::path::PathBuf>,
        sync_mode: SyncMode,
        segment_target_bytes: u64,
        checkpoint_lsn: Lsn,
        apply: &mut ReplaySink<'_>,
    ) -> Result<Arc<Self>, WalError> {
        Self::open_replaying_with(
            dir,
            sync_mode,
            segment_target_bytes,
            checkpoint_lsn,
            false,
            apply,
        )
    }

    /// [`Self::open_replaying`], and with `truncate_corrupt_tail` an
    /// incident-recovery open of a log that fails with
    /// [`WalError::Corrupt`]: instead of failing, the log is cut at the
    /// first corrupt point and opens with the transactions committed
    /// before it. The corrupt segment's bytes from that point are copied
    /// to `<segment>.corrupt-<offset>` before it is truncated, and every
    /// later segment is renamed to `<segment>.after-corrupt-<id>-<offset>`
    /// (nothing is deleted). Corruption in an older, sealed segment so
    /// loses every later segment's transactions. The cut is logged as a
    /// warning and reported by [`Self::corrupt_cut`]. Other open errors
    /// ([`WalError::Gap`], `Malformed`, I/O) still fail.
    pub fn open_replaying_with(
        dir: impl Into<std::path::PathBuf>,
        sync_mode: SyncMode,
        segment_target_bytes: u64,
        checkpoint_lsn: Lsn,
        truncate_corrupt_tail: bool,
        apply: &mut ReplaySink<'_>,
    ) -> Result<Arc<Self>, WalError> {
        let segments = SegmentDir::new(dir);
        fs::create_dir_all(segments.root())?;
        let dir_lock = DirLock::acquire(segments.root())?;

        let mut entries = segments.list()?;
        Self::discard_unborn_segment(&segments, &mut entries)?;
        let (active_id, active_writer, next_lsn, corrupt_cut) = if entries.is_empty() {
            let (id, writer, next_lsn) = Self::open_fresh(&segments, checkpoint_lsn)?;
            (id, writer, next_lsn, None)
        } else {
            Self::open_existing(
                &segments,
                &entries,
                checkpoint_lsn,
                truncate_corrupt_tail,
                apply,
            )?
        };

        // Treat everything readable at open time, and everything the
        // snapshot covers, as the recovered durability fence. This does
        // not prove the bytes were fsync-confirmed before the previous
        // process died; it means they survived to this open and future
        // appends must start after them.
        let durable_lsn = Lsn::new(next_lsn.raw() - 1);

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
            batch_target_bytes: AtomicUsize::new(BATCH_TARGET_BYTES),
            fsyncs: AtomicU64::new(0),
            durable_listener: Mutex::new(None),
            appended_bytes: AtomicU64::new(0),
            checkpoint_mark: AtomicU64::new(0),
            growth_threshold: AtomicU64::new(0),
            growth_listener: Mutex::new(None),
            fsync_held: AtomicBool::new(false),
            corrupt_cut,
            _dir_lock: dir_lock,
        });

        // Spawn the GroupSync flusher *after* the Arc exists so it can hold a
        // `Weak<Wal>` that drops when the last strong ref does. The flusher's
        // own Drop joins the thread, so removing the field on `Wal::drop` is
        // a clean shutdown signal. Wasm has no real fsync boundary and no
        // thread support, so GroupSync there relies on the drop-time flush.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let SyncMode::GroupSync { interval_ms } = sync_mode;
            let interval = Duration::from_millis(u64::from(interval_ms.max(1)));
            let handle = spawn_group_flusher(Arc::downgrade(&wal), interval);
            *wal.flusher.lock().map_err(|_| WalError::Poisoned)? = Some(handle);
        }

        Ok(wal)
    }

    /// Brand-new WAL directory. Create segment 1 with `base_lsn` just
    /// above the snapshot fence (1 without a snapshot, so LSN 0 stays
    /// reserved for "empty / never written"). Starting above the fence
    /// keeps new records from reusing LSNs the snapshot already covers,
    /// which replay would skip.
    fn open_fresh(
        segments: &SegmentDir,
        checkpoint_lsn: Lsn,
    ) -> Result<(SegmentId, SegmentWriter, Lsn), WalError> {
        let id = SegmentId::FIRST;
        let base = next_lsn_after(checkpoint_lsn)?;
        let writer = SegmentWriter::create(segments.path_for(id), base)?;
        segments.sync_dir()?;
        Ok((id, writer, base))
    }

    /// A crash inside [`SegmentWriter::create`] can leave the newest
    /// segment without a durable header: empty, shorter than a header, or
    /// a header-sized run of bytes that does not decode. Such a file never
    /// held a record, so remove it and open from the segment before it.
    fn discard_unborn_segment(
        segments: &SegmentDir,
        entries: &mut Vec<crate::dir::SegmentEntry>,
    ) -> Result<(), WalError> {
        let Some(newest) = entries.last() else {
            return Ok(());
        };
        if fs::metadata(&newest.path)?.len() > SEGMENT_HEADER_LEN as u64 {
            return Ok(());
        }
        match SegmentReader::open(&newest.path) {
            Ok(_) => Ok(()),
            Err(WalError::Io(err)) if err.kind() != std::io::ErrorKind::UnexpectedEof => {
                Err(WalError::Io(err))
            }
            Err(_) => {
                fs::remove_file(&newest.path)?;
                segments.sync_dir()?;
                entries.pop();
                Ok(())
            }
        }
    }

    /// Existing directory. Replay every segment to surface committed
    /// events + detect a torn tail; reopen the highest-id segment
    /// for append; truncate it if the torn tail is in *that* segment.
    /// Returns the active segment, its writer, and the next LSN to hand
    /// out.
    fn open_existing(
        segments: &SegmentDir,
        entries: &[crate::dir::SegmentEntry],
        checkpoint_lsn: Lsn,
        truncate_corrupt_tail: bool,
        apply: &mut ReplaySink<'_>,
    ) -> Result<(SegmentId, SegmentWriter, Lsn, Option<CorruptCut>), WalError> {
        let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
        let mut replay = replay_segments(&paths, checkpoint_lsn, truncate_corrupt_tail, apply)?;
        // Never hand out an LSN at or below the fence: a snapshot can be
        // ahead of a log whose tail was lost, and replay skips everything
        // the fence covers.
        let next_lsn = next_lsn_after(replay.max_lsn.max(checkpoint_lsn))?;

        let mut entries = entries;
        if let Some(cut) = &mut replay.corrupt_cut {
            let index = entries
                .iter()
                .position(|e| e.path == cut.segment_path)
                .ok_or_else(|| {
                    WalError::Malformed(format!(
                        "corrupt segment {} is not in the WAL directory listing",
                        cut.segment_path.display()
                    ))
                })?;
            cut_corrupt_log(segments, &entries[index..], cut)?;
            entries = &entries[..=index];
        }
        let corrupt_cut = replay.corrupt_cut.take();

        // The active segment is whichever file has the highest
        // numeric id — segment file names are self-describing, so
        // there is no separate CURRENT pointer.
        let active = entries
            .last()
            .ok_or_else(|| WalError::Malformed("WAL directory has no segments".into()))?;

        if SegmentReader::open(&active.path)?.header().sealed {
            // The previous process sealed this segment and died before
            // its successor existed. Finish the rotation.
            let next_id = active
                .id
                .checked_next()
                .ok_or_else(|| WalError::Malformed("WAL segment id space is exhausted".into()))?;
            let writer = SegmentWriter::create(segments.path_for(next_id), next_lsn)?;
            segments.sync_dir()?;
            return Ok((next_id, writer, next_lsn, corrupt_cut));
        }

        let mut writer = SegmentWriter::open_for_append_at(
            segments.path_for(active.id),
            replay.last_good_offset,
        )?;

        // Replay reports a torn tail only for the newest, unsealed
        // segment; anything else already failed as corruption.
        if let Some(t) = &replay.torn_tail {
            if t.segment_path != active.path {
                return Err(WalError::Malformed(format!(
                    "torn tail found in sealed segment {}",
                    t.segment_path.display()
                )));
            }
            preserve_torn_tail(segments, t)?;
            writer.truncate_to(t.last_good_offset)?;
        }

        Ok((active.id, writer, next_lsn, corrupt_cut))
    }

    /// Where this open cut a corrupt log, when it was opened with
    /// `truncate_corrupt_tail` (see [`Self::open_replaying_with`]) and
    /// found corruption. `None` for a log that opened intact.
    pub fn corrupt_cut(&self) -> Option<&CorruptCut> {
        self.corrupt_cut.as_ref()
    }

    pub fn dir(&self) -> &Path {
        self.segments.root()
    }

    pub fn sync_mode(&self) -> SyncMode {
        self.sync_mode
    }

    pub fn durable_lsn(&self) -> Lsn {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .durable_lsn
    }

    /// Latched durability failure, if any. `None` means the WAL is healthy.
    /// Once set, every commit / flush / force_fsync starts returning
    /// [`WalError::Poisoned`] and the WAL stops accepting new
    /// transactions until the operator restarts from the last
    /// consistent snapshot + WAL.
    pub fn bg_failure(&self) -> Option<String> {
        self.bg_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Direct handle to the latched-failure mutex. Used by the bg
    /// flusher to record an fsync failure exactly once. Hidden from
    /// outside the module so the latch stays single-writer.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn bg_failure_slot(&self) -> &BgFailure {
        &self.bg_failure
    }

    fn check_healthy(&self) -> Result<(), WalError> {
        if self
            .bg_failure
            .lock()
            .map_err(|_| WalError::Poisoned)?
            .is_some()
        {
            return Err(WalError::Poisoned);
        }
        Ok(())
    }

    /// LSN that the *next* `begin` / `append` call will allocate.
    /// Exposed for tests and for sanity checks at boot; not part of
    /// any durability contract.
    pub fn next_lsn(&self) -> Lsn {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .next_lsn
    }

    pub fn oldest_segment_id(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .oldest_segment_id
            .raw()
    }

    pub fn active_segment_id(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active_segment_id
            .raw()
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
        let mut state = self.lock_state()?;
        self.maybe_rotate(&mut state)?;
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxBegin { lsn })
    }

    /// Append a single mutation to the active segment's pending
    /// buffer. *Test/admin primitive.* Not durable until `flush()`
    /// runs; production commits use [`Self::commit_tx`].
    pub fn append(&self, tx_begin_lsn: Lsn, event: &MutationEvent) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.lock_state()?;
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
        let mut state = self.lock_state()?;
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
        let mut state = self.lock_state()?;
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxCommit { lsn, tx_begin_lsn })
    }

    /// Append a `TxAbort` marker. *Test/admin primitive.* Production
    /// code never writes `TxAbort`: [`Self::commit_tx`] writes the
    /// begin/batch/commit triple atomically, so an aborted query has
    /// nothing on disk to mark as aborted.
    pub fn abort(&self, tx_begin_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.lock_state()?;
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
        Ok(match self.commit_tx_lsn(events)? {
            Some(_) => WroteCommit::Yes,
            None => WroteCommit::No,
        })
    }

    /// Same as [`Self::commit_tx`], but returns the LSN of the `TxCommit`
    /// record it wrote (`None` when `events` was empty and nothing was
    /// written). Change feeds use it as the transaction's resume token.
    pub fn commit_tx_lsn(&self, events: Vec<MutationEvent>) -> Result<Option<Lsn>, WalError> {
        self.commit_tx_events(&events)
    }

    /// [`Self::commit_tx_lsn`] for borrowed events, so a caller that
    /// keeps the events (a change feed) does not have to clone them.
    ///
    /// Events that encode to more than one batch record's worth are
    /// written as several `MutationBatch` records inside the one
    /// transaction. A value the encoder rejects fails the call before
    /// any byte is buffered or any LSN is used, and leaves the WAL
    /// usable.
    pub fn commit_tx_events(&self, events: &[MutationEvent]) -> Result<Option<Lsn>, WalError> {
        self.check_healthy()?;
        if events.is_empty() {
            return Ok(None);
        }

        // Size every event before taking the lock. This is also where an
        // unencodable event fails, so nothing below can hit an encode
        // error after bytes reached the file.
        let batches = plan_batches(events, self.batch_target_bytes.load(Ordering::Relaxed))?;

        // Phase 1: allocate the LSN window and encode the records into
        // the active segment's pending buffer in one critical section,
        // instead of separate lock acquisitions per record.
        let commit_lsn = {
            let mut state = self.lock_state()?;
            self.maybe_rotate(&mut state)?;
            let first_lsn = state.next_lsn;
            let pending_mark = state.active_writer.pending_len();
            let file_mark = state.active_writer.flushed_len();
            let start_bytes = state.active_writer.bytes_written();
            let result = Self::append_tx(&mut state, events, &batches);
            if result.is_ok() {
                let appended = state.active_writer.bytes_written() - start_bytes;
                self.appended_bytes.fetch_add(appended, Ordering::Relaxed);
            } else {
                if state.active_writer.flushed_len() == file_mark {
                    // Nothing of this transaction reached the file:
                    // forget it entirely, LSNs included.
                    state.active_writer.rollback_pending(pending_mark);
                    state.next_lsn = first_lsn;
                } else {
                    // Its first batches are on disk without a commit,
                    // which replay discards. Drop the rest; the LSNs
                    // stay used.
                    state.active_writer.rollback_pending(0);
                }
            }
            result?
        };

        // Phase 2: make commit bytes visible to the OS page cache. Storage
        // durability is provided by the GroupSync flusher or an explicit
        // force_fsync/checkpoint/sync/drop boundary.
        self.flush_inner(FlushKind::PerConfiguredMode)?;

        let threshold = self.growth_threshold.load(Ordering::Relaxed);
        if threshold != 0 && self.bytes_since_checkpoint() >= threshold {
            self.notify_growth();
        }

        Ok(Some(commit_lsn))
    }

    /// Append `TxBegin`, one `MutationBatch` per entry of `batches`, and
    /// `TxCommit`. A transaction larger than one batch is written to the
    /// file batch by batch so the pending buffer stays bounded; after the
    /// first such write a failure leaves an uncommitted transaction on
    /// disk, which replay discards.
    fn append_tx(
        state: &mut WalState,
        events: &[MutationEvent],
        batches: &BatchPlan,
    ) -> Result<Lsn, WalError> {
        let begin_lsn = Self::alloc_lsn(state)?;
        state
            .active_writer
            .append(&WalRecord::TxBegin { lsn: begin_lsn })?;
        let mut start = 0;
        for (index, &(end, payload_len)) in batches.iter().enumerate() {
            if index > 0 {
                state.active_writer.flush_buffer()?;
            }
            let chunk = &events[start..end];
            let lsn = Self::alloc_lsn(state)?;
            encode_frame(
                state.active_writer.pending_mut()?,
                RecordKind::MutationBatch,
                lsn,
                begin_lsn,
                payload_len,
                |out| codec::encode_events_into(out, chunk),
            )?;
            start = end;
        }
        let commit_lsn = Self::alloc_lsn(state)?;
        state.active_writer.append(&WalRecord::TxCommit {
            lsn: commit_lsn,
            tx_begin_lsn: begin_lsn,
        })?;
        Ok(commit_lsn)
    }

    fn alloc_lsn(state: &mut WalState) -> Result<Lsn, WalError> {
        let lsn = state.next_lsn;
        state.next_lsn = lsn
            .checked_next()
            .ok_or_else(|| WalError::Malformed("WAL LSN space is exhausted".into()))?;
        Ok(lsn)
    }

    #[cfg(test)]
    pub(crate) fn set_batch_target_bytes_for_tests(&self, bytes: usize) {
        self.batch_target_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Bytes of transactions appended since the last checkpoint
    /// ([`Self::mark_checkpoint`], which [`Self::truncate_up_to`] calls) or
    /// since open. Hosts use it to checkpoint by WAL growth.
    pub fn bytes_since_checkpoint(&self) -> u64 {
        self.appended_bytes
            .load(Ordering::Relaxed)
            .saturating_sub(self.checkpoint_mark.load(Ordering::Relaxed))
    }

    /// Start counting [`Self::bytes_since_checkpoint`] from zero again.
    pub fn mark_checkpoint(&self) {
        self.checkpoint_mark.store(
            self.appended_bytes.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    /// Install (or with `None` remove) the callback told after each commit
    /// that leaves [`Self::bytes_since_checkpoint`] at or above `threshold`
    /// bytes. A host uses it to checkpoint by WAL growth. It keeps firing
    /// on every commit until a checkpoint restarts the count or the
    /// listener is removed. A `threshold` of 0 removes the listener.
    pub fn set_growth_listener(&self, threshold: u64, listener: Option<GrowthListener>) {
        let mut slot = self
            .growth_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let threshold = if listener.is_some() { threshold } else { 0 };
        *slot = listener.filter(|_| threshold != 0);
        self.growth_threshold.store(threshold, Ordering::Relaxed);
    }

    /// Change the threshold of the listener set with
    /// [`Self::set_growth_listener`], keeping the listener. Ignored (and
    /// 0 is not allowed to remove it) while no listener is set.
    pub fn set_growth_threshold(&self, threshold: u64) {
        let slot = self
            .growth_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() && threshold != 0 {
            self.growth_threshold.store(threshold, Ordering::Relaxed);
        }
    }

    fn notify_growth(&self) {
        let listener = self
            .growth_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener();
        }
    }

    /// Install (or with `None` remove) the callback told about each advance
    /// of the durable LSN. A change feed uses it to hand out commits only
    /// once they are durable.
    pub fn set_durable_listener(&self, listener: Option<DurableListener>) {
        *self
            .durable_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = listener;
    }

    /// Ask the GroupSync flusher to `fsync` now instead of at the end of
    /// its interval. Returns at once; the listener set with
    /// [`Self::set_durable_listener`] hears when the `fsync` is done.
    pub fn request_fsync(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(flusher) = self
            .flusher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            flusher.request_fsync();
        }
    }

    /// Test hook for crash simulations: while `held`, every `fsync` is
    /// skipped and the durable LSN does not move, so the bytes written
    /// meanwhile are the ones a power loss could take. Not for production
    /// use.
    #[doc(hidden)]
    pub fn hold_fsync_for_tests(&self, held: bool) {
        self.fsync_held.store(held, Ordering::SeqCst);
    }

    /// Number of `fsync`s the WAL has issued for its log since open.
    /// Diagnostic; an idle WAL does not grow it.
    pub fn fsync_count(&self) -> u64 {
        self.fsyncs.load(Ordering::Relaxed)
    }

    /// Append a `Checkpoint` marker. `snapshot_lsn` should equal the
    /// LSN written into the snapshot file's header — replay uses
    /// it to defend against the snapshot-rename-but-no-marker race.
    pub fn checkpoint_marker(&self, snapshot_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.lock_state()?;
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
        let next_lsn = lsn
            .checked_next()
            .ok_or_else(|| WalError::Malformed("WAL LSN space is exhausted".into()))?;
        state.active_writer.append(&build(lsn))?;
        state.next_lsn = next_lsn;
        Ok(lsn)
    }

    /// Flush the active segment's pending buffer.
    ///
    /// Under [`SyncMode::GroupSync`], a normal flush writes bytes to the OS
    /// but leaves `durable_lsn` unchanged until an explicit `force_fsync`,
    /// checkpoint, sync, the background flusher, or clean drop.
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
    /// `check_healthy` gate so clean shutdown can force a final GroupSync
    /// sync even if callers are otherwise done with the handle.
    ///
    /// The `fsync` itself runs after the state lock is released, so
    /// commits keep appending while the disk catches up. Only the LSN
    /// written before the `fsync` started becomes durable, and a WAL with
    /// nothing new since the last `fsync` skips it.
    pub(super) fn flush_inner(&self, kind: FlushKind) -> Result<(), WalError> {
        let (file, written_lsn) = {
            let mut state = self.lock_state()?;
            state.active_writer.flush_buffer()?;
            if matches!(kind, FlushKind::PerConfiguredMode) {
                return Ok(());
            }
            let written_lsn = Lsn::new(state.next_lsn.raw().saturating_sub(1));
            if state.durable_lsn >= written_lsn || self.fsync_held.load(Ordering::SeqCst) {
                return Ok(());
            }
            // If the segment rotates before the `fsync` below runs, the
            // rotation syncs it too (`seal`), and this handle keeps the
            // file open either way.
            (state.active_writer.sync_handle(), written_lsn)
        };
        sync_file(&file)?;
        self.fsyncs.fetch_add(1, Ordering::Relaxed);
        let durable = {
            let mut state = self.lock_state()?;
            if written_lsn <= state.durable_lsn {
                return Ok(());
            }
            state.durable_lsn = written_lsn;
            written_lsn
        };
        let listener = self
            .durable_listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(listener) = listener {
            listener(durable);
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
    ///
    /// Also restarts [`Self::bytes_since_checkpoint`]: every checkpoint
    /// ends here.
    pub fn truncate_up_to(&self, fence_lsn: Lsn) -> Result<(), WalError> {
        self.mark_checkpoint();
        let mut state = self.lock_state()?;
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
                state.oldest_segment_id = entry.id.checked_next().ok_or_else(|| {
                    WalError::Malformed("WAL segment id space is exhausted".into())
                })?;
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
        // Order matters for crash safety. Make the current segment
        // durable first, so it can never end in a torn tail once a newer
        // segment exists. Then create the next segment with `base_lsn =
        // next_lsn` (so file names line up with the LSNs they contain)
        // and make its directory entry durable. Seal the old segment
        // last: a crash before the seal leaves an unsealed but complete
        // segment, which replay reads like any other, and a crash inside
        // `create` leaves a headerless newest file that open removes.
        state.active_writer.flush_and_sync()?;
        let next_id = state
            .active_segment_id
            .checked_next()
            .ok_or_else(|| WalError::Malformed("WAL segment id space is exhausted".into()))?;
        let next_path = self.segments.path_for(next_id);
        let writer = SegmentWriter::create(next_path.clone(), state.next_lsn)?;
        let sealed = self
            .segments
            .sync_dir()
            .and_then(|()| state.active_writer.seal());
        if let Err(err) = sealed {
            // Keep appending to the old segment; drop the new one so the
            // next rotation can create it again.
            drop(writer);
            let _ = fs::remove_file(&next_path);
            return Err(err);
        }
        state.active_writer = writer;
        state.active_segment_id = next_id;
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, WalState>, WalError> {
        self.state.lock().map_err(|_| WalError::Poisoned)
    }
}

/// LSN that follows `lsn`, the first one a WAL fenced at `lsn` may use.
fn next_lsn_after(lsn: Lsn) -> Result<Lsn, WalError> {
    lsn.checked_next()
        .ok_or_else(|| WalError::Malformed("WAL LSN space is exhausted".into()))
}

/// How a transaction's events split into `MutationBatch` records: for each
/// record, the end index (exclusive) of its events and its payload length.
type BatchPlan = Vec<(usize, usize)>;

/// Split `events` into batch records of at most about `target` bytes each
/// (a single event larger than that gets a record of its own, up to the
/// record size limit). Sizes every event, so an event the encoder would
/// reject fails here.
fn plan_batches(events: &[MutationEvent], target: usize) -> Result<BatchPlan, WalError> {
    let mut plan = Vec::with_capacity(1);
    let mut start = 0;
    let mut chunk_bytes = 0usize;
    for (index, event) in events.iter().enumerate() {
        let size = codec::encoded_batch_event_len(event)?;
        let count = index - start;
        if count > 0 {
            let grown = codec::batch_payload_len(count + 1, chunk_bytes.saturating_add(size))?;
            if !matches!(framed_len(grown), Ok(len) if len <= target) {
                plan.push((index, batch_payload_len_checked(count, chunk_bytes)?));
                start = index;
                chunk_bytes = 0;
            }
        }
        chunk_bytes = chunk_bytes
            .checked_add(size)
            .ok_or_else(|| WalError::Encode("transaction payload length overflows usize".into()))?;
    }
    plan.push((
        events.len(),
        batch_payload_len_checked(events.len() - start, chunk_bytes)?,
    ));
    Ok(plan)
}

/// Payload length of one planned batch record, rejecting a record over
/// the size limit (only possible for a batch of one huge event).
fn batch_payload_len_checked(count: usize, events_len: usize) -> Result<usize, WalError> {
    let payload_len = codec::batch_payload_len(count, events_len)?;
    framed_len(payload_len).map_err(|_| {
        WalError::Encode(format!(
            "a single mutation encodes to {payload_len} bytes, over the {MAX_RECORD_LEN}-byte \
             WAL record limit"
        ))
    })?;
    Ok(payload_len)
}

/// Copy the bytes a torn-tail truncation is about to drop to
/// `<segment>.torn-<offset>` beside the segment, so an operator can
/// inspect them. Replay only calls a tail torn when no valid record
/// follows it, but keeping the bytes costs little and makes a wrong call
/// recoverable.
fn preserve_torn_tail(segments: &SegmentDir, torn: &TornTailInfo) -> Result<(), WalError> {
    keep_segment_tail(segments, &torn.segment_path, torn.last_good_offset, "torn")?;
    Ok(())
}

/// Copy `segment`'s bytes from `offset` to its end into a new file
/// `<segment>.<kind>-<offset>` beside it (with a `-<n>` suffix if that
/// name is taken), fsynced. Returns the new file, or `None` when the
/// segment has no bytes past `offset`.
fn keep_segment_tail(
    segments: &SegmentDir,
    segment: &Path,
    offset: u64,
    kind: &str,
) -> Result<Option<PathBuf>, WalError> {
    let mut source = fs::File::open(segment)?;
    let len = source.metadata()?.len();
    if len <= offset {
        return Ok(None);
    }
    source.seek(SeekFrom::Start(offset))?;
    let file_name = segment
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (mut out, path) = create_unique(segments, &format!("{file_name}.{kind}-{offset}"))?;
    std::io::copy(&mut (&mut source).take(len - offset), &mut out)?;
    out.flush()?;
    sync_file(&out)?;
    segments.sync_dir()?;
    Ok(Some(path))
}

/// Create `<dir>/<name>`, or `<name>-<n>` for the first free `n`.
fn create_unique(segments: &SegmentDir, name: &str) -> Result<(fs::File, PathBuf), WalError> {
    let mut attempt = 0u32;
    loop {
        let path = segments.root().join(unique_name(name, attempt));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && attempt < 1000 => {
                attempt += 1;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn unique_name(name: &str, attempt: u32) -> String {
    if attempt == 0 {
        name.to_string()
    } else {
        format!("{name}-{attempt}")
    }
}

/// Cut the log at `cut`: keep the corrupt segment's bytes from the cut
/// offset beside it, move every later segment out of the log, then
/// truncate the corrupt segment. `from` starts with the corrupt segment.
/// Each step keeps a crash safe to retry: until the truncate, the next
/// open finds the same corruption and cuts again.
fn cut_corrupt_log(
    segments: &SegmentDir,
    from: &[crate::dir::SegmentEntry],
    cut: &mut CorruptCut,
) -> Result<(), WalError> {
    let Some((corrupt, later)) = from.split_first() else {
        return Ok(());
    };
    cut.kept_tail = keep_segment_tail(segments, &corrupt.path, cut.offset, "corrupt")?;

    let stem = corrupt
        .path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for entry in later {
        let file_name = entry
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = format!("{file_name}.after-corrupt-{stem}-{}", cut.offset);
        let mut attempt = 0u32;
        let target = loop {
            let target = segments.root().join(unique_name(&base, attempt));
            if !target.exists() || attempt >= 1000 {
                break target;
            }
            attempt += 1;
        };
        if target.exists() {
            return Err(WalError::Malformed(format!(
                "cannot move WAL segment {} aside: {} exists",
                entry.path.display(),
                target.display()
            )));
        }
        fs::rename(&entry.path, &target)?;
        cut.moved_segments.push((entry.path.clone(), target));
    }
    segments.sync_dir()?;

    let file = OpenOptions::new().write(true).open(&corrupt.path)?;
    file.set_len(cut.offset)?;
    sync_file(&file)?;
    segments.sync_dir()?;

    tracing::warn!(
        segment = %corrupt.path.display(),
        offset = cut.offset,
        reason = %cut.reason,
        kept_tail = ?cut.kept_tail,
        moved_segments = ?cut.moved_segments,
        "WAL was corrupt and truncate_corrupt_tail is set: cut the log at the corrupt record; \
         transactions from that point on are not recovered and were kept beside the log"
    );
    Ok(())
}

impl Drop for Wal {
    fn drop(&mut self) {
        let _ = self.flush_inner(FlushKind::ForceFsync);
        // Join the group flusher, if any, before the directory lock is
        // released. That keeps the "one live append owner" boundary intact
        // through shutdown.
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(slot) = self.flusher.get_mut() {
            let _ = slot.take();
        }
    }
}
