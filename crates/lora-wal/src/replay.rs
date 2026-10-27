//! WAL replay: walk segments, surface only committed mutation events,
//! and report the byte position of any torn tail so the caller can
//! truncate the active segment cleanly.
//!
//! The walk is deliberately a single forward sweep over the segments in
//! ascending id order. Per-transaction events are buffered in a
//! `BTreeMap<tx_begin_lsn, _>` and handed to the caller's sink on the
//! corresponding [`WalRecord::TxCommit`], so replay holds the open
//! transactions, never the whole log; an [`WalRecord::TxAbort`] (or a
//! missing commit at end-of-log) causes the bucket to be dropped without
//! emission. This is the contract the WAL plan calls "per-mutation log,
//! per-query commit": the recorder writes one record per primitive
//! mutation but replay only ever reapplies whole queries.
//!
//! A transaction is buffered only up to [`PENDING_BUDGET_BYTES`] of its
//! encoded records. Past that its events go to the sink as they are read
//! ([`ReplayStep::Partial`]), and the sink applies them provisionally: the
//! commit confirms them, and an abort, a missing commit or a record of
//! another transaction withdraws them ([`ReplayStep::Discard`]). An open
//! then holds one buffer's worth of a large transaction instead of all of
//! it decoded, which can be several times the graph it builds.
//!
//! Replay walks past `checkpoint_lsn`: any record whose LSN is at or
//! below it is already represented in the loaded snapshot and is
//! skipped. Because checkpoints are taken with the store write lock
//! held, no transaction can straddle the fence — `tx_begin_lsn <=
//! checkpoint_lsn` implies the matching commit (if any) is also at or
//! below it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use lora_store::MutationEvent;

use crate::dir::SegmentDir;
use crate::errors::WalError;
use crate::lsn::Lsn;
use crate::record::{RecordKind, WalRecord, RECORD_HEADER_LEN, RECORD_TRAILER_LEN};
use crate::segment::{SegmentReader, SEGMENT_HEADER_LEN};

/// Outcome of a full replay walk. The committed transactions went to the
/// sink as they were read.
#[derive(Debug)]
pub struct ReplayOutcome {
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

    /// Where replay stopped at corruption instead of failing, when it was
    /// asked to (`truncate_corrupt_tail`). Everything from this point on
    /// was not replayed; the caller cuts the log here.
    pub corrupt_cut: Option<CorruptCut>,
}

/// A corrupt point at which an open with `truncate_corrupt_tail` cut the
/// log: the segment was truncated at `offset`, the bytes after it were
/// copied to `kept_tail`, and every later segment was renamed to the
/// matching path in `moved_segments`. Nothing is deleted.
#[derive(Debug, Clone)]
pub struct CorruptCut {
    pub segment_path: PathBuf,
    pub offset: u64,
    /// The [`WalError::Corrupt`] reason replay found at `offset`.
    pub reason: String,
    /// Copy of the segment's bytes from `offset` to its end, or `None`
    /// when there were none.
    pub kept_tail: Option<PathBuf>,
    /// Later segments moved out of the log, as `(from, to)`.
    pub moved_segments: Vec<(PathBuf, PathBuf)>,
}

#[derive(Debug)]
pub struct TornTailInfo {
    pub segment_path: PathBuf,
    pub last_good_offset: u64,
    pub cause: WalError,
}

/// How much of one transaction replay buffers, in encoded record bytes,
/// before it hands the transaction's events to the sink as they arrive.
pub const PENDING_BUDGET_BYTES: usize = 4 * 1024 * 1024;

/// What replay hands its sink, in log order.
#[derive(Debug)]
pub enum ReplayStep {
    /// A committed transaction: its events, or the rest of them after
    /// [`Self::Partial`] steps.
    Commit(Vec<MutationEvent>),
    /// Events of a transaction larger than [`PENDING_BUDGET_BYTES`] whose
    /// commit has not been read yet. Apply them provisionally: a
    /// [`Self::Commit`] or a [`Self::Discard`] follows, with no step of
    /// another transaction in between.
    Partial(Vec<MutationEvent>),
    /// The transaction of the preceding [`Self::Partial`] steps did not
    /// commit: undo them.
    Discard,
}

/// Collects replayed events into one list: a sink body for callers that
/// want the whole log.
#[derive(Default)]
pub(crate) struct CollectEvents {
    pub(crate) events: Vec<MutationEvent>,
    /// Where the provisional events of a streaming transaction start.
    provisional_from: Option<usize>,
}

impl CollectEvents {
    pub(crate) fn step(&mut self, step: ReplayStep) {
        match step {
            ReplayStep::Commit(events) => {
                self.events.extend(events);
                self.provisional_from = None;
            }
            ReplayStep::Partial(events) => {
                self.provisional_from.get_or_insert(self.events.len());
                self.events.extend(events);
            }
            ReplayStep::Discard => {
                if let Some(from) = self.provisional_from.take() {
                    self.events.truncate(from);
                }
            }
        }
    }
}

/// Receives the committed transactions, in commit order (see
/// [`ReplayStep`]). An error stops the replay and is returned from it.
pub type ReplaySink<'a> = dyn FnMut(ReplayStep) -> Result<(), WalError> + 'a;

/// Walk every segment in `paths` (already in ascending id order), handing
/// each committed transaction to `sink` as its commit is read.
///
/// Records with `lsn <= checkpoint_lsn` are skipped — they are already
/// captured in the snapshot the caller is about to restore from.
///
/// A bad frame is a torn tail only when it is in the newest segment, that
/// segment is not sealed, and no valid record follows it: that is the
/// shape an interrupted append leaves. Anything else is corruption and
/// fails with [`WalError::Corrupt`] rather than being cut off, because
/// cutting it would silently drop committed transactions. A log whose
/// oldest segment starts above `checkpoint_lsn + 1`, or that is missing a
/// segment in the middle, fails with [`WalError::Gap`].
///
/// With `cut_corruption`, a [`WalError::Corrupt`] point ends the replay
/// instead of failing it: the outcome's `corrupt_cut` names it, and only
/// the transactions committed before it reached `sink`.
pub(crate) fn replay_segments(
    paths: &[PathBuf],
    checkpoint_lsn: Lsn,
    cut_corruption: bool,
    sink: &mut ReplaySink<'_>,
) -> Result<ReplayOutcome, WalError> {
    check_segment_ids_contiguous(paths)?;
    let mut state = ReplayState::new(sink);
    let mut torn_tail: Option<TornTailInfo> = None;
    let mut corrupt_cut: Option<CorruptCut> = None;
    let mut last_good_offset = SEGMENT_HEADER_LEN as u64;

    'outer: for (index, path) in paths.iter().enumerate() {
        let mut reader = SegmentReader::open(path)?;
        last_good_offset = reader.position();
        let segment_base = reader.header().base_lsn;
        if index == 0 {
            check_log_start(segment_base, checkpoint_lsn, path)?;
        }
        state.validate_segment(segment_base, path)?;
        let may_have_torn_tail = index + 1 == paths.len() && !reader.header().sealed;

        loop {
            // Capture position before the read so we can report the
            // start-of-bad-record offset on torn tail.
            let before = reader.position();
            match reader.read_frame() {
                Ok(Some(raw)) => {
                    // The frame's CRC matched, so these are the bytes that
                    // were written: a payload that does not decode is never
                    // a torn tail.
                    let encoded_len = raw.encoded_len();
                    let record = match raw.decode() {
                        Ok(record) => record,
                        Err(err) => {
                            let err = corrupt(path, before, err.to_string());
                            corrupt_cut = Some(cut_or_fail(err, cut_corruption)?);
                            last_good_offset = before;
                            break 'outer;
                        }
                    };
                    state.accept_record(record, encoded_len, segment_base, checkpoint_lsn, path)?;
                    last_good_offset = reader.position();
                }
                Ok(None) => break,
                Err(err @ WalError::Io(_)) => return Err(err),
                Err(cause) => {
                    if !may_have_torn_tail {
                        let place = if reader.header().sealed {
                            "a sealed segment"
                        } else {
                            "a segment that is not the newest"
                        };
                        let err = corrupt(path, before, format!("{cause} in {place}"));
                        corrupt_cut = Some(cut_or_fail(err, cut_corruption)?);
                        last_good_offset = before;
                        break 'outer;
                    }
                    if let Some(at) = reader.find_valid_frame_after(before, state.last_lsn)? {
                        let err = corrupt(
                            path,
                            before,
                            format!(
                                "{cause}; a valid record follows at byte {at}, \
                                 so this is not a torn tail"
                            ),
                        );
                        corrupt_cut = Some(cut_or_fail(err, cut_corruption)?);
                        last_good_offset = before;
                        break 'outer;
                    }
                    torn_tail = Some(TornTailInfo {
                        segment_path: path.clone(),
                        last_good_offset: before,
                        cause,
                    });
                    break 'outer;
                }
            }
        }
    }

    state.finish(torn_tail, corrupt_cut, last_good_offset)
}

/// `err` (a [`WalError::Corrupt`]) as the point to cut the log at, when
/// the caller asked for that, or `err` itself.
fn cut_or_fail(err: WalError, cut_corruption: bool) -> Result<CorruptCut, WalError> {
    match err {
        WalError::Corrupt {
            path,
            offset,
            reason,
        } if cut_corruption => Ok(CorruptCut {
            segment_path: path,
            offset,
            reason,
            kept_tail: None,
            moved_segments: Vec::new(),
        }),
        err => Err(err),
    }
}

fn corrupt(path: &Path, offset: u64, reason: String) -> WalError {
    WalError::Corrupt {
        path: path.to_path_buf(),
        offset,
        reason,
    }
}

/// Segment ids are allocated one at a time and only ever removed from the
/// front, so a hole between the first and last id means a segment file
/// was lost.
fn check_segment_ids_contiguous(paths: &[PathBuf]) -> Result<(), WalError> {
    for pair in paths.windows(2) {
        let (Some(prev), Some(next)) = (SegmentDir::id_of(&pair[0]), SegmentDir::id_of(&pair[1]))
        else {
            continue;
        };
        if prev.checked_next() != Some(next) {
            return Err(WalError::Gap(format!(
                "segment `{}` is followed by `{}`; the segments between them are missing",
                pair[0].display(),
                pair[1].display()
            )));
        }
    }
    Ok(())
}

/// The oldest segment must start at or below `checkpoint_lsn + 1`:
/// everything before it has to be in the snapshot the caller loaded.
/// Checkpoints remove only segments whose records are all at or below
/// their fence, so a later start means the caller is replaying without
/// the snapshot those segments were folded into.
fn check_log_start(base: Lsn, checkpoint_lsn: Lsn, path: &Path) -> Result<(), WalError> {
    if base.raw().saturating_sub(1) > checkpoint_lsn.raw() {
        return Err(WalError::Gap(format!(
            "the oldest segment `{}` starts at LSN {} but the snapshot covers only up to \
             LSN {}; open with the snapshot taken when the older segments were removed",
            path.display(),
            base.raw(),
            checkpoint_lsn.raw()
        )));
    }
    Ok(())
}

/// A transaction read but not yet committed or aborted.
#[derive(Default)]
struct PendingTx {
    events: Vec<MutationEvent>,
    /// Encoded size of the records behind `events`.
    bytes: usize,
}

struct ReplayState<'s, 'a> {
    pending: BTreeMap<Lsn, PendingTx>,
    /// The transaction whose events went to the sink as
    /// [`ReplayStep::Partial`] steps, until its commit or discard.
    streaming: Option<Lsn>,
    /// Transactions withdrawn with [`ReplayStep::Discard`] because a
    /// record of another one came between theirs. Only a log written
    /// through the low-level `begin`/`append`/`commit` primitives can
    /// interleave transactions; one of these that commits fails replay.
    withdrawn: Vec<Lsn>,
    sink: &'s mut ReplaySink<'a>,
    max_lsn: Lsn,
    last_lsn: Lsn,
    last_segment_base: Option<Lsn>,
    checkpoint_lsn_observed: Option<Lsn>,
}

impl<'s, 'a> ReplayState<'s, 'a> {
    fn new(sink: &'s mut ReplaySink<'a>) -> Self {
        Self {
            pending: BTreeMap::new(),
            streaming: None,
            withdrawn: Vec::new(),
            sink,
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
        encoded_len: usize,
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

        self.apply_record(record, encoded_len, lsn)
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

    fn apply_record(
        &mut self,
        record: WalRecord,
        encoded_len: usize,
        lsn: Lsn,
    ) -> Result<(), WalError> {
        match record {
            WalRecord::Mutation {
                tx_begin_lsn,
                event,
                ..
            } => self.add_events(tx_begin_lsn, lsn, "mutation", encoded_len, |events| {
                events.push(event)
            })?,
            WalRecord::MutationBatch {
                tx_begin_lsn,
                events: batch,
                ..
            } => self.add_events(tx_begin_lsn, lsn, "mutation batch", encoded_len, |events| {
                events.extend(batch)
            })?,
            WalRecord::TxBegin { lsn } => {
                self.withdraw_other_than(lsn)?;
                self.begin_transaction(lsn)?;
            }
            WalRecord::TxCommit { tx_begin_lsn, .. } => {
                self.withdraw_other_than(tx_begin_lsn)?;
                let pending = self.take_pending(tx_begin_lsn, lsn, "commit")?;
                if self.withdrawn.contains(&tx_begin_lsn) {
                    return Err(WalError::Malformed(format!(
                        "transaction {} commits at lsn {} after records of another \
                         transaction came between its own, past the replay buffer",
                        tx_begin_lsn.raw(),
                        lsn.raw()
                    )));
                }
                self.streaming = None;
                (self.sink)(ReplayStep::Commit(pending.events))?;
            }
            WalRecord::TxAbort { tx_begin_lsn, .. } => {
                let _ = self.take_pending(tx_begin_lsn, lsn, "abort")?;
                if self.streaming == Some(tx_begin_lsn) {
                    self.streaming = None;
                    (self.sink)(ReplayStep::Discard)?;
                }
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
        if self.pending.insert(lsn, PendingTx::default()).is_some() {
            return Err(WalError::Malformed(format!(
                "duplicate tx begin at lsn {}",
                lsn.raw()
            )));
        }
        Ok(())
    }

    /// Buffer a mutation record's events with its transaction, and hand
    /// them on as a [`ReplayStep::Partial`] once the transaction has
    /// outgrown [`PENDING_BUDGET_BYTES`].
    fn add_events(
        &mut self,
        tx_begin_lsn: Lsn,
        record_lsn: Lsn,
        kind: &str,
        encoded_len: usize,
        add: impl FnOnce(&mut Vec<MutationEvent>),
    ) -> Result<(), WalError> {
        self.withdraw_other_than(tx_begin_lsn)?;
        let pending = self
            .pending
            .get_mut(&tx_begin_lsn)
            .ok_or_else(|| missing_tx_begin(kind, record_lsn, tx_begin_lsn))?;
        add(&mut pending.events);
        pending.bytes = pending.bytes.saturating_add(encoded_len);
        if pending.bytes >= PENDING_BUDGET_BYTES && !self.withdrawn.contains(&tx_begin_lsn) {
            let events = std::mem::take(&mut pending.events);
            pending.bytes = 0;
            self.streaming = Some(tx_begin_lsn);
            (self.sink)(ReplayStep::Partial(events))?;
        }
        Ok(())
    }

    /// A record of a transaction other than the one streaming to the sink
    /// came: withdraw the streaming one, whose events the sink would
    /// otherwise hold provisionally under the other's.
    fn withdraw_other_than(&mut self, tx_begin_lsn: Lsn) -> Result<(), WalError> {
        match self.streaming {
            Some(streaming) if streaming != tx_begin_lsn => {
                self.streaming = None;
                self.withdrawn.push(streaming);
                (self.sink)(ReplayStep::Discard)
            }
            _ => Ok(()),
        }
    }

    fn take_pending(
        &mut self,
        tx_begin_lsn: Lsn,
        record_lsn: Lsn,
        kind: &str,
    ) -> Result<PendingTx, WalError> {
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

    fn finish(
        self,
        torn_tail: Option<TornTailInfo>,
        corrupt_cut: Option<CorruptCut>,
        last_good_offset: u64,
    ) -> Result<ReplayOutcome, WalError> {
        // Any transaction still in `pending` at end-of-log was started
        // but never committed (and never explicitly aborted). Treat as
        // crashed mid-query and discard.
        if self.streaming.is_some() {
            (self.sink)(ReplayStep::Discard)?;
        }
        Ok(ReplayOutcome {
            max_lsn: self.max_lsn,
            torn_tail,
            checkpoint_lsn_observed: self.checkpoint_lsn_observed,
            last_good_offset,
            corrupt_cut,
        })
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
/// the numeric prefix encoded in the file name, dropping the events.
///
/// Used by recovery diagnostics in `lora-database::Database::recover`
/// to peek at the newest checkpoint marker before opening the live
/// `Wal` handle.
pub fn replay_dir(dir: &Path, checkpoint_lsn: Lsn) -> Result<ReplayOutcome, WalError> {
    let entries = SegmentDir::new(dir).list()?;
    let paths: Vec<PathBuf> = entries.into_iter().map(|e| e.path).collect();
    replay_segments(&paths, checkpoint_lsn, false, &mut |_| Ok(()))
}

/// Newest checkpoint marker in the WAL at `dir`: the `snapshot_lsn` of the
/// last [`WalRecord::Checkpoint`] record, or `None` when there is none.
///
/// A cheap stand-in for `replay_dir(dir, Lsn::ZERO).checkpoint_lsn_observed`
/// for diagnostics: it reads only each record's fixed header and skips the
/// payload, and checks the CRC of checkpoint records only. It does not
/// validate the log; a segment it cannot follow is read up to that point.
pub fn newest_checkpoint_marker(dir: &Path) -> Result<Option<Lsn>, WalError> {
    let mut newest = None;
    for entry in SegmentDir::new(dir).list()? {
        let file = File::open(&entry.path)?;
        let len = file.metadata()?.len();
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        reader.seek(SeekFrom::Start(SEGMENT_HEADER_LEN as u64))?;
        let mut position = SEGMENT_HEADER_LEN as u64;
        let mut header = [0u8; RECORD_HEADER_LEN];
        while position + RECORD_HEADER_LEN as u64 <= len {
            reader.read_exact(&mut header)?;
            let length = u64::from(u32::from_le_bytes([
                header[0], header[1], header[2], header[3],
            ]));
            if length < (RECORD_HEADER_LEN + RECORD_TRAILER_LEN) as u64 || position + length > len {
                break;
            }
            if header[4] == RecordKind::Checkpoint as u8 {
                if length != (RECORD_HEADER_LEN + RECORD_TRAILER_LEN) as u64 {
                    break;
                }
                let mut crc = [0u8; RECORD_TRAILER_LEN];
                reader.read_exact(&mut crc)?;
                if crc32fast::hash(&header) != u32::from_le_bytes(crc) {
                    break;
                }
                let mut snapshot_lsn = [0u8; 8];
                snapshot_lsn.copy_from_slice(&header[13..21]);
                newest = Some(Lsn::new(u64::from_le_bytes(snapshot_lsn)));
            } else {
                let rest = i64::try_from(length - RECORD_HEADER_LEN as u64)
                    .map_err(|_| WalError::Decode("record length overflows i64".into()))?;
                reader.seek_relative(rest)?;
            }
            position += length;
        }
    }
    Ok(newest)
}
