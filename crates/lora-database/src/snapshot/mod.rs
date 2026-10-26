//! Snapshot integration for [`Database`]. Owns:
//!
//! * the byte-level entry points (`save_snapshot_to_*`, `load_snapshot_*`,
//!   `checkpoint_to`, `checkpoint_managed`, `recover`-side helpers),
//! * the JSON option/credential adapters used by the language bindings,
//! * the [`SnapshotByteFormat`] sniff and the [`SnapshotAdmin`] trait,
//! * filesystem hygiene helpers (`TempFileGuard`, `snapshot_tmp_path`,
//!   `sync_parent_dir`).
//!
//! Every byte-level path here goes through the columnar `lora-snapshot`
//! codec exclusively; the legacy in-store `LORASNAP` format was retired.
//! `lora-store` now owns only the [`SnapshotPayload`] vocabulary, this
//! module owns the encode/decode integration.

mod json;
pub(crate) mod store;

pub use json::{snapshot_credentials_from_json, snapshot_options_from_json};
pub(crate) use store::ManagedSnapshotStore;
pub use store::{SnapshotConfig, DEFAULT_CHECKPOINT_WAL_BYTES};

use std::fs::OpenOptions;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;

use lora_snapshot::{
    decode_snapshot as decode_database_snapshot, read_graph_snapshot, write_graph_snapshot,
    write_snapshot as write_database_snapshot, Compression, SnapshotCodecError,
    SnapshotCredentials, SnapshotInfo, SnapshotOptions, DATABASE_SNAPSHOT_MAGIC,
};
use lora_store::{InMemoryGraph, MutationRecorder, SnapshotMeta, SnapshotPayload};
use lora_wal::WalRecorder;

use crate::durable_io::{sync_dir, sync_file};
use crate::error::{LoraError, LoraErrorCode};
use crate::Database;

/// Magic-byte sniff for snapshot bytes. The legacy in-store `LORASNAP`
/// codec was removed in favour of the columnar `lora-snapshot` format,
/// so this collapses to a single recognized variant; kept as a typed
/// detect for forward compatibility if a future format is introduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotByteFormat {
    Database,
}

impl SnapshotByteFormat {
    pub fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(DATABASE_SNAPSHOT_MAGIC) {
            Some(Self::Database)
        } else {
            None
        }
    }
}

pub(crate) fn snapshot_info_to_meta(info: SnapshotInfo) -> SnapshotMeta {
    SnapshotMeta {
        format_version: info.format_version,
        node_count: info.node_count,
        relationship_count: info.relationship_count,
        wal_lsn: info.wal_lsn,
    }
}

// ---------------------------------------------------------------------------
// Filesystem hygiene helpers
// ---------------------------------------------------------------------------

static SNAPSHOT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Scratch path beside `target`, unique per process and call so
/// concurrent saves to one target never write the same file.
pub(crate) fn snapshot_tmp_path(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        SNAPSHOT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(tmp)
}

pub(crate) fn sync_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    Ok(sync_dir(parent)?)
}

/// RAII handle that deletes its path on drop unless [`commit`] is called.
///
/// The snapshot save path creates `<target>.tmp` before the payload is
/// written; if any step between then and the final rename fails (or the
/// thread unwinds), the guard's `Drop` removes the scratch file so a crashed
/// save never leaves leftovers on disk.
///
/// [`commit`]: Self::commit
pub(crate) struct TempFileGuard {
    path: Option<PathBuf>,
}

impl TempFileGuard {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Disarm the guard. Call this once the tmp file's contents have been
    /// handed off (e.g. renamed to their final destination) so the `Drop`
    /// impl does not try to remove them.
    pub(crate) fn commit(mut self) {
        self.path.take();
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            // Best-effort: cleanup failure is not worth surfacing — the
            // worst case is a leaked scratch file that the next save
            // overwrites via `OpenOptions::truncate(true)`.
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Decode columnar snapshot bytes into a payload + info. Returns the
/// underlying typed [`SnapshotCodecError`] so callers can `?` it
/// straight into a [`LoraError`] via the `From` impl.
pub(crate) fn decode_snapshot_bytes(
    bytes: &[u8],
    credentials: Option<&SnapshotCredentials>,
) -> Result<(SnapshotPayload, SnapshotInfo), SnapshotCodecError> {
    decode_database_snapshot(bytes, credentials)
}

/// Write a snapshot of `graph` to `path` (created or truncated) as a
/// stream, then fsync it. The caller renames it into place.
pub(crate) fn write_graph_snapshot_file(
    path: &Path,
    graph: &InMemoryGraph,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<SnapshotInfo, LoraError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    let info = write_graph_snapshot(&mut file, graph, wal_lsn, options)?;
    sync_file(&file)?;
    Ok(info)
}

/// Decode the snapshot file at `path` straight into a graph.
pub(crate) fn read_graph_snapshot_file(
    path: &Path,
    credentials: Option<&SnapshotCredentials>,
) -> Result<(InMemoryGraph, SnapshotInfo), LoraError> {
    let file = std::fs::File::open(path)?;
    Ok(read_graph_snapshot(BufReader::new(file), credentials)?)
}

/// Encode a payload through the columnar codec.
pub(crate) fn encode_snapshot_to<W: Write>(
    writer: W,
    payload: &SnapshotPayload,
    wal_lsn: Option<u64>,
    options: &SnapshotOptions,
) -> Result<SnapshotInfo, SnapshotCodecError> {
    write_database_snapshot(writer, payload, wal_lsn, options)
}

// ---------------------------------------------------------------------------
// Database<InMemoryGraph> snapshot surface
// ---------------------------------------------------------------------------

impl Database<InMemoryGraph> {
    /// Serialize the current graph state to `path` using the default
    /// columnar codec options (uncompressed, unencrypted). Writes are
    /// atomic via a unique `<path>.<pid>.<n>.tmp` + rename + parent-dir
    /// fsync.
    ///
    /// On a WAL-backed database the WAL is fsynced first and its durable
    /// LSN is stamped into the snapshot as the fence, so
    /// [`Database::recover`] from this file replays only the writes made
    /// after it. Unlike [`Self::checkpoint_to`] it appends no checkpoint
    /// marker and truncates no WAL segments.
    ///
    /// Callers that need compression or encryption should reach for
    /// [`Self::save_snapshot_to_with_options`] directly.
    pub fn save_snapshot_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta, LoraError> {
        let options = SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        };
        self.save_snapshot_to_with_options(path, &options)
    }

    /// Replace the current graph state with a snapshot loaded from `path`.
    /// Decodes via the columnar codec; encrypted snapshots require
    /// [`Self::load_snapshot_from_with_credentials`] instead.
    pub fn load_snapshot_from(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta, LoraError> {
        self.load_snapshot_from_with_credentials(path, None)
    }

    /// Convenience constructor: open (or create) an empty in-memory database
    /// and immediately restore it from `path`. Errors if the file cannot be
    /// opened or the snapshot is malformed.
    pub fn in_memory_from_snapshot(path: impl AsRef<Path>) -> Result<Self, LoraError> {
        let db = Self::in_memory();
        db.load_snapshot_from_with_credentials(path, None)?;
        Ok(db)
    }

    /// Serialize the current graph state into the database snapshot byte
    /// format.
    ///
    /// This uses the column-oriented `lora-snapshot` codec, the same one
    /// driven by `save_snapshot_to_with_options`. On a WAL-backed database
    /// the bytes carry the WAL fence, as a checkpoint does, so
    /// `Database::recover` replays only the writes made after the save.
    /// The default is uncompressed so bytes stay portable across native
    /// and WASM builds; callers that want a specific codec can use
    /// [`Self::save_snapshot_to_bytes_with_options`].
    pub fn save_snapshot_to_bytes(&self) -> Result<Vec<u8>, LoraError> {
        let options = SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        };
        let (bytes, _) = self.save_snapshot_to_bytes_with_options(&options)?;
        Ok(bytes)
    }

    /// Serialize the current graph state into database snapshot bytes with
    /// explicit codec options.
    pub fn save_snapshot_to_bytes_with_options(
        &self,
        options: &SnapshotOptions,
    ) -> Result<(Vec<u8>, SnapshotInfo), LoraError> {
        let (graph, wal_lsn) = self.graph_with_wal_fence()?;
        let payload = graph.snapshot_payload();
        let mut bytes = Vec::new();
        let info = encode_snapshot_to(&mut bytes, &payload, wal_lsn, options)?;
        Ok((bytes, info))
    }

    /// Serialize the current graph state to a database snapshot file with
    /// explicit codec options. This is the path form of
    /// [`Self::save_snapshot_to_bytes_with_options`] and supports the same
    /// compression and encryption options.
    pub fn save_snapshot_to_with_options(
        &self,
        path: impl AsRef<Path>,
        options: &SnapshotOptions,
    ) -> Result<SnapshotMeta, LoraError> {
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);
        let (graph, wal_lsn) = self.graph_with_wal_fence()?;

        let tmp_guard = TempFileGuard::new(tmp.clone());
        let info = write_graph_snapshot_file(&tmp, &graph, wal_lsn, options)?;
        drop(graph);

        std::fs::rename(&tmp, path)?;
        tmp_guard.commit();

        sync_parent_dir(path).map_err(|e| LoraError::new(LoraErrorCode::Io, e.to_string()))?;

        Ok(snapshot_info_to_meta(info))
    }

    /// The published graph and, on a WAL-backed database, the WAL fence
    /// that covers it: the durable LSN of the last record the graph
    /// contains. Both are taken under the writer lock after a WAL fsync,
    /// so `Database::recover` from a snapshot of this graph replays
    /// exactly the records after it. Writers wait only for the fsync,
    /// not while the snapshot is encoded.
    ///
    /// A database without a WAL has no fence. Neither has a poisoned
    /// WAL: its graph may not match the log any more, so the snapshot is
    /// written unfenced, as a seed for a fresh WAL directory.
    fn graph_with_wal_fence(&self) -> Result<(Arc<InMemoryGraph>, Option<u64>), LoraError> {
        let Some(recorder) = &self.wal else {
            return Ok((self.read_store(), None));
        };
        let _writer = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match recorder.force_fsync_wal_only() {
            Ok(()) => Ok((
                self.store.load_full(),
                Some(recorder.wal().durable_lsn().raw()),
            )),
            Err(lora_wal::WalError::Poisoned) => {
                tracing::warn!(
                    reason = recorder.poisoned_reason().unwrap_or_default(),
                    "WAL is poisoned; saving an unfenced snapshot that must not be recovered over this WAL"
                );
                Ok((self.store.load_full(), None))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Replace the current graph state from snapshot bytes (columnar
    /// `lora-snapshot` format).
    pub fn load_snapshot_from_bytes(&self, bytes: &[u8]) -> Result<SnapshotMeta, LoraError> {
        self.load_snapshot_from_bytes_with_credentials(bytes, None)
    }

    /// Replace the current graph state from snapshot bytes, supplying
    /// credentials when loading an encrypted database snapshot.
    ///
    /// On a WAL-backed database the restore is made durable by a
    /// checkpoint before it becomes visible: into the managed snapshot
    /// directory, or the container of a named database. A WAL-backed
    /// database with neither refuses the load with `LORA_CONFIG`, since
    /// recovery would rebuild the pre-restore graph from its WAL.
    pub fn load_snapshot_from_bytes_with_credentials(
        &self,
        bytes: &[u8],
        credentials: Option<&SnapshotCredentials>,
    ) -> Result<SnapshotMeta, LoraError> {
        if SnapshotByteFormat::detect(bytes).is_none() {
            return Err(LoraError::new(
                LoraErrorCode::SnapshotCodec,
                "snapshot bytes have unrecognized magic",
            ));
        }
        let Some(recorder) = self.wal.clone() else {
            let mut guard = self.write_store();
            let (payload, info) = decode_snapshot_bytes(bytes, credentials)?;
            let meta = snapshot_info_to_meta(info);
            guard
                .staged_mut_or_error()?
                .load_snapshot_payload(payload)?;
            // Publish the staged graph atomically into the live store;
            // dropping the guard without `publish` would discard the restore
            // (rollback semantics on the writer lease).
            guard.publish_in_place();
            // Still under the writer lock: change feeds see the restore in
            // commit order.
            self.publish_reset();
            drop(guard);
            return Ok(meta);
        };

        // A restore writes no WAL records, so on a WAL-backed database it
        // is only durable as a checkpoint: the loaded graph becomes the
        // snapshot recovery starts from, fenced at the WAL's current end.
        if self.snapshots.is_none() && self.named_archive.is_none() {
            return Err(LoraError::new(
                LoraErrorCode::Config,
                "cannot load a snapshot into a WAL-backed database without a snapshot directory, because recovery would replay the old WAL instead; open it with managed snapshots or as a named database, or use `Database::recover` with the snapshot",
            ));
        }
        let (payload, info) = decode_snapshot_bytes(bytes, credentials)?;
        let meta = snapshot_info_to_meta(info);

        // Checkpoint lock before writer lock, as `sync()` takes them.
        let _checkpoint = match &self.named_archive {
            Some(archive) => Some(archive.checkpoint_lock()?),
            None => None,
        };
        let mut guard = self.write_store();
        let staged = guard.staged_mut_or_error()?;
        staged.load_snapshot_payload(payload)?;
        staged.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));

        let mut installed = false;
        let checkpointed =
            self.checkpoint_restored(guard.staged_or_error()?, &recorder, &mut installed);
        if let Err(err) = checkpointed {
            if installed {
                // Recovery may now start from the restored graph while the
                // live one is the old graph: stop writes until a reopen
                // settles which one is durable.
                recorder.poison(
                    "snapshot restore failed after its checkpoint was written; reopen the database",
                );
            }
            return Err(err);
        }
        guard.publish_in_place();
        self.publish_reset();
        drop(guard);
        Ok(meta)
    }

    /// Checkpoint a restored graph on a WAL-backed database, into the
    /// managed snapshot directory or the named database's container.
    /// Caller holds the writer lock (and the archive's checkpoint lock).
    fn checkpoint_restored(
        &self,
        graph: &InMemoryGraph,
        recorder: &WalRecorder,
        installed: &mut bool,
    ) -> Result<(), LoraError> {
        if let Some(snapshots) = &self.snapshots {
            snapshots
                .checkpoint_tracked(graph, recorder, installed)
                .map_err(LoraError::from_anyhow)?;
            return Ok(());
        }
        let Some(archive) = &self.named_archive else {
            return Ok(());
        };
        recorder.force_fsync_wal_only()?;
        let snapshot_lsn = recorder.wal().durable_lsn();
        let staged = archive.staged_snapshot_path();
        let staged_guard = TempFileGuard::new(staged.clone());
        write_graph_snapshot_file(
            &staged,
            graph,
            Some(snapshot_lsn.raw()),
            &SnapshotOptions::default(),
        )?;
        // From here the work directory may hold the restored graph.
        *installed = true;
        archive.install_snapshot(|| recorder.truncate_covered_by_mirror(snapshot_lsn))?;
        staged_guard.commit();
        Ok(())
    }

    /// Replace the current graph state from a database snapshot file,
    /// supplying credentials when the snapshot is encrypted.
    pub fn load_snapshot_from_with_credentials(
        &self,
        path: impl AsRef<Path>,
        credentials: Option<&SnapshotCredentials>,
    ) -> Result<SnapshotMeta, LoraError> {
        let bytes = std::fs::read(path.as_ref())?;
        self.load_snapshot_from_bytes_with_credentials(&bytes, credentials)
    }

    /// Take a checkpoint: snapshot the current state with the WAL's
    /// `durable_lsn` stamped into the header, append a `Checkpoint`
    /// marker to the WAL, then drop sealed segments at or below the
    /// fence.
    ///
    /// Errors with "checkpoint requires WAL enabled" when called on a
    /// database constructed without a WAL; [`Self::save_snapshot_to`]
    /// writes a snapshot without touching the WAL.
    ///
    /// The write-lock-held window covers snapshot serialization plus the
    /// checkpoint marker append. Truncation runs after the rename
    /// but still under the write lock; making it concurrent with queries
    /// is a v2 concern (see `docs/decisions/0004-wal.md`).
    pub fn checkpoint_to(&self, path: impl AsRef<Path>) -> Result<SnapshotMeta, LoraError> {
        let recorder = self.wal.as_ref().ok_or_else(|| {
            LoraError::new(LoraErrorCode::Internal, "checkpoint requires WAL enabled")
        })?;
        let path = path.as_ref();
        let tmp = snapshot_tmp_path(path);

        let guard = self.write_store();

        // Make every record appended so far durable, then capture
        // the LSN that becomes the snapshot fence.
        recorder.force_fsync()?;
        let snapshot_lsn = recorder.wal().durable_lsn();

        let tmp_guard = TempFileGuard::new(tmp.clone());
        let options = SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        };
        let info = write_graph_snapshot_file(
            &tmp,
            guard.staged_or_error()?,
            Some(snapshot_lsn.raw()),
            &options,
        )?;
        let meta = snapshot_info_to_meta(info);

        std::fs::rename(&tmp, path)?;
        tmp_guard.commit();

        sync_parent_dir(path).map_err(|e| LoraError::new(LoraErrorCode::Io, e.to_string()))?;

        // Append the checkpoint marker AFTER the rename succeeds —
        // this preserves the invariant that a `Checkpoint` record
        // in the WAL implies the snapshot it points at exists.
        recorder.checkpoint_marker(snapshot_lsn)?;
        recorder.force_fsync()?;

        // Best-effort segment truncation. Failure here doesn't undo
        // the checkpoint — the next call will retry.
        if let Err(err) = recorder.truncate_up_to(snapshot_lsn) {
            tracing::warn!(
                lsn = snapshot_lsn.raw(),
                error = %err,
                "WAL truncation after checkpoint failed; will retry later"
            );
        }

        Ok(meta)
    }

    /// Take a checkpoint into the managed snapshot directory configured by
    /// [`Self::open_with_wal_snapshots`].
    pub fn checkpoint_managed(&self) -> Result<SnapshotMeta, LoraError> {
        let recorder = self.wal.as_ref().ok_or_else(|| {
            LoraError::new(
                LoraErrorCode::Internal,
                "managed checkpoint requires WAL enabled",
            )
        })?;
        let snapshots = self.snapshots.as_ref().ok_or_else(|| {
            LoraError::new(
                LoraErrorCode::Internal,
                "managed checkpoint requires snapshots enabled",
            )
        })?;
        let guard = self.write_store();
        snapshots
            .checkpoint(guard.staged_or_error()?, recorder)
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// SnapshotAdmin — type-erased admin entry for transports.
// ---------------------------------------------------------------------------

/// Storage-agnostic admin surface for HTTP / binding callers that want to
/// drive snapshot operations without naming the backend type parameter.
///
/// Implemented on `Database<InMemoryGraph>` since the in-memory backend
/// is currently the only one that bridges to the columnar
/// `lora-snapshot` codec. Transports (e.g. `lora-server`) type-erase on
/// `Arc<dyn SnapshotAdmin>`.
pub trait SnapshotAdmin: Send + Sync + 'static {
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta, LoraError>;
    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta, LoraError>;
}

impl SnapshotAdmin for Database<InMemoryGraph> {
    fn save_snapshot(&self, path: &Path) -> Result<SnapshotMeta, LoraError> {
        self.save_snapshot_to(path)
    }

    fn load_snapshot(&self, path: &Path) -> Result<SnapshotMeta, LoraError> {
        self.load_snapshot_from(path)
    }
}
