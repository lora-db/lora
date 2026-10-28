use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use anyhow::{anyhow, Context, Result};
use lora_snapshot::{read_graph_snapshot, write_graph_snapshot, SnapshotOptions};
use lora_store::{InMemoryGraph, SnapshotMeta};
use lora_wal::{Lsn, WalRecorder};

use crate::durable_io::{sync_dir, sync_file};
use crate::live_store::LiveStore;

const CURRENT_FILE: &str = "CURRENT";

/// Put a loaded graph in `graph`'s place, keeping its recorder.
fn replace_graph(graph: &mut InMemoryGraph, loaded: InMemoryGraph) {
    let recorder = graph.mutation_recorder().cloned();
    *graph = loaded;
    graph.set_mutation_recorder(recorder);
}
const SNAPSHOT_PREFIX: &str = "snapshot-";
const SNAPSHOT_SUFFIX: &str = ".lsnap";

#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    pub dir: PathBuf,
    /// When set, create a managed checkpoint after this many committed WAL
    /// transactions. `None` keeps checkpointing manual via `sync()` /
    /// `checkpoint_managed()`.
    pub checkpoint_every_commits: Option<u64>,
    /// Number of older checkpoint files to retain in addition to `CURRENT`.
    pub keep_old: usize,
    /// Columnar snapshot codec options. Defaults to fast gzip compression and
    /// no encryption.
    pub codec: SnapshotOptions,
    /// Create a managed checkpoint once the WAL has grown by this many bytes
    /// since the last one, or by a quarter of the current snapshot's size
    /// when that is larger, so a long-running process does not keep every
    /// WAL segment and checkpoint writes stay proportional to WAL growth.
    /// The checkpoint runs on a background thread; writers wait only while
    /// the graph and its LSN are taken. 0 turns it off.
    /// Defaults to [`DEFAULT_CHECKPOINT_WAL_BYTES`].
    pub checkpoint_wal_bytes: u64,
}

/// Default for [`SnapshotConfig::checkpoint_wal_bytes`] and
/// [`crate::DatabaseOpenOptions::checkpoint_wal_bytes`]: the WAL size at
/// which a named database's open compacts, as a checkpoint at runtime.
pub const DEFAULT_CHECKPOINT_WAL_BYTES: u64 = crate::named::DEFAULT_COMPACT_ON_OPEN_WAL_BYTES;

impl SnapshotConfig {
    pub fn enabled(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            checkpoint_every_commits: None,
            keep_old: 1,
            codec: SnapshotOptions::default(),
            checkpoint_wal_bytes: DEFAULT_CHECKPOINT_WAL_BYTES,
        }
    }

    /// Checkpoint once the WAL has grown by `bytes` since the last
    /// checkpoint; 0 turns it off. See [`Self::checkpoint_wal_bytes`].
    pub fn checkpoint_wal_bytes(mut self, bytes: u64) -> Self {
        self.checkpoint_wal_bytes = bytes;
        self
    }

    pub fn every_commits(mut self, commits: u64) -> Self {
        self.checkpoint_every_commits = Some(commits.max(1));
        self
    }

    pub fn keep_old(mut self, keep_old: usize) -> Self {
        self.keep_old = keep_old;
        self
    }

    pub fn codec(mut self, codec: SnapshotOptions) -> Self {
        self.codec = codec;
        self
    }
}

pub(crate) struct ManagedSnapshotStore {
    config: SnapshotConfig,
    commits_since_checkpoint: AtomicU64,
    /// Held for the whole of a checkpoint, so checkpoints install their
    /// snapshots one at a time. Holds the fence of the newest installed
    /// one: a checkpoint taken behind the writers skips installing a
    /// snapshot older than it, which would otherwise become `CURRENT` after
    /// a newer checkpoint dropped the WAL it needs.
    newest: Mutex<Lsn>,
}

impl ManagedSnapshotStore {
    pub(crate) fn open(config: SnapshotConfig) -> Result<Self> {
        fs::create_dir_all(&config.dir)
            .with_context(|| format!("create snapshot dir {}", config.dir.display()))?;
        Ok(Self {
            config,
            commits_since_checkpoint: AtomicU64::new(0),
            newest: Mutex::new(Lsn::ZERO),
        })
    }

    pub(crate) fn checkpoint_wal_bytes(&self) -> u64 {
        self.config.checkpoint_wal_bytes
    }

    fn lock_newest(&self) -> MutexGuard<'_, Lsn> {
        self.newest.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn load_latest(&self, graph: &mut InMemoryGraph) -> Result<Lsn> {
        let Some(path) = self.latest_snapshot_path()? else {
            return Ok(Lsn::ZERO);
        };
        let file =
            File::open(&path).with_context(|| format!("open snapshot {}", path.display()))?;
        let (loaded, info) =
            read_graph_snapshot(BufReader::new(file), self.config.codec.encryption.as_ref())
                .with_context(|| format!("load snapshot {}", path.display()))?;
        replace_graph(graph, loaded);
        let lsn = info.wal_lsn.map(Lsn::new).unwrap_or(Lsn::ZERO);
        *self.lock_newest() = lsn;
        Ok(lsn)
    }

    /// Managed snapshot files whose LSN fence is at or below `lsn`,
    /// newest first. Used by change-feed history replay to find a base.
    pub(crate) fn snapshot_files_at_or_below(&self, lsn: Lsn) -> Result<Vec<(Lsn, PathBuf)>> {
        let mut files = snapshot_files(&self.config.dir)?;
        files.retain(|(file_lsn, _)| *file_lsn <= lsn);
        files.sort_by_key(|(file_lsn, _)| std::cmp::Reverse(*file_lsn));
        Ok(files)
    }

    /// Load one managed snapshot file into `graph`, returning its LSN
    /// fence.
    pub(crate) fn load_file_into(&self, path: &Path, graph: &mut InMemoryGraph) -> Result<Lsn> {
        let file = File::open(path).with_context(|| format!("open snapshot {}", path.display()))?;
        let (loaded, info) =
            read_graph_snapshot(BufReader::new(file), self.config.codec.encryption.as_ref())
                .with_context(|| format!("load snapshot {}", path.display()))?;
        replace_graph(graph, loaded);
        Ok(info.wal_lsn.map(Lsn::new).unwrap_or(Lsn::ZERO))
    }

    pub(crate) fn checkpoint(
        &self,
        graph: &InMemoryGraph,
        recorder: &WalRecorder,
    ) -> Result<SnapshotMeta> {
        self.checkpoint_tracked(graph, recorder, &mut false)
    }

    /// [`Self::checkpoint`], setting `installed` once the new snapshot file
    /// is in place, from where recovery may load it. A failure with
    /// `installed` still `false` left the recovered state unchanged.
    ///
    /// The caller holds the writer lock, so `graph` is the graph at the
    /// WAL's end.
    pub(crate) fn checkpoint_tracked(
        &self,
        graph: &InMemoryGraph,
        recorder: &WalRecorder,
        installed: &mut bool,
    ) -> Result<SnapshotMeta> {
        let mut newest = self.lock_newest();
        self.checkpoint_locked(&mut newest, graph, recorder, installed)
    }

    fn checkpoint_locked(
        &self,
        newest: &mut Lsn,
        graph: &InMemoryGraph,
        recorder: &WalRecorder,
        installed: &mut bool,
    ) -> Result<SnapshotMeta> {
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync before managed snapshot failed: {e}"))?;
        let snapshot_lsn = recorder.wal().durable_lsn();
        self.install_locked(newest, graph, snapshot_lsn, recorder, installed)
    }

    /// Checkpoint without holding the writer lock while the snapshot is
    /// written: `writer` is taken only to fsync the WAL and take the graph
    /// with its LSN. Returns `None` when a checkpoint at or past that LSN
    /// was installed meanwhile. Used by the automatic checkpoint by WAL
    /// growth, from its own thread.
    pub(crate) fn checkpoint_behind_writers(
        &self,
        writer: &Mutex<()>,
        store: &LiveStore<InMemoryGraph>,
        recorder: &WalRecorder,
    ) -> Result<Option<SnapshotMeta>> {
        let (graph, snapshot_lsn): (Arc<InMemoryGraph>, Lsn) = {
            let _writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
            recorder
                .force_fsync()
                .map_err(|e| anyhow!("WAL fsync before managed snapshot failed: {e}"))?;
            (store.load_full(), recorder.wal().durable_lsn())
        };
        let mut newest = self.lock_newest();
        if *newest >= snapshot_lsn {
            return Ok(None);
        }
        self.install_locked(&mut newest, &graph, snapshot_lsn, recorder, &mut false)
            .map(Some)
    }

    /// Write `graph` as the checkpoint at `snapshot_lsn`, append its marker
    /// and drop the WAL it covers. The caller holds `newest`.
    fn install_locked(
        &self,
        newest: &mut Lsn,
        graph: &InMemoryGraph,
        snapshot_lsn: Lsn,
        recorder: &WalRecorder,
        installed: &mut bool,
    ) -> Result<SnapshotMeta> {
        let meta = self.write_snapshot(graph, snapshot_lsn, installed)?;
        *newest = snapshot_lsn;

        recorder
            .checkpoint_marker(snapshot_lsn)
            .map_err(|e| anyhow!("WAL checkpoint marker failed: {e}"))?;
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync after checkpoint marker failed: {e}"))?;
        if let Err(err) = recorder.truncate_up_to(snapshot_lsn) {
            tracing::warn!(
                lsn = snapshot_lsn.raw(),
                error = %err,
                "WAL truncation after managed checkpoint failed; will retry later"
            );
        }

        self.commits_since_checkpoint.store(0, Ordering::Relaxed);
        // The new snapshot is in place and `CURRENT` names it: a failed
        // cleanup of older files only leaves extra files behind.
        if let Err(err) = self.prune_old_snapshots(snapshot_lsn) {
            tracing::warn!(
                lsn = snapshot_lsn.raw(),
                error = %err,
                "removing old managed snapshots failed; will retry at the next checkpoint"
            );
        }
        Ok(meta)
    }

    /// Count one committed WAL transaction and checkpoint when the
    /// configured threshold is reached.
    ///
    /// Runs after the commit is durable and published, so it cannot fail
    /// the write: a failed checkpoint is logged and the count starts over,
    /// so a checkpoint that keeps failing is retried every
    /// `checkpoint_every_commits` commits rather than on every one. The
    /// WAL still holds every commit, so recovery is unaffected.
    pub(crate) fn observe_commit(&self, graph: &InMemoryGraph, recorder: &WalRecorder) {
        let Some(every) = self.config.checkpoint_every_commits else {
            return;
        };
        let commits = self
            .commits_since_checkpoint
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        if commits < every {
            return;
        }
        // A checkpoint running behind the writers holds the lock: skip
        // rather than make every writer wait for it; the count stays, so
        // the next commit tries again.
        let mut newest = match self.newest.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        if let Err(err) = self.checkpoint_locked(&mut newest, graph, recorder, &mut false) {
            self.commits_since_checkpoint.store(0, Ordering::Relaxed);
            tracing::warn!(
                dir = %self.config.dir.display(),
                error = %format!("{err:#}"),
                "automatic managed checkpoint failed; the WAL keeps every commit and the checkpoint is retried later"
            );
        }
    }

    fn write_snapshot(
        &self,
        graph: &InMemoryGraph,
        snapshot_lsn: Lsn,
        installed: &mut bool,
    ) -> Result<SnapshotMeta> {
        let target = snapshot_path(&self.config.dir, snapshot_lsn);
        let tmp = tmp_path(&target);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("open temp snapshot {}", tmp.display()))?;
        let info = write_graph_snapshot(
            &mut file,
            graph,
            Some(snapshot_lsn.raw()),
            &self.config.codec,
        )
        .map_err(|e| anyhow!("encode managed snapshot failed: {e}"))?;
        let meta = SnapshotMeta {
            format_version: info.format_version,
            node_count: info.node_count,
            relationship_count: info.relationship_count,
            wal_lsn: info.wal_lsn,
        };
        sync_file(&file)?;
        drop(file);

        fs::rename(&tmp, &target)
            .with_context(|| format!("rename {} to {}", tmp.display(), target.display()))?;
        // Without `CURRENT`, recovery picks the newest file in the dir.
        *installed = true;
        sync_dir(&self.config.dir)
            .with_context(|| format!("sync snapshot dir {}", self.config.dir.display()))?;
        write_current(&self.config.dir, &target)?;
        Ok(meta)
    }

    /// Size of the snapshot recovery would load, 0 without one.
    pub(crate) fn current_snapshot_bytes(&self) -> u64 {
        match self.latest_snapshot_path() {
            Ok(Some(path)) => fs::metadata(path).map_or(0, |m| m.len()),
            _ => 0,
        }
    }

    fn latest_snapshot_path(&self) -> Result<Option<PathBuf>> {
        let current = self.config.dir.join(CURRENT_FILE);
        match fs::read_to_string(&current) {
            Ok(name) => {
                let name = name.trim();
                if name.is_empty() {
                    return Ok(None);
                }
                let path = self.config.dir.join(name);
                if path.exists() {
                    return Ok(Some(path));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("read {}", current.display())),
        }

        let latest = snapshot_files(&self.config.dir)?
            .into_iter()
            .max_by_key(|(lsn, _)| *lsn)
            .map(|(_, path)| path);
        Ok(latest)
    }

    fn prune_old_snapshots(&self, current_lsn: Lsn) -> Result<()> {
        let mut snapshots = snapshot_files(&self.config.dir)?;
        snapshots.retain(|(lsn, _)| *lsn != current_lsn);
        snapshots.sort_by_key(|(lsn, _)| *lsn);
        let retain = self.config.keep_old;
        let remove_count = snapshots.len().saturating_sub(retain);
        for (_, path) in snapshots.into_iter().take(remove_count) {
            fs::remove_file(&path)
                .with_context(|| format!("remove old snapshot {}", path.display()))?;
        }
        sync_dir(&self.config.dir)
            .with_context(|| format!("sync snapshot dir {}", self.config.dir.display()))?;
        Ok(())
    }
}

fn snapshot_path(dir: &Path, lsn: Lsn) -> PathBuf {
    dir.join(format!(
        "{SNAPSHOT_PREFIX}{:020}{SNAPSHOT_SUFFIX}",
        lsn.raw()
    ))
}

fn snapshot_files(dir: &Path) -> Result<Vec<(Lsn, PathBuf)>> {
    let mut out = Vec::new();
    for entry in
        fs::read_dir(dir).with_context(|| format!("read snapshot dir {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(raw) = name
            .strip_prefix(SNAPSHOT_PREFIX)
            .and_then(|name| name.strip_suffix(SNAPSHOT_SUFFIX))
        else {
            continue;
        };
        if let Ok(lsn) = raw.parse::<u64>() {
            out.push((Lsn::new(lsn), path));
        }
    }
    Ok(out)
}

fn write_current(dir: &Path, target: &Path) -> Result<()> {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow!(
                "snapshot path has no portable filename: {}",
                target.display()
            )
        })?;
    let current = dir.join(CURRENT_FILE);
    let tmp = tmp_path(&current);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)
        .with_context(|| format!("open temp CURRENT {}", tmp.display()))?;
    writeln!(file, "{name}")?;
    sync_file(&file)?;
    drop(file);
    fs::rename(&tmp, &current)
        .with_context(|| format!("rename {} to {}", tmp.display(), current.display()))?;
    sync_dir(dir).with_context(|| format!("sync snapshot dir {}", dir.display()))?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}
