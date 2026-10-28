//! Automatic checkpoint by WAL growth.
//!
//! Without it a long-running process keeps every WAL segment until someone
//! calls `sync()` or a checkpoint, or reopens the database. The WAL counts
//! the bytes committed since the last checkpoint
//! ([`lora_wal::Wal::bytes_since_checkpoint`]) and calls a growth listener
//! after a commit that crosses the threshold. The listener only starts a
//! thread; the thread checkpoints the way `sync()` does on a named database
//! and the way [`ManagedSnapshotStore::checkpoint_behind_writers`] does for
//! managed snapshots: writers wait only while the WAL is fsynced and the
//! graph and its LSN are taken, never while the snapshot is written. The
//! listener runs on a committing thread, but by then the commit is in the
//! WAL, and the checkpoint thread needs the writer lock, so it starts after
//! the write is published. A failed checkpoint is logged, never returned:
//! the WAL still holds every commit, and the next checkpoint is attempted
//! once the WAL has grown by the threshold again.
//!
//! Each checkpoint rewrites the whole snapshot, so the threshold grows with
//! the current snapshot: the effective trigger is
//! `max(checkpoint_wal_bytes, snapshot bytes / SNAPSHOT_FRACTION)`, refreshed
//! at open and after every automatic checkpoint. Checkpoint I/O then stays
//! at most about `SNAPSHOT_FRACTION` times the WAL bytes written, whatever
//! the graph size; with a fixed threshold a bulk load into a large graph
//! would rewrite the full snapshot back to back. The WAL also bounds replay
//! on open, which runs on one thread at about 85 MB/s while a snapshot
//! loads on every core, so the fraction keeps it to a quarter of the
//! snapshot rather than a whole one.
//!
//! A WAL with nowhere to put a snapshot (`Database::open_with_wal`,
//! `Database::recover`) cannot be checkpointed automatically; it logs one
//! warning when it crosses the threshold.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;

use lora_store::InMemoryGraph;
use lora_wal::WalRecorder;

use crate::database::checkpoint_named;
use crate::error::LoraError;
use crate::live_store::LiveStore;
use crate::snapshot::{ManagedSnapshotStore, DEFAULT_CHECKPOINT_WAL_BYTES};
use crate::wal::archive::WalArchive;

/// The WAL may grow by `1 / SNAPSHOT_FRACTION` of the snapshot before an
/// automatic checkpoint (or, past the open-time minimum, an open) rewrites it.
pub(crate) const SNAPSHOT_FRACTION: u64 = 4;

/// Where an automatic checkpoint writes its snapshot.
pub(crate) enum CheckpointTarget {
    /// No snapshot location: only warn.
    None,
    Managed(Arc<ManagedSnapshotStore>),
    Named(Arc<WalArchive>),
}

pub(crate) struct AutoCheckpoint {
    /// Configured size.
    threshold: u64,
    /// `max(threshold, snapshot bytes / SNAPSHOT_FRACTION)`, the size the WAL
    /// listens for.
    effective: AtomicU64,
    store: Arc<LiveStore<InMemoryGraph>>,
    writer: Arc<Mutex<()>>,
    // Declared before `target`, so the WAL is dropped before a named
    // database's archive, whose drop writes the container from the
    // flushed WAL.
    wal: Arc<WalRecorder>,
    target: CheckpointTarget,
    /// A checkpoint thread is running.
    running: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl AutoCheckpoint {
    /// Install the growth listener on `wal`. `threshold` 0 installs nothing.
    /// For a WAL without a snapshot location the warning threshold is
    /// [`DEFAULT_CHECKPOINT_WAL_BYTES`].
    pub(crate) fn install(
        threshold: u64,
        target: CheckpointTarget,
        store: Arc<LiveStore<InMemoryGraph>>,
        writer: Arc<Mutex<()>>,
        wal: Arc<WalRecorder>,
    ) -> Option<Arc<Self>> {
        let threshold = match target {
            CheckpointTarget::None => DEFAULT_CHECKPOINT_WAL_BYTES,
            _ => threshold,
        };
        if threshold == 0 {
            return None;
        }
        let auto = Arc::new(Self {
            threshold,
            effective: AtomicU64::new(threshold),
            store,
            writer,
            wal,
            target,
            running: AtomicBool::new(false),
            worker: Mutex::new(None),
        });
        // Weak: the WAL owns the listener, and the listener must not keep
        // the WAL (through `auto.wal`) alive.
        let weak: Weak<Self> = Arc::downgrade(&auto);
        auto.wal.wal().set_growth_listener(
            threshold,
            Some(Arc::new(move || {
                if let Some(auto) = weak.upgrade() {
                    auto.on_growth();
                }
            })),
        );
        auto.refresh_threshold();
        Some(auto)
    }

    /// Bytes of the snapshot a checkpoint would replace.
    fn snapshot_bytes(&self) -> u64 {
        match &self.target {
            CheckpointTarget::None => 0,
            CheckpointTarget::Managed(snapshots) => snapshots.current_snapshot_bytes(),
            CheckpointTarget::Named(archive) => archive.snapshot_bytes(),
        }
    }

    /// Listen for `max(threshold, snapshot bytes / SNAPSHOT_FRACTION)` of
    /// growth.
    pub(crate) fn refresh_threshold(&self) {
        let effective = self
            .threshold
            .max(self.snapshot_bytes() / SNAPSHOT_FRACTION);
        self.effective.store(effective, Ordering::Relaxed);
        self.wal.wal().set_growth_threshold(effective);
    }

    fn on_growth(self: &Arc<Self>) {
        if matches!(self.target, CheckpointTarget::None) {
            // Once: stop listening.
            self.wal.wal().set_growth_listener(0, None);
            tracing::warn!(
                dir = %self.wal.wal().dir().display(),
                bytes = self.effective.load(Ordering::Relaxed),
                "the WAL has grown past the automatic checkpoint size, but this database has no \
                 snapshot location, so every WAL segment is kept until a checkpoint_to() or a \
                 restart from a snapshot; open it with managed snapshots or as a named database \
                 to checkpoint automatically"
            );
            return;
        }
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        // Count from here, so the listener stays quiet while this runs and
        // a failing checkpoint is retried after the next `threshold` bytes.
        self.wal.wal().mark_checkpoint();
        let auto = self.clone();
        let spawned = std::thread::Builder::new()
            .name("lora-checkpoint".into())
            .spawn(move || {
                auto.run();
                auto.running.store(false, Ordering::Release);
            });
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        match spawned {
            Ok(handle) => {
                // The previous thread has finished (`running` was false).
                if let Some(done) = worker.replace(handle) {
                    let _ = done.join();
                }
            }
            Err(err) => {
                self.running.store(false, Ordering::Release);
                tracing::warn!(error = %err, "could not start the automatic checkpoint thread");
            }
        }
    }

    fn run(&self) {
        let result = match &self.target {
            CheckpointTarget::None => Ok(()),
            CheckpointTarget::Managed(snapshots) => snapshots
                .checkpoint_behind_writers(&self.writer, &self.store, &self.wal)
                .map(|_| ())
                .map_err(LoraError::from_anyhow),
            CheckpointTarget::Named(archive) => {
                checkpoint_named(archive, &self.writer, &self.wal, &self.store).map(|_| ())
            }
        };
        // The snapshot just written (or kept) sets the next trigger.
        self.refresh_threshold();
        if let Err(err) = result {
            tracing::warn!(
                dir = %self.wal.wal().dir().display(),
                error = %err,
                "automatic checkpoint by WAL size failed; the WAL keeps every commit and the \
                 checkpoint is retried once it has grown by the threshold again"
            );
        }
    }

    /// Stop listening and wait for a running checkpoint. Called when the
    /// database is dropped.
    pub(crate) fn shutdown(&self) {
        self.wal.wal().set_growth_listener(0, None);
        let handle = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}
