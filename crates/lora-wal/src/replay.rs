//! WAL replay: walk segments, surface only committed mutation events,
//! and report the byte position of any torn tail so the caller can
//! truncate the active segment cleanly.
//!
//! The walk is deliberately a single forward sweep over the segments in
//! ascending id order. Per-transaction events are buffered in a
//! `BTreeMap<tx_begin_lsn, Vec<MutationEvent>>` and only released on the
//! corresponding [`WalRecord::TxCommit`]; an [`WalRecord::TxAbort`] (or
//! a missing commit at end-of-log) causes the bucket to be dropped
//! without emission. This is the contract the WAL plan calls
//! "per-mutation log, per-query commit": the recorder writes one record
//! per primitive mutation but replay only ever reapplies whole queries.
//!
//! Replay walks past `checkpoint_lsn`: any record whose LSN is at or
//! below it is already represented in the loaded snapshot and is
//! skipped. Because checkpoints are taken with the store write lock
//! held, no transaction can straddle the fence — `tx_begin_lsn <=
//! checkpoint_lsn` implies the matching commit (if any) is also at or
//! below it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lora_store::MutationEvent;

use crate::dir::SegmentDir;
use crate::errors::WalError;
use crate::lsn::Lsn;
use crate::record::WalRecord;
use crate::segment::{SegmentReader, SEGMENT_HEADER_LEN};

/// Outcome of a full replay walk.
#[derive(Debug)]
pub struct ReplayOutcome {
    /// Mutation events from committed transactions, in append order.
    /// Apply these to a fresh store (or one freshly loaded from a
    /// snapshot at `checkpoint_lsn`) to reproduce the pre-crash state.
    pub committed_events: Vec<MutationEvent>,

    /// Highest LSN observed in any segment, regardless of whether the
    /// owning transaction committed. Used to seed `next_lsn` for new
    /// appends so we never reuse an already-allocated id.
    pub max_lsn: Lsn,

    /// Torn-tail diagnostic. `Some` iff a record failed to decode
    /// before the natural end-of-log. The caller must truncate the
    /// affected segment to `last_good_offset` before resuming
    /// appends, otherwise replay-after-replay will keep tripping the
    /// same CRC.
    pub torn_tail: Option<TornTailInfo>,

    /// Newest checkpoint LSN observed in a [`WalRecord::Checkpoint`]
    /// marker.
    ///
    /// Informational only. The recovery contract today is "the
    /// snapshot's `wal_lsn` is the replay fence" — if the operator
    /// passes an older snapshot than the newest checkpoint marker on
    /// disk we still replay every record above the snapshot's fence,
    /// which is conservative-correct (the only cost is duplicated
    /// work). A tighter "marker overrides snapshot" contract is
    /// deferred to v2 because it would require us to know that the
    /// marker's snapshot file actually exists where the operator can
    /// reach it, which is a separate observability concern.
    ///
    /// Surfaced so callers (e.g. `lora-database::Database::recover`)
    /// can log a warning when the snapshot is older than the newest
    /// observed marker.
    pub checkpoint_lsn_observed: Option<Lsn>,

    /// Offset immediately after the last well-formed record in the last
    /// segment walked. `Wal::open` uses this to reopen the active writer
    /// without performing a second full scan of the active segment.
    pub last_good_offset: u64,
}

#[derive(Debug)]
pub struct TornTailInfo {
    pub segment_path: PathBuf,
    pub last_good_offset: u64,
    pub cause: WalError,
}

/// Walk every segment in `paths` (already in ascending id order).
///
/// Records with `lsn <= checkpoint_lsn` are skipped — they are already
/// captured in the snapshot the caller is about to restore from.
pub(crate) fn replay_segments(
    paths: &[PathBuf],
    checkpoint_lsn: Lsn,
) -> Result<ReplayOutcome, WalError> {
    let mut state = ReplayState::new();
    let mut torn_tail: Option<TornTailInfo> = None;
    let mut last_good_offset = SEGMENT_HEADER_LEN as u64;

    'outer: for path in paths {
        let mut reader = SegmentReader::open(path)?;
        last_good_offset = reader.position();
        let segment_base = reader.header().base_lsn;
        state.validate_segment(segment_base, path)?;

        loop {
            // Capture position before the read so we can report the
            // start-of-bad-record offset on torn tail.
            let before = reader.position();
            match reader.read_record() {
                Ok(Some(record)) => {
                    state.accept_record(record, segment_base, checkpoint_lsn, path)?;
                    last_good_offset = reader.position();
                }
                Ok(None) => break,
                Err(err) => {
                    torn_tail = Some(TornTailInfo {
                        segment_path: path.clone(),
                        last_good_offset: before,
                        cause: err,
                    });
                    break 'outer;
                }
            }
        }
    }

    Ok(state.finish(torn_tail, last_good_offset))
}

struct ReplayState {
    pending: BTreeMap<Lsn, Vec<MutationEvent>>,
    committed: Vec<MutationEvent>,
    max_lsn: Lsn,
    last_lsn: Lsn,
    last_segment_base: Option<Lsn>,
    checkpoint_lsn_observed: Option<Lsn>,
}

impl ReplayState {
    fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            committed: Vec::new(),
            max_lsn: Lsn::ZERO,
            last_lsn: Lsn::ZERO,
            last_segment_base: None,
            checkpoint_lsn_observed: None,
        }
    }

    fn validate_segment(&mut self, segment_base: Lsn, path: &Path) -> Result<(), WalError> {
        if let Some(prev_base) = self.last_segment_base {
            if segment_base <= prev_base {
                return Err(WalError::Malformed(format!(
                    "segment base_lsn {} is not greater than previous base_lsn {} ({})",
                    segment_base.raw(),
                    prev_base.raw(),
                    path.display()
                )));
            }
        }
        if !self.last_lsn.is_zero() && segment_base <= self.last_lsn {
            return Err(WalError::Malformed(format!(
                "segment base_lsn {} is not greater than previous record lsn {} ({})",
                segment_base.raw(),
                self.last_lsn.raw(),
                path.display()
            )));
        }
        self.last_segment_base = Some(segment_base);
        Ok(())
    }

    fn accept_record(
        &mut self,
        record: WalRecord,
        segment_base: Lsn,
        checkpoint_lsn: Lsn,
        path: &Path,
    ) -> Result<(), WalError> {
        let lsn = record.lsn();
        self.validate_record_lsn(lsn, segment_base, path)?;
        self.observe_lsn(lsn);

        if lsn.raw() <= checkpoint_lsn.raw() {
            self.skip_fenced_record(&record);
            return Ok(());
        }

        self.apply_record(record, lsn)
    }

    fn validate_record_lsn(
        &self,
        lsn: Lsn,
        segment_base: Lsn,
        path: &Path,
    ) -> Result<(), WalError> {
        if lsn < segment_base {
            return Err(WalError::Malformed(format!(
                "record lsn {} is below segment base_lsn {} ({})",
                lsn.raw(),
                segment_base.raw(),
                path.display()
            )));
        }
        if !self.last_lsn.is_zero() && lsn <= self.last_lsn {
            return Err(WalError::Malformed(format!(
                "record lsn {} is not greater than previous lsn {} ({})",
                lsn.raw(),
                self.last_lsn.raw(),
                path.display()
            )));
        }
        Ok(())
    }

    fn observe_lsn(&mut self, lsn: Lsn) {
        self.last_lsn = lsn;
        if lsn > self.max_lsn {
            self.max_lsn = lsn;
        }
    }

    fn skip_fenced_record(&mut self, record: &WalRecord) {
        // Already in the snapshot. Markers below the fence still need
        // to keep their pending bucket clean, but no checkpoint can
        // split a transaction (the store write lock is held during
        // checkpoint), so dropping events outright is safe.
        if let WalRecord::TxCommit { tx_begin_lsn, .. } | WalRecord::TxAbort { tx_begin_lsn, .. } =
            record
        {
            self.pending.remove(tx_begin_lsn);
        }
    }

    fn apply_record(&mut self, record: WalRecord, lsn: Lsn) -> Result<(), WalError> {
        match record {
            WalRecord::Mutation {
                tx_begin_lsn,
                event,
                ..
            } => self
                .pending_events_mut(tx_begin_lsn, lsn, "mutation")?
                .push(event),
            WalRecord::MutationBatch {
                tx_begin_lsn,
                events,
                ..
            } => self
                .pending_events_mut(tx_begin_lsn, lsn, "mutation batch")?
                .extend(events),
            WalRecord::TxBegin { lsn } => self.begin_transaction(lsn)?,
            WalRecord::TxCommit { tx_begin_lsn, .. } => {
                let events = self.take_pending(tx_begin_lsn, lsn, "commit")?;
                self.committed.extend(events);
            }
            WalRecord::TxAbort { tx_begin_lsn, .. } => {
                let _ = self.take_pending(tx_begin_lsn, lsn, "abort")?;
            }
            WalRecord::Checkpoint { snapshot_lsn, .. } => {
                self.observe_checkpoint(lsn, snapshot_lsn)?;
            }
        }
        Ok(())
    }

    fn begin_transaction(&mut self, lsn: Lsn) -> Result<(), WalError> {
        // Materialise the bucket eagerly so begin-without-mutations
        // transactions still get a deterministic commit/abort.
        if self.pending.insert(lsn, Vec::new()).is_some() {
            return Err(WalError::Malformed(format!(
                "duplicate tx begin at lsn {}",
                lsn.raw()
            )));
        }
        Ok(())
    }

    fn pending_events_mut(
        &mut self,
        tx_begin_lsn: Lsn,
        record_lsn: Lsn,
        kind: &str,
    ) -> Result<&mut Vec<MutationEvent>, WalError> {
        self.pending
            .get_mut(&tx_begin_lsn)
            .ok_or_else(|| missing_tx_begin(kind, record_lsn, tx_begin_lsn))
    }

    fn take_pending(
        &mut self,
        tx_begin_lsn: Lsn,
        record_lsn: Lsn,
        kind: &str,
    ) -> Result<Vec<MutationEvent>, WalError> {
        self.pending
            .remove(&tx_begin_lsn)
            .ok_or_else(|| missing_tx_begin(kind, record_lsn, tx_begin_lsn))
    }

    fn observe_checkpoint(&mut self, lsn: Lsn, snapshot_lsn: Lsn) -> Result<(), WalError> {
        if snapshot_lsn > lsn {
            return Err(WalError::Malformed(format!(
                "checkpoint at lsn {} points to future snapshot lsn {}",
                lsn.raw(),
                snapshot_lsn.raw()
            )));
        }
        if let Some(prev) = self.checkpoint_lsn_observed {
            if snapshot_lsn < prev {
                return Err(WalError::Malformed(format!(
                    "checkpoint snapshot lsn {} regressed below previous checkpoint {}",
                    snapshot_lsn.raw(),
                    prev.raw()
                )));
            }
        }
        self.checkpoint_lsn_observed = Some(snapshot_lsn);
        Ok(())
    }

    fn finish(self, torn_tail: Option<TornTailInfo>, last_good_offset: u64) -> ReplayOutcome {
        // Any transaction still in `pending` at end-of-log was started
        // but never committed (and never explicitly aborted). Treat as
        // crashed mid-query and discard.
        ReplayOutcome {
            committed_events: self.committed,
            max_lsn: self.max_lsn,
            torn_tail,
            checkpoint_lsn_observed: self.checkpoint_lsn_observed,
            last_good_offset,
        }
    }
}

fn missing_tx_begin(kind: &str, record_lsn: Lsn, tx_begin_lsn: Lsn) -> WalError {
    WalError::Malformed(format!(
        "{kind} at lsn {} references missing tx begin {}",
        record_lsn.raw(),
        tx_begin_lsn.raw()
    ))
}

/// Convenience: replay every `*.wal` segment in `dir` after sorting by
/// the numeric prefix encoded in the file name.
///
/// Used by recovery diagnostics in `lora-database::Database::recover`
/// to peek at the newest checkpoint marker before opening the live
/// `Wal` handle.
pub fn replay_dir(dir: &Path, checkpoint_lsn: Lsn) -> Result<ReplayOutcome, WalError> {
    let entries = SegmentDir::new(dir).list()?;
    let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
    replay_segments(&paths, checkpoint_lsn)
}
