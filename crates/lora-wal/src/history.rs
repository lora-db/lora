//! Forward reader over committed WAL transactions.
//!
//! Recovery ([`crate::replay`]) flattens every committed transaction into
//! one event stream. Change-feed consumers need the transaction boundaries
//! and the commit LSN of each transaction instead, so they can hand out a
//! resume token per commit. [`CommittedTxReader`] walks the same segments
//! lazily, one committed transaction at a time, and stops at an upper LSN
//! bound so it can run against a WAL that is still being appended to.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lora_store::MutationEvent;

use crate::dir::SegmentDir;
use crate::errors::WalError;
use crate::lsn::Lsn;
use crate::record::WalRecord;
use crate::segment::SegmentReader;

/// One committed transaction as read back from the log.
#[derive(Debug, Clone, PartialEq)]
pub struct CommittedTx {
    /// LSN of the `TxCommit` record. Strictly increasing across
    /// transactions.
    pub commit_lsn: Lsn,
    /// Every mutation the transaction committed, in append order.
    pub events: Vec<MutationEvent>,
}

/// Lazily yields committed transactions whose `TxBegin` LSN is above
/// `after` and whose commit LSN is at or below `upto`.
///
/// Records past `upto` are never decoded, so the reader is safe to use
/// while a writer keeps appending to the active segment: everything at or
/// below `upto` was written to the OS before `upto` was observed.
pub struct CommittedTxReader {
    paths: Vec<PathBuf>,
    next_path: usize,
    reader: Option<SegmentReader>,
    after: Lsn,
    upto: Lsn,
    pending: BTreeMap<Lsn, Vec<MutationEvent>>,
    done: bool,
}

impl CommittedTxReader {
    pub fn open(dir: &Path, after: Lsn, upto: Lsn) -> Result<Self, WalError> {
        let paths = SegmentDir::new(dir)
            .list()?
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        Ok(Self {
            paths,
            next_path: 0,
            reader: None,
            after,
            upto,
            pending: BTreeMap::new(),
            done: after >= upto,
        })
    }

    /// Next committed transaction, or `None` once `upto` is reached or the
    /// log ends.
    pub fn next_tx(&mut self) -> Result<Option<CommittedTx>, WalError> {
        while !self.done {
            let Some(record) = self.next_record()? else {
                self.done = true;
                break;
            };
            let lsn = record.lsn();
            if lsn >= self.upto {
                self.done = true;
            }
            if lsn > self.upto {
                break;
            }
            match record {
                WalRecord::TxBegin { lsn } if lsn > self.after => {
                    self.pending.insert(lsn, Vec::new());
                }
                WalRecord::Mutation {
                    tx_begin_lsn,
                    event,
                    ..
                } => {
                    if let Some(events) = self.pending.get_mut(&tx_begin_lsn) {
                        events.push(event);
                    }
                }
                WalRecord::MutationBatch {
                    tx_begin_lsn,
                    events,
                    ..
                } => {
                    if let Some(pending) = self.pending.get_mut(&tx_begin_lsn) {
                        pending.extend(events);
                    }
                }
                WalRecord::TxCommit { lsn, tx_begin_lsn } => {
                    if let Some(events) = self.pending.remove(&tx_begin_lsn) {
                        return Ok(Some(CommittedTx {
                            commit_lsn: lsn,
                            events,
                        }));
                    }
                }
                WalRecord::TxAbort { tx_begin_lsn, .. } => {
                    self.pending.remove(&tx_begin_lsn);
                }
                WalRecord::TxBegin { .. } | WalRecord::Checkpoint { .. } => {}
            }
        }
        Ok(None)
    }

    fn next_record(&mut self) -> Result<Option<WalRecord>, WalError> {
        loop {
            if self.reader.is_none() {
                let Some(path) = self.paths.get(self.next_path) else {
                    return Ok(None);
                };
                self.next_path += 1;
                self.reader = Some(SegmentReader::open(path)?);
            }
            let reader = self.reader.as_mut().expect("reader was just opened");
            match reader.read_record()? {
                Some(record) => return Ok(Some(record)),
                None => self.reader = None,
            }
        }
    }
}

/// Lowest LSN the WAL in `dir` still holds a record for: the `base_lsn` of
/// its oldest segment. `None` when the directory has no segments.
///
/// Every record at or above this LSN is retained; everything below it was
/// truncated after a checkpoint.
pub fn oldest_retained_lsn(dir: &Path) -> Result<Option<Lsn>, WalError> {
    let entries = SegmentDir::new(dir).list()?;
    match entries.first() {
        Some(first) => Ok(Some(SegmentDir::base_lsn(&first.path)?)),
        None => Ok(None),
    }
}
