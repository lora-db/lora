//! Constructors for [`Database<InMemoryGraph>`] and [`Database<S>`].
//!
//! Five public entry points live here:
//!
//! * [`Database::in_memory`] — fresh empty in-memory database.
//! * [`Database::open_with_wal`] — open or create a WAL-backed database.
//! * [`Database::open_with_wal_snapshots`] — same, with managed snapshots.
//! * [`Database::open_named`] — open a portable `.loradb` archive.
//! * [`Database::recover`] — restore from a snapshot then replay the WAL.
//! * [`Database::new`] / [`Database::from_graph`] — build from a generic store.
//!
//! All four WAL paths share the same "install recorder, assemble
//! Database" tail; that is captured in the private
//! [`Database::from_graph_with_wal`] helper to keep the public
//! constructors focused on the recovery decisions specific to each
//! entry point.

use std::any::Any;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex};

use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, MutationRecorder};
use lora_wal::{replay_dir, Lsn, Wal, WalConfig, WalMirror, WalRecorder};

use crate::database::Database;
use crate::error::{LoraError, LoraErrorCode};
use crate::live_store::LiveStore;
use crate::named::{DatabaseName, DatabaseOpenOptions};
use crate::plan_cache::PlanCache;
use crate::snapshot::{ManagedSnapshotStore, SnapshotConfig};
use crate::wal::archive::WalArchive;

use super::replay::replay_into;

impl Database<InMemoryGraph> {
    /// Convenience constructor: a fresh, empty in-memory graph database.
    pub fn in_memory() -> Self {
        Self::from_graph(InMemoryGraph::new())
    }

    /// Open or create a WAL-enabled in-memory database from a fresh
    /// graph.
    ///
    /// `WalConfig::Disabled` falls back to [`Database::in_memory`].
    /// Otherwise, opens the WAL directory, replays any committed
    /// events into a fresh graph, installs a [`WalRecorder`] on the
    /// graph, and returns a database ready to serve queries.
    ///
    /// To restore from a snapshot in addition to the WAL, use
    /// [`Database::recover`] instead.
    pub fn open_with_wal(wal_config: WalConfig) -> Result<Self, LoraError> {
        match wal_config {
            WalConfig::Disabled => Ok(Self::in_memory()),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                let mut graph = InMemoryGraph::new();
                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, Lsn::ZERO)?;
                replay_into(&mut graph, events).map_err(LoraError::from_anyhow)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                Ok(Self::from_graph_with_wal(graph, recorder, None))
            }
        }
    }

    /// Open or create a WAL-backed database with managed snapshots beside it.
    ///
    /// Recovery loads the newest managed snapshot first, then replays WAL
    /// records above the snapshot's LSN fence. Checkpoints are written through
    /// [`Self::checkpoint_managed`] / [`Self::sync`], or automatically when
    /// `snapshot_config.checkpoint_every_commits` is set.
    pub fn open_with_wal_snapshots(
        wal_config: WalConfig,
        snapshot_config: SnapshotConfig,
    ) -> Result<Self, LoraError> {
        let snapshot_store =
            Arc::new(ManagedSnapshotStore::open(snapshot_config).map_err(LoraError::from_anyhow)?);
        let mut graph = InMemoryGraph::new();

        match wal_config {
            WalConfig::Disabled => Err(LoraError::new(
                LoraErrorCode::Config,
                "managed snapshots require WAL enabled",
            )),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                let snapshot_lsn = snapshot_store
                    .load_latest(&mut graph)
                    .map_err(LoraError::from_anyhow)?;
                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, snapshot_lsn)?;
                replay_into(&mut graph, events).map_err(LoraError::from_anyhow)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                Ok(Self::from_graph_with_wal(
                    graph,
                    recorder,
                    Some(snapshot_store),
                ))
            }
        }
    }

    /// Open or create a named portable database rooted under
    /// `options.database_dir`.
    ///
    /// The database name may be either a portable basename (`app` or
    /// `app.loradb`) or a safe relative path (`tenant/app`). It is resolved
    /// under `options.database_dir` before the WAL archive backend opens.
    pub fn open_named(
        database_name: impl AsRef<str>,
        options: DatabaseOpenOptions,
    ) -> Result<Self, LoraError> {
        let name = DatabaseName::parse(database_name.as_ref())?;
        let archive = Arc::new(WalArchive::open(
            options.database_path_for(&name),
            options.max_database_bytes,
        )?);
        let mut graph = InMemoryGraph::new();
        let (wal, events) = Wal::open(
            archive.work_dir(),
            options.sync_mode,
            options.segment_target_bytes,
            Lsn::ZERO,
        )?;
        replay_into(&mut graph, events).map_err(LoraError::from_anyhow)?;
        let mirror: Arc<dyn WalMirror> = archive;
        let recorder = Arc::new(WalRecorder::new_with_mirror(wal, Some(mirror)));
        // Mark the archive dirty so a fresh named database is materialized as
        // a portable ZIP. The archive writer coalesces this with any immediate
        // follow-up writes and flushes it in the background, with a final flush
        // on database drop.
        recorder.flush()?;
        Ok(Self::from_graph_with_wal(graph, recorder, None))
    }

    /// Restore from a snapshot file then replay any WAL records past
    /// it.
    ///
    /// The snapshot's `wal_lsn` (when set) becomes the replay fence —
    /// events at or below that LSN are already represented in the
    /// loaded snapshot and are skipped. A missing snapshot file is
    /// treated as "fresh start" so operators can pass the same path
    /// on every boot.
    ///
    /// If the WAL contains a checkpoint marker newer than the
    /// snapshot's `wal_lsn`, a one-line warning is printed to stderr
    /// — the snapshot is stale relative to a more recent checkpoint
    /// the operator is presumably aware of. Recovery still proceeds
    /// from the snapshot's fence (replay re-applies every record
    /// above it, which is conservative-correct); a tighter contract
    /// is deferred to v2 because verifying that the marker's
    /// snapshot file actually exists and is loadable is a separate
    /// observability concern.
    pub fn recover(
        snapshot_path: impl AsRef<Path>,
        wal_config: WalConfig,
    ) -> Result<Self, LoraError> {
        let snapshot_path = snapshot_path.as_ref();
        let mut graph = InMemoryGraph::new();
        let snapshot_lsn = match File::open(snapshot_path) {
            Ok(f) => {
                let reader = BufReader::new(f);
                let (payload, info) = crate::snapshot::read_snapshot_from(reader, None)?;
                graph.load_snapshot_payload(payload)?;
                info.wal_lsn.map(Lsn::new).unwrap_or(Lsn::ZERO)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lsn::ZERO,
            Err(e) => return Err(e.into()),
        };

        match wal_config {
            WalConfig::Disabled => Ok(Self::from_graph(graph)),
            WalConfig::Enabled {
                dir,
                sync_mode,
                segment_target_bytes,
            } => {
                // Diagnostic peek at the WAL's newest checkpoint
                // marker so we can warn the operator about a stale
                // snapshot before we start replaying. Treat any error
                // as "no marker" — the subsequent `Wal::open` will
                // surface the real failure if there is one.
                if dir.exists() {
                    if let Ok(outcome) = replay_dir(&dir, Lsn::ZERO) {
                        if let Some(marker) = outcome.checkpoint_lsn_observed {
                            if marker > snapshot_lsn {
                                eprintln!(
                                    "lora-wal: snapshot at LSN {} is older than the newest \
                                     checkpoint marker on disk (LSN {}). Replaying every WAL \
                                     record above LSN {}; consider passing the more recent \
                                     snapshot to --restore-from.",
                                    snapshot_lsn.raw(),
                                    marker.raw(),
                                    snapshot_lsn.raw()
                                );
                            }
                        }
                    }
                }

                let (wal, events) = Wal::open(dir, sync_mode, segment_target_bytes, snapshot_lsn)?;
                replay_into(&mut graph, events).map_err(LoraError::from_anyhow)?;
                let recorder = Arc::new(WalRecorder::new(wal));
                Ok(Self::from_graph_with_wal(graph, recorder, None))
            }
        }
    }

    /// Install the durable recorder on `graph` and assemble the
    /// `Database` envelope. Shared by every WAL-backed constructor.
    fn from_graph_with_wal(
        mut graph: InMemoryGraph,
        recorder: Arc<WalRecorder>,
        snapshots: Option<Arc<ManagedSnapshotStore>>,
    ) -> Self {
        graph.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));
        Self {
            store: Arc::new(LiveStore::new(Arc::new(graph))),
            writer: Arc::new(Mutex::new(())),
            lock_table: Arc::new(lora_store::LockTable::new()),
            wal: Some(recorder),
            snapshots,
            plan_cache: Arc::new(PlanCache::new()),
        }
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Build a database from a pre-wrapped, shared store.
    pub(crate) fn new(store: Arc<LiveStore<S>>) -> Self {
        Self {
            store,
            writer: Arc::new(Mutex::new(())),
            lock_table: Arc::new(lora_store::LockTable::new()),
            wal: None,
            snapshots: None,
            plan_cache: Arc::new(PlanCache::new()),
        }
    }

    /// Build a database by taking ownership of a bare graph store.
    pub fn from_graph(graph: S) -> Self {
        Self::new(Arc::new(LiveStore::new(Arc::new(graph))))
    }
}
