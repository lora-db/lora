//! Change-feed history rebuilt from the write-ahead log.
//!
//! A feed that resumes from an LSN older than the in-memory retention
//! window replays the WAL. The WAL only holds primitive mutation events, so
//! producing enriched [`Change`]s (labels, property maps, deleted records)
//! needs the graph as it was at each commit. The replay therefore starts
//! from a *base*: an empty graph when the WAL still holds every record, or
//! a snapshot (managed checkpoint or `.loradb` container) whose LSN fence
//! is at or below the requested resume point with the WAL retained past it.
//! It applies every committed transaction up to the resume point silently,
//! then turns each later transaction into a [`ChangeBatch`].

use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use lora_store::InMemoryGraph;
use lora_wal::{oldest_retained_lsn, CommittedTxReader, Lsn};

use super::build::{build_changes, PreImages};
use super::ChangeBatch;
use crate::database::replay::replay_into;
use crate::error::{LoraError, LoraErrorCode};
use crate::snapshot::ManagedSnapshotStore;

/// Where a history replay starts.
pub(crate) enum HistoryBase {
    /// The WAL holds every record since the database was created.
    Empty,
    /// A managed checkpoint file on disk.
    ManagedSnapshot {
        store: Arc<ManagedSnapshotStore>,
        path: PathBuf,
        lsn: Lsn,
    },
    /// The snapshot of a `.loradb` container, opened when the replay was
    /// planned (a checkpoint may replace the file since) and decoded when
    /// it starts.
    Container { file: File, lsn: Lsn },
}

impl HistoryBase {
    fn lsn(&self) -> Lsn {
        match self {
            Self::Empty => Lsn::ZERO,
            Self::ManagedSnapshot { lsn, .. } | Self::Container { lsn, .. } => *lsn,
        }
    }
}

/// Inputs for choosing a base.
pub(crate) struct HistorySources {
    pub wal_dir: PathBuf,
    pub snapshots: Option<Arc<ManagedSnapshotStore>>,
    /// The container's snapshot file, when the database is a `.loradb`
    /// container that holds one.
    pub container: Option<PathBuf>,
}

/// A planned replay of `(after, upto]` from the WAL.
pub(crate) struct HistoryReplay {
    wal_dir: PathBuf,
    base: Option<HistoryBase>,
    after: Lsn,
    upto: Lsn,
    running: Option<Running>,
}

struct Running {
    graph: InMemoryGraph,
    reader: CommittedTxReader,
}

fn truncated(from: u64) -> LoraError {
    LoraError::new(
        LoraErrorCode::ChangesTruncated,
        format!(
            "change feed cannot resume from LSN {from} because the write-ahead log no longer holds that history; start a new feed without `fromLsn` and re-read current state"
        ),
    )
}

impl HistoryReplay {
    /// Pick the newest base that covers `after` and check the WAL still
    /// holds every record past it. Fails with `LORA_CHANGES_TRUNCATED` when
    /// no base qualifies.
    pub(crate) fn plan(sources: HistorySources, after: u64, upto: u64) -> Result<Self, LoraError> {
        let oldest = oldest_retained_lsn(&sources.wal_dir)?.ok_or_else(|| truncated(after))?;
        // Every record above `base` must still be on disk.
        let covers = |base: Lsn| oldest.raw() <= base.raw().saturating_add(1);

        let mut best: Option<HistoryBase> = None;
        let mut consider = |candidate: HistoryBase| {
            let lsn = candidate.lsn();
            if lsn.raw() > after || !covers(lsn) {
                return;
            }
            if best.as_ref().is_none_or(|b| b.lsn() < lsn) {
                best = Some(candidate);
            }
        };

        consider(HistoryBase::Empty);
        if let Some(store) = &sources.snapshots {
            for (lsn, path) in store
                .snapshot_files_at_or_below(Lsn::new(after))
                .map_err(LoraError::from_anyhow)?
            {
                consider(HistoryBase::ManagedSnapshot {
                    store: store.clone(),
                    path,
                    lsn,
                });
            }
        }
        if let Some(path) = sources.container {
            // Only the manifest is read here; the graph decodes when the
            // replay starts.
            let mut file = File::open(path)?;
            let info = lora_snapshot::read_snapshot_info(BufReader::new(&mut file))?;
            consider(HistoryBase::Container {
                file,
                lsn: Lsn::new(info.wal_lsn.unwrap_or(0)),
            });
        }

        let base = best.ok_or_else(|| truncated(after))?;
        Ok(Self {
            wal_dir: sources.wal_dir,
            base: Some(base),
            after: Lsn::new(after),
            upto: Lsn::new(upto),
            running: None,
        })
    }

    fn start(&mut self) -> Result<()> {
        let base = self
            .base
            .take()
            .ok_or_else(|| anyhow!("change feed history already started"))?;
        let base_lsn = base.lsn();
        let mut graph = InMemoryGraph::new();
        match base {
            HistoryBase::Empty => {}
            HistoryBase::ManagedSnapshot { store, path, .. } => {
                store.load_file_into(&path, &mut graph)?;
            }
            HistoryBase::Container { mut file, .. } => {
                file.seek(SeekFrom::Start(0))?;
                (graph, _) = lora_snapshot::read_graph_snapshot(BufReader::new(file), None)?;
            }
        }
        let reader = CommittedTxReader::open(&self.wal_dir, base_lsn, self.upto)?;
        self.running = Some(Running { graph, reader });
        Ok(())
    }

    /// Next historical batch, or `None` once `upto` is reached.
    /// `should_stop` is polled between transactions so a closed feed stops
    /// replaying promptly.
    pub(crate) fn next_batch(
        &mut self,
        should_stop: &dyn Fn() -> bool,
    ) -> Result<Option<ChangeBatch>, LoraError> {
        if self.running.is_none() {
            self.start().map_err(LoraError::from_anyhow)?;
        }
        let running = self.running.as_mut().expect("history replay started");
        loop {
            if should_stop() {
                return Ok(None);
            }
            let Some(tx) = running.reader.next_tx()? else {
                return Ok(None);
            };
            if tx.commit_lsn <= self.after {
                replay_into(&mut running.graph, tx.events).map_err(LoraError::from_anyhow)?;
                continue;
            }
            let pre = PreImages::capture(&tx.events, &running.graph);
            replay_into(&mut running.graph, tx.events.clone()).map_err(LoraError::from_anyhow)?;
            let changes = build_changes(&tx.events, &pre, &running.graph);
            if changes.is_empty() {
                continue;
            }
            return Ok(Some(ChangeBatch {
                lsn: tx.commit_lsn.raw(),
                changes,
            }));
        }
    }
}
