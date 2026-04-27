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
use crate::error::WalError;
use crate::lsn::Lsn;
use crate::record::WalRecord;
use crate::segment::SegmentReader;

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
    let mut pending: BTreeMap<Lsn, Vec<MutationEvent>> = BTreeMap::new();
    let mut committed: Vec<MutationEvent> = Vec::new();
    let mut max_lsn = Lsn::ZERO;
    let mut last_lsn = Lsn::ZERO;
    let mut last_segment_base: Option<Lsn> = None;
    let mut torn_tail: Option<TornTailInfo> = None;
    let mut checkpoint_lsn_observed: Option<Lsn> = None;

    'outer: for path in paths {
        let mut reader = SegmentReader::open(path)?;
        let segment_base = reader.header().base_lsn;
        if let Some(prev_base) = last_segment_base {
            if segment_base <= prev_base {
                return Err(WalError::Malformed(format!(
                    "segment base_lsn {} is not greater than previous base_lsn {} ({})",
                    segment_base.raw(),
                    prev_base.raw(),
                    path.display()
                )));
            }
        }
        if !last_lsn.is_zero() && segment_base <= last_lsn {
            return Err(WalError::Malformed(format!(
                "segment base_lsn {} is not greater than previous record lsn {} ({})",
                segment_base.raw(),
                last_lsn.raw(),
                path.display()
            )));
        }
        last_segment_base = Some(segment_base);

        loop {
            // Capture position before the read so we can report the
            // start-of-bad-record offset on torn tail.
            let before = reader.position();
            match reader.read_record() {
                Ok(Some(record)) => {
                    let lsn = record.lsn();
                    if lsn < segment_base {
                        return Err(WalError::Malformed(format!(
                            "record lsn {} is below segment base_lsn {} ({})",
                            lsn.raw(),
                            segment_base.raw(),
                            path.display()
                        )));
                    }
                    if !last_lsn.is_zero() && lsn <= last_lsn {
                        return Err(WalError::Malformed(format!(
                            "record lsn {} is not greater than previous lsn {} ({})",
                            lsn.raw(),
                            last_lsn.raw(),
                            path.display()
                        )));
                    }
                    last_lsn = lsn;
                    if lsn > max_lsn {
                        max_lsn = lsn;
                    }
                    if lsn.raw() <= checkpoint_lsn.raw() {
                        // Already in the snapshot. Markers below the
                        // fence still need to keep their pending
                        // bucket clean, but no checkpoint can split
                        // a transaction (the store write lock is held
                        // during checkpoint), so dropping events outright
                        // is safe.
                        if let WalRecord::TxCommit { tx_begin_lsn, .. }
                        | WalRecord::TxAbort { tx_begin_lsn, .. } = &record
                        {
                            pending.remove(tx_begin_lsn);
                        }
                        continue;
                    }
                    match record {
                        WalRecord::Mutation {
                            tx_begin_lsn,
                            event,
                            ..
                        } => {
                            let events = pending.get_mut(&tx_begin_lsn).ok_or_else(|| {
                                WalError::Malformed(format!(
                                    "mutation at lsn {} references missing tx begin {}",
                                    lsn.raw(),
                                    tx_begin_lsn.raw()
                                ))
                            })?;
                            events.push(event);
                        }
                        WalRecord::MutationBatch {
                            tx_begin_lsn,
                            events: batch,
                            ..
                        } => {
                            let events = pending.get_mut(&tx_begin_lsn).ok_or_else(|| {
                                WalError::Malformed(format!(
                                    "mutation batch at lsn {} references missing tx begin {}",
                                    lsn.raw(),
                                    tx_begin_lsn.raw()
                                ))
                            })?;
                            events.extend(batch);
                        }
                        WalRecord::TxBegin { lsn } => {
                            // Materialise the bucket eagerly so
                            // begin-without-mutations transactions
                            // still get a deterministic commit/abort.
                            if pending.insert(lsn, Vec::new()).is_some() {
                                return Err(WalError::Malformed(format!(
                                    "duplicate tx begin at lsn {}",
                                    lsn.raw()
                                )));
                            }
                        }
                        WalRecord::TxCommit { tx_begin_lsn, .. } => {
                            let events = pending.remove(&tx_begin_lsn).ok_or_else(|| {
                                WalError::Malformed(format!(
                                    "commit at lsn {} references missing tx begin {}",
                                    lsn.raw(),
                                    tx_begin_lsn.raw()
                                ))
                            })?;
                            committed.extend(events);
                        }
                        WalRecord::TxAbort { tx_begin_lsn, .. } => {
                            pending.remove(&tx_begin_lsn).ok_or_else(|| {
                                WalError::Malformed(format!(
                                    "abort at lsn {} references missing tx begin {}",
                                    lsn.raw(),
                                    tx_begin_lsn.raw()
                                ))
                            })?;
                        }
                        WalRecord::Checkpoint { snapshot_lsn, .. } => {
                            if snapshot_lsn > lsn {
                                return Err(WalError::Malformed(format!(
                                    "checkpoint at lsn {} points to future snapshot lsn {}",
                                    lsn.raw(),
                                    snapshot_lsn.raw()
                                )));
                            }
                            if let Some(prev) = checkpoint_lsn_observed {
                                if snapshot_lsn < prev {
                                    return Err(WalError::Malformed(format!(
                                        "checkpoint snapshot lsn {} regressed below previous checkpoint {}",
                                        snapshot_lsn.raw(),
                                        prev.raw()
                                    )));
                                }
                            }
                            checkpoint_lsn_observed = Some(snapshot_lsn);
                        }
                    }
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

    // Any transaction still in `pending` at end-of-log was started but
    // never committed (and never explicitly aborted). Treat as crashed
    // mid-query and discard.
    drop(pending);

    Ok(ReplayOutcome {
        committed_events: committed,
        max_lsn,
        torn_tail,
        checkpoint_lsn_observed,
    })
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
