use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Context, Result};
use lora_snapshot::{read_snapshot, write_snapshot, SnapshotOptions};
use lora_store::{InMemoryGraph, SnapshotMeta};
use lora_wal::{Lsn, WalRecorder};

const CURRENT_FILE: &str = "CURRENT";
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
}

impl SnapshotConfig {
    pub fn enabled(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            checkpoint_every_commits: None,
            keep_old: 1,
            codec: SnapshotOptions::default(),
        }
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
}

impl ManagedSnapshotStore {
    pub(crate) fn open(config: SnapshotConfig) -> Result<Self> {
        fs::create_dir_all(&config.dir)
            .with_context(|| format!("create snapshot dir {}", config.dir.display()))?;
        Ok(Self {
            config,
            commits_since_checkpoint: AtomicU64::new(0),
        })
    }

    pub(crate) fn load_latest(&self, graph: &mut InMemoryGraph) -> Result<Lsn> {
        let Some(path) = self.latest_snapshot_path()? else {
            return Ok(Lsn::ZERO);
        };
        let file =
            File::open(&path).with_context(|| format!("open snapshot {}", path.display()))?;
        let (payload, info) =
            read_snapshot(BufReader::new(file), self.config.codec.encryption.as_ref())
                .with_context(|| format!("load snapshot {}", path.display()))?;
        graph.load_snapshot_payload(payload)?;
        Ok(info.wal_lsn.map(Lsn::new).unwrap_or(Lsn::ZERO))
    }

    pub(crate) fn checkpoint(
        &self,
        graph: &InMemoryGraph,
        recorder: &WalRecorder,
    ) -> Result<SnapshotMeta> {
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync before managed snapshot failed: {e}"))?;
        let snapshot_lsn = recorder.wal().durable_lsn();
        let meta = self.write_snapshot(graph, snapshot_lsn)?;

        recorder
            .checkpoint_marker(snapshot_lsn)
            .map_err(|e| anyhow!("WAL checkpoint marker failed: {e}"))?;
        recorder
            .force_fsync()
            .map_err(|e| anyhow!("WAL fsync after checkpoint marker failed: {e}"))?;
        let _ = recorder.truncate_up_to(snapshot_lsn);

        self.commits_since_checkpoint.store(0, Ordering::Relaxed);
        self.prune_old_snapshots(snapshot_lsn)?;
        Ok(meta)
    }

    pub(crate) fn observe_commit(
        &self,
        graph: &InMemoryGraph,
        recorder: &WalRecorder,
    ) -> Result<()> {
        let Some(every) = self.config.checkpoint_every_commits else {
            return Ok(());
        };
        let commits = self
            .commits_since_checkpoint
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        if commits >= every {
            self.checkpoint(graph, recorder)?;
        }
        Ok(())
    }

    fn write_snapshot(&self, graph: &InMemoryGraph, snapshot_lsn: Lsn) -> Result<SnapshotMeta> {
        let target = snapshot_path(&self.config.dir, snapshot_lsn);
        let tmp = tmp_path(&target);
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("open temp snapshot {}", tmp.display()))?;
        let mut writer = BufWriter::new(file);
        let payload = graph.snapshot_payload();
        let info = write_snapshot(
            &mut writer,
            &payload,
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
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);

        fs::rename(&tmp, &target)
            .with_context(|| format!("rename {} to {}", tmp.display(), target.display()))?;
        sync_dir(&self.config.dir);
        write_current(&self.config.dir, &target)?;
        Ok(meta)
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
        sync_dir(&self.config.dir);
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
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, &current)
        .with_context(|| format!("rename {} to {}", tmp.display(), current.display()))?;
    sync_dir(dir);
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

fn sync_dir(path: &Path) {
    if let Ok(dir) = File::open(path) {
        let _ = dir.sync_all();
    }
}
