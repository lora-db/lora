//! Online copies of a named database ([`Database::snapshot_to_dir`]) and
//! the explicit checkpoint ([`Database::checkpoint`]).
//!
//! A named database lives in its `.loradb` container plus, while it is
//! open, a work directory the WAL appends to. Copying those files while
//! the database runs does not give a database: the WAL may be cut inside
//! a record, the container may lag the WAL, and the directory lock keeps
//! a second process from opening the live directory to copy it properly.
//!
//! A snapshot instead takes the graph as of one committed transaction
//! (under the writer lock, after the WAL is fsynced, as a checkpoint
//! does), streams it to a snapshot file beside the target, and writes a
//! container holding only that snapshot, fenced at the transaction's LSN.
//! Writers wait for the fsync and the graph handle, not for the copy: the
//! graph's storage is shared copy-on-write, so later writes copy what they
//! change and the snapshot keeps reading the version it took.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use lora_snapshot::SnapshotOptions;
use lora_store::{InMemoryGraph, SnapshotMeta};

use super::{snapshot_info_to_meta, write_graph_snapshot_file};
use crate::durable_io::sync_dir;
use crate::error::{LoraError, LoraErrorCode};
use crate::wal::archive::{staged_snapshot_in, staging_dir_for, write_snapshot_container};
use crate::Database;

/// What [`Database::snapshot_to_dir`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectorySnapshot {
    /// The `.loradb` container written under the target directory, at the
    /// database's own relative path (`app` writes `<target>/app.loradb`).
    pub path: PathBuf,
    /// Size of that container in bytes.
    pub bytes: u64,
    /// The WAL position the snapshot is taken at: it holds every
    /// transaction committed at or below this LSN and none above it. A
    /// change feed batch with this `lsn` is the last write it contains.
    pub lsn: u64,
    pub node_count: u64,
    pub relationship_count: u64,
}

impl Database<InMemoryGraph> {
    /// Write a self-contained copy of this named database to `target_dir`,
    /// which must be missing or empty, while readers and writers keep
    /// running.
    ///
    /// The copy is the graph as of one committed transaction, the last one
    /// the WAL holds when the call starts, and opens with
    /// [`Self::open_named`] under the same name with `target_dir` as the
    /// database directory. Writers wait only while the WAL is fsynced and
    /// the graph is taken, not while it is written; memory stays bounded
    /// by the copy buffers, plus the parts of the graph writes change
    /// meanwhile, which the snapshot keeps reading in their old version.
    ///
    /// The container is written under a temporary name and renamed into
    /// place once it is complete and fsynced, so a crash or kill leaves
    /// the target without a `.loradb` (and a temporary file the next open
    /// of the target removes), never a partial one. `target_dir` may be on
    /// another filesystem.
    ///
    /// Fails with `LORA_CONFIG` on a database not opened with
    /// [`Self::open_named`], and `LORA_VALIDATION` when `target_dir` holds
    /// anything.
    pub fn snapshot_to_dir(
        &self,
        target_dir: impl AsRef<Path>,
    ) -> Result<DirectorySnapshot, LoraError> {
        let (Some(wal), Some(archive)) = (&self.wal, &self.named_archive) else {
            return Err(LoraError::new(
                LoraErrorCode::Config,
                "a directory snapshot needs a named database; open it with a name and a database directory",
            ));
        };
        let target_dir = target_dir.as_ref();
        prepare_target_dir(target_dir)?;
        let target = target_dir.join(archive.relative_path());
        if let Some(parent) = target.parent() {
            create_dirs_durably(parent)?;
        }

        let (graph, lsn) = {
            let _writer = self
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            wal.force_fsync_wal_only()?;
            (self.store.load_full(), wal.wal().durable_lsn().raw())
        };

        let staging = StagingDir::create(staging_dir_for(&target))?;
        let info = write_graph_snapshot_file(
            &staged_snapshot_in(&staging.path),
            &graph,
            Some(lsn),
            &SnapshotOptions::default(),
        )?;
        drop(graph);
        write_snapshot_container(&staging.path, &target, archive.max_archive_bytes())?;
        drop(staging);

        Ok(DirectorySnapshot {
            bytes: fs::metadata(&target)?.len(),
            path: target,
            lsn,
            node_count: info.node_count as u64,
            relationship_count: info.relationship_count as u64,
        })
    }

    /// Take a checkpoint now: the snapshot recovery starts from becomes the
    /// current graph, and the WAL it covers is dropped.
    ///
    /// On a named database this is what [`Self::sync`] does (the container
    /// is rewritten as the snapshot and the WAL's tail); on one opened with
    /// [`Self::open_with_wal_snapshots`] it is
    /// [`Self::checkpoint_managed`]. Fails with `LORA_CONFIG` on a database
    /// with neither.
    pub fn checkpoint(&self) -> Result<SnapshotMeta, LoraError> {
        match (&self.wal, &self.named_archive, &self.snapshots) {
            (Some(wal), Some(archive), _) => {
                Ok(snapshot_info_to_meta(self.checkpoint_named(wal, archive)?))
            }
            (Some(_), None, Some(_)) => self.checkpoint_managed(),
            _ => Err(LoraError::new(
                LoraErrorCode::Config,
                "checkpoint needs a named database or a WAL with a snapshot directory",
            )),
        }
    }
}

/// Make `dir` an empty directory to snapshot into: create it when it is
/// missing, refuse it when it holds anything.
fn prepare_target_dir(dir: &Path) -> Result<(), LoraError> {
    match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(LoraError::new(
                    LoraErrorCode::Validation,
                    format!(
                        "snapshot target `{}` is not empty; pass a missing or empty directory",
                        dir.display()
                    ),
                ));
            }
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => create_dirs_durably(dir),
        Err(err) => Err(err.into()),
    }
}

/// `fs::create_dir_all`, with each new directory's entry fsynced.
fn create_dirs_durably(dir: &Path) -> Result<(), LoraError> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    if let Some(parent) = dir.parent() {
        create_dirs_durably(parent)?;
    }
    match fs::create_dir(dir) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => return Ok(()),
        Err(err) => return Err(err.into()),
    }
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        sync_dir(parent)?;
    }
    Ok(())
}

/// The scratch directory a snapshot is staged in; removed on drop.
struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    fn create(path: PathBuf) -> Result<Self, LoraError> {
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
