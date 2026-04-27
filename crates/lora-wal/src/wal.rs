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
#[cfg(test)]
use std::fs::OpenOptions;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use lora_store::MutationEvent;

use crate::config::SyncMode;
use crate::dir::{SegmentDir, SegmentId};
use crate::error::WalError;
use crate::lock::DirLock;
use crate::lsn::Lsn;
use crate::record::WalRecord;
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

/// Latched failure from the background flusher. Wrapped in a `Mutex`
/// instead of an `AtomicCell<Option<String>>` because failures are
/// rare and we want the message preserved verbatim for operator-facing
/// reporting (`/admin/wal/status` `bgFailure`). Once `Some`, every
/// subsequent commit/flush returns [`WalError::Poisoned`] and the
/// operator is expected to restart from the last consistent
/// snapshot + WAL.
type BgFailure = Mutex<Option<String>>;

/// Selects the durability work that [`Wal::flush_inner`] actually does.
/// Centralising the three modes here means `flush` and `force_fsync`
/// share one code path and the call sites don't have to remember which
/// mode advances `durable_lsn` and which does not.
#[derive(Debug, Clone, Copy)]
enum FlushKind {
    /// Honour the configured [`SyncMode`]. This is what the recorder's
    /// `flush()` calls into.
    PerConfiguredMode,
    /// Always write the buffer + fsync + advance `durable_lsn`,
    /// regardless of mode. Used by checkpoints and the bg flusher.
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
    /// Latched bg-flusher failure; surfaced via [`Wal::bg_failure`] and
    /// propagated to commit/flush/force_fsync as
    /// [`WalError::Poisoned`].
    bg_failure: Arc<BgFailure>,
    /// Background flusher for `SyncMode::Group`. `Drop` joins the
    /// thread, so a `Wal` going out of scope is a clean shutdown
    /// signal.
    _flusher: Mutex<Option<GroupFlusherHandle>>,
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
            _flusher: Mutex::new(None),
            _dir_lock: dir_lock,
        });

        // Spawn the Group flusher *after* the Arc exists so it can
        // hold a `Weak<Wal>` that drops when the last strong ref
        // does. The flusher's own Drop joins the thread, so removing
        // the field (e.g. on Wal::drop) is a clean shutdown signal.
        if let SyncMode::Group { interval_ms } = sync_mode {
            let interval = Duration::from_millis(u64::from(interval_ms.max(1)));
            let handle = spawn_group_flusher(Arc::downgrade(&wal), interval);
            *wal._flusher.lock().unwrap() = Some(handle);
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

    /// Latched message from the background flusher, if it has ever
    /// failed an `fsync`. `None` means the WAL is healthy. Once set,
    /// every commit / flush / force_fsync starts returning
    /// [`WalError::Poisoned`] and the WAL stops accepting new
    /// transactions until the operator restarts from the last
    /// consistent snapshot + WAL.
    pub fn bg_failure(&self) -> Option<String> {
        self.bg_failure.lock().unwrap().clone()
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

    /// Begin a new transaction. Allocates a `TxBegin` record and
    /// returns its LSN, which the caller must thread back through
    /// `append` / `commit` / `abort` so replay can group the events.
    ///
    /// If the active segment has crossed `segment_target_bytes`,
    /// rotation happens here — `TxBegin` is the only record kind
    /// guaranteed to be a transaction boundary, so rotating just
    /// before its append keeps every transaction wholly in one
    /// segment.
    pub fn begin(&self) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        self.maybe_rotate(&mut state)?;
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxBegin { lsn })
    }

    /// Append a single mutation to the in-memory pending buffer of
    /// the active segment. Not durable until `flush()` runs.
    pub fn append(&self, tx_begin_lsn: Lsn, event: &MutationEvent) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::Mutation {
            lsn,
            tx_begin_lsn,
            event: event.clone(),
        })
    }

    /// Append many mutations as one framed record. This keeps the replay
    /// contract identical to repeated `append` calls while avoiding per-event
    /// length/CRC/framing overhead for write-heavy statements.
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

    /// Append a `TxCommit` marker. Caller is expected to subsequently
    /// call `flush()` (under `SyncMode::PerCommit`) to make the
    /// commit durable before returning to its caller.
    pub fn commit(&self, tx_begin_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxCommit { lsn, tx_begin_lsn })
    }

    /// Append a `TxAbort` marker. Replay drops the events keyed by
    /// `tx_begin_lsn` without re-applying them.
    pub fn abort(&self, tx_begin_lsn: Lsn) -> Result<Lsn, WalError> {
        self.check_healthy()?;
        let mut state = self.state.lock().unwrap();
        Self::alloc_and_append(&mut state, |lsn| WalRecord::TxAbort { lsn, tx_begin_lsn })
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
    /// - `Group` — write the buffer to the OS, but let the background
    ///   flusher fsync and advance `durable_lsn` on its cadence.
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
    /// checkpoint). Returns [`WalError::Poisoned`] if the bg flusher
    /// has already failed.
    pub fn force_fsync(&self) -> Result<(), WalError> {
        self.check_healthy()?;
        self.flush_inner(FlushKind::ForceFsync)
    }

    /// Single source of truth for the flush state machine. Skips the
    /// `check_healthy` gate so the bg flusher can call into it
    /// without recursing through its own latch.
    fn flush_inner(&self, kind: FlushKind) -> Result<(), WalError> {
        let mut state = self.state.lock().unwrap();
        let written_lsn = Lsn::new(state.next_lsn.raw().saturating_sub(1));

        // Decide whether this call is allowed to advance
        // `durable_lsn`. The bg flusher's job in Group mode is to advance
        // that fence after fsync; PerCommit and None do it inline; Group's
        // user-driven `flush()` only pushes bytes to the OS.
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
        if let Ok(slot) = self._flusher.get_mut() {
            let _ = slot.take();
        }
    }
}

// ---------------------------------------------------------------------------
// Group-mode background flusher
// ---------------------------------------------------------------------------

/// Owns the OS thread that periodically `fsync`s the WAL under
/// `SyncMode::Group`. Held inside the `Wal` itself so dropping the
/// last `Arc<Wal>` runs `Drop` here, signals shutdown, and joins
/// before the underlying `WalState` is destroyed.
struct GroupFlusherHandle {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for GroupFlusherHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            // `let _ = ...` because the thread can only fail by
            // panicking; even then, the Wal itself is being dropped
            // and there is nothing useful to do with the panic at
            // teardown.
            let _ = h.join();
        }
    }
}

fn spawn_group_flusher(weak: Weak<Wal>, interval: Duration) -> GroupFlusherHandle {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    let handle = thread::spawn(move || {
        // Sleep first so a shortlived Wal that opens-and-closes
        // immediately doesn't pay for an extra wakeup. We re-check
        // the shutdown flag at every iteration so a Drop signal
        // racing with a sleep wakes up at most one interval late.
        while !shutdown_clone.load(Ordering::Acquire) {
            // Break the sleep into ~50 ms slices so shutdown can be
            // observed without waiting up to a full `interval` at
            // teardown. This matters for tests, which want fast
            // join times.
            let slice = Duration::from_millis(50).min(interval);
            let mut elapsed = Duration::ZERO;
            while elapsed < interval && !shutdown_clone.load(Ordering::Acquire) {
                thread::sleep(slice);
                elapsed += slice;
            }
            if shutdown_clone.load(Ordering::Acquire) {
                break;
            }
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
                        let mut slot = wal.bg_failure.lock().unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use lora_store::{MutationEvent, Properties, PropertyValue};

    use crate::testing::TmpDir;

    fn ev(id: u64) -> MutationEvent {
        let mut p = Properties::new();
        p.insert("v".into(), PropertyValue::Int(id as i64));
        MutationEvent::CreateNode {
            id,
            labels: vec!["N".into()],
            properties: p,
        }
    }

    fn open_default(dir: &Path) -> (Arc<Wal>, Vec<MutationEvent>) {
        Wal::open(dir, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO).unwrap()
    }

    #[test]
    fn fresh_open_creates_first_segment() {
        let dir = TmpDir::new("fresh");
        let (wal, replay) = open_default(&dir.path);
        assert!(replay.is_empty());
        assert_eq!(wal.next_lsn(), Lsn::new(1));
        assert_eq!(wal.active_segment_id(), 1);
        // No CURRENT pointer file is written — the highest segment id
        // is the source of truth for "active segment".
        let entries: Vec<_> = std::fs::read_dir(&dir.path)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.iter().any(|n| n == ".lora-wal.lock"),
            "WAL dir should contain the live directory lock, found: {entries:?}"
        );
        assert!(
            entries
                .iter()
                .filter(|n| n.as_str() != ".lora-wal.lock")
                .all(|n| n.ends_with(".wal")),
            "WAL dir should contain only segment files plus the lock, found: {entries:?}"
        );
    }

    #[test]
    fn opening_same_directory_twice_fails_until_first_handle_drops() {
        let dir = TmpDir::new("exclusive");
        let (wal, _) = open_default(&dir.path);

        match Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO) {
            Err(WalError::AlreadyOpen { dir: locked_dir }) => {
                assert_eq!(locked_dir, dir.path);
            }
            Err(err) => panic!("expected AlreadyOpen, got {err:?}"),
            Ok(_) => panic!("second WAL open on same directory should fail"),
        }

        drop(wal);
        let (reopened, _) = open_default(&dir.path);
        drop(reopened);
    }

    #[test]
    fn begin_append_commit_round_trip_through_replay() {
        let dir = TmpDir::new("commit");

        // First boot: write three transactions and crash without
        // running shutdown.
        {
            let (wal, _) = open_default(&dir.path);
            let begin = wal.begin().unwrap();
            wal.append(begin, &ev(1)).unwrap();
            wal.append(begin, &ev(2)).unwrap();
            wal.commit(begin).unwrap();
            wal.flush().unwrap();

            let begin = wal.begin().unwrap();
            wal.append(begin, &ev(3)).unwrap();
            wal.commit(begin).unwrap();
            wal.flush().unwrap();
            // drop without explicit close
        }

        // Second boot: replay should yield events 1, 2, 3 in order.
        let (wal, replay) = open_default(&dir.path);
        assert_eq!(replay.len(), 3);
        assert_eq!(replay[0], ev(1));
        assert_eq!(replay[1], ev(2));
        assert_eq!(replay[2], ev(3));
        // next_lsn should be past every record we wrote (2 begins +
        // 3 mutations + 2 commits = 7 records → next_lsn = 8).
        assert_eq!(wal.next_lsn(), Lsn::new(8));
    }

    #[test]
    fn aborted_transaction_is_dropped_on_replay() {
        let dir = TmpDir::new("abort");

        {
            let (wal, _) = open_default(&dir.path);
            let b1 = wal.begin().unwrap();
            wal.append(b1, &ev(1)).unwrap();
            wal.commit(b1).unwrap();
            wal.flush().unwrap();

            let b2 = wal.begin().unwrap();
            wal.append(b2, &ev(99)).unwrap();
            wal.abort(b2).unwrap();
            wal.flush().unwrap();
        }

        let (_, replay) = open_default(&dir.path);
        assert_eq!(replay, vec![ev(1)]);
    }

    #[test]
    fn uncommitted_transaction_at_end_of_log_is_discarded() {
        let dir = TmpDir::new("uncommitted");

        {
            let (wal, _) = open_default(&dir.path);
            let b1 = wal.begin().unwrap();
            wal.append(b1, &ev(1)).unwrap();
            wal.commit(b1).unwrap();
            wal.flush().unwrap();

            // Begin + append but never commit. Simulates a crash
            // mid-query.
            let b2 = wal.begin().unwrap();
            wal.append(b2, &ev(99)).unwrap();
            wal.flush().unwrap();
        }

        let (_, replay) = open_default(&dir.path);
        assert_eq!(replay, vec![ev(1)]);
    }

    #[test]
    fn segment_rotation_at_begin_boundary() {
        let dir = TmpDir::new("rotate");

        // Tiny segment target so we trip rotation on the second
        // transaction.
        let (wal, _) = Wal::open(&dir.path, SyncMode::PerCommit, 256, Lsn::ZERO).unwrap();

        // First tx: a few events, takes us past 256 bytes.
        let b1 = wal.begin().unwrap();
        for i in 0..5 {
            wal.append(b1, &ev(i)).unwrap();
        }
        wal.commit(b1).unwrap();
        wal.flush().unwrap();
        assert_eq!(wal.active_segment_id(), 1);

        // Second `begin` triggers rotation.
        let b2 = wal.begin().unwrap();
        wal.append(b2, &ev(100)).unwrap();
        wal.commit(b2).unwrap();
        wal.flush().unwrap();
        assert_eq!(
            wal.active_segment_id(),
            2,
            "begin() should have rotated to segment 2"
        );

        let segments = SegmentDir::new(&dir.path).list().unwrap();
        assert_eq!(segments.len(), 2);

        drop(wal);
        let (_, replay) = open_default(&dir.path);
        assert_eq!(replay.len(), 6);
    }

    #[test]
    fn checkpoint_lsn_skips_already_checkpointed_events() {
        let dir = TmpDir::new("ckpt-skip");
        let (wal, _) = open_default(&dir.path);

        // Tx A: events 1,2 — ends at lsn 4.
        let a = wal.begin().unwrap();
        wal.append(a, &ev(1)).unwrap();
        wal.append(a, &ev(2)).unwrap();
        let commit_a = wal.commit(a).unwrap();
        wal.flush().unwrap();

        // Tx B: event 3 — past the fence.
        let b = wal.begin().unwrap();
        wal.append(b, &ev(3)).unwrap();
        wal.commit(b).unwrap();
        wal.flush().unwrap();
        drop(wal);

        // Re-open with checkpoint_lsn = commit_a so tx A is treated
        // as already-applied.
        let (_, replay) =
            Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, commit_a).unwrap();
        assert_eq!(replay, vec![ev(3)]);
    }

    #[test]
    fn replay_rejects_commit_without_begin() {
        let dir = TmpDir::new("commit-without-begin");

        {
            let (wal, _) = open_default(&dir.path);
            wal.commit(Lsn::new(99)).unwrap();
            wal.flush().unwrap();
        }

        let err = match Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO) {
            Ok(_) => panic!("malformed WAL should not open"),
            Err(err) => err,
        };
        assert!(
            matches!(err, WalError::Malformed(ref msg) if msg.contains("missing tx begin")),
            "expected malformed missing-begin error, got {err:?}"
        );
    }

    #[test]
    fn replay_rejects_mutation_without_begin() {
        let dir = TmpDir::new("mutation-without-begin");

        {
            let (wal, _) = open_default(&dir.path);
            wal.append(Lsn::new(99), &ev(1)).unwrap();
            wal.flush().unwrap();
        }

        let err = match Wal::open(&dir.path, SyncMode::PerCommit, 8 * 1024 * 1024, Lsn::ZERO) {
            Ok(_) => panic!("malformed WAL should not open"),
            Err(err) => err,
        };
        assert!(
            matches!(err, WalError::Malformed(ref msg) if msg.contains("missing tx begin")),
            "expected malformed missing-begin error, got {err:?}"
        );
    }

    #[test]
    fn torn_tail_is_truncated_on_open() {
        let dir = TmpDir::new("torn");

        {
            let (wal, _) = open_default(&dir.path);
            let b = wal.begin().unwrap();
            wal.append(b, &ev(1)).unwrap();
            wal.commit(b).unwrap();
            wal.flush().unwrap();
        }

        // Append garbage to the active segment by hand.
        let segments = SegmentDir::new(&dir.path).list().unwrap();
        let active = &segments.last().unwrap().path;
        {
            use std::io::Write;
            let mut f = OpenOptions::new().append(true).open(active).unwrap();
            f.write_all(&[0xff; 32]).unwrap();
            f.sync_all().unwrap();
        }

        // Re-open. Torn tail must be truncated; replay still yields
        // ev(1); next_lsn picks up cleanly.
        let (wal, replay) = open_default(&dir.path);
        assert_eq!(replay, vec![ev(1)]);

        // Subsequent appends don't trip a CRC failure.
        let b = wal.begin().unwrap();
        wal.append(b, &ev(2)).unwrap();
        wal.commit(b).unwrap();
        wal.flush().unwrap();
        drop(wal);

        let (_, replay) = open_default(&dir.path);
        assert_eq!(replay, vec![ev(1), ev(2)]);
    }

    #[test]
    fn checkpoint_marker_is_recorded_and_observed() {
        let dir = TmpDir::new("ckpt-marker");

        let snapshot_lsn = {
            let (wal, _) = open_default(&dir.path);
            let b = wal.begin().unwrap();
            wal.append(b, &ev(1)).unwrap();
            let commit = wal.commit(b).unwrap();
            wal.flush().unwrap();
            wal.checkpoint_marker(commit).unwrap();
            wal.flush().unwrap();
            commit
        };

        let outcome = crate::replay::replay_dir(&dir.path, Lsn::ZERO).unwrap();
        assert_eq!(
            outcome.checkpoint_lsn_observed,
            Some(snapshot_lsn),
            "checkpoint marker should be surfaced by replay"
        );
    }

    #[test]
    fn group_mode_durable_lsn_advances_via_bg_flusher() {
        let dir = TmpDir::new("group");
        // 25 ms interval = bg flusher should land within one or two
        // 50 ms slices.
        let (wal, _) = Wal::open(
            &dir.path,
            SyncMode::Group { interval_ms: 25 },
            8 * 1024 * 1024,
            Lsn::ZERO,
        )
        .unwrap();

        let begin = wal.begin().unwrap();
        wal.append(begin, &ev(1)).unwrap();
        wal.commit(begin).unwrap();
        wal.flush().unwrap(); // Group: write_buffer only; durable_lsn untouched.

        // Immediately after a Group flush, durable_lsn should still
        // be Lsn::ZERO — the bg flusher hasn't fired yet.
        assert_eq!(
            wal.durable_lsn(),
            Lsn::ZERO,
            "Group flush() must not advance durable_lsn"
        );

        // Wait up to ~500 ms for the bg flusher to advance the LSN.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            if wal.durable_lsn() > Lsn::ZERO {
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "bg flusher did not advance durable_lsn within 500 ms (still at {})",
                    wal.durable_lsn()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
        // Wal drop should join the bg thread cleanly.
        drop(wal);
    }

    #[test]
    fn none_mode_advances_durable_lsn_on_flush() {
        let dir = TmpDir::new("none");
        let (wal, _) = Wal::open(&dir.path, SyncMode::None, 8 * 1024 * 1024, Lsn::ZERO).unwrap();

        let begin = wal.begin().unwrap();
        wal.append(begin, &ev(1)).unwrap();
        wal.commit(begin).unwrap();
        wal.flush().unwrap();

        // None mode: flush() advances durable_lsn even without
        // fsync, because the mode opted out of crash durability.
        assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
    }

    #[test]
    fn force_fsync_always_advances_durable_lsn() {
        let dir = TmpDir::new("force-fsync");
        let (wal, _) = Wal::open(
            &dir.path,
            SyncMode::Group {
                interval_ms: 60_000,
            },
            8 * 1024 * 1024,
            Lsn::ZERO,
        )
        .unwrap();

        let begin = wal.begin().unwrap();
        wal.append(begin, &ev(1)).unwrap();
        wal.commit(begin).unwrap();
        wal.flush().unwrap(); // Group flush: durable_lsn unchanged.
        assert_eq!(wal.durable_lsn(), Lsn::ZERO);

        // force_fsync bypasses the configured cadence — used by
        // checkpoints to grab a fence on demand.
        wal.force_fsync().unwrap();
        assert_eq!(wal.durable_lsn().raw(), wal.next_lsn().raw() - 1);
    }

    #[test]
    fn truncate_up_to_drops_old_sealed_segments() {
        let dir = TmpDir::new("truncate");

        // Tiny target so each tx forces a rotation on the next begin.
        let (wal, _) = Wal::open(&dir.path, SyncMode::PerCommit, 64, Lsn::ZERO).unwrap();

        let mut last_commit = Lsn::ZERO;
        for i in 0..5 {
            let b = wal.begin().unwrap();
            wal.append(b, &ev(i)).unwrap();
            last_commit = wal.commit(b).unwrap();
            wal.flush().unwrap();
        }
        // Five transactions × tiny target: we should be on segment ≥ 4.
        assert!(
            wal.active_segment_id() >= 4,
            "expected several rotations, got {}",
            wal.active_segment_id()
        );

        let segments = SegmentDir::new(&dir.path);
        let before = segments.list().unwrap().len();
        wal.truncate_up_to(last_commit).unwrap();
        let after = segments.list().unwrap().len();

        assert!(
            after < before,
            "truncate_up_to should have dropped at least one segment ({} → {})",
            before,
            after
        );
        // Active + tombstone are always retained.
        assert!(
            after >= 2,
            "active and the segment preceding it must be kept"
        );

        // Subsequent appends + reopen still produce all five events
        // because the dropped segments only contained transactions
        // already at or below `last_commit`, which we feed back as
        // the checkpoint fence on reopen.
        drop(wal);
        let (_, replay) = Wal::open(&dir.path, SyncMode::PerCommit, 64, last_commit).unwrap();
        // Everything was at or below the fence, so replay is empty.
        assert!(replay.is_empty());
    }
}
