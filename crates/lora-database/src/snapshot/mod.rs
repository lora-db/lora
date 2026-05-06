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
pub use store::SnapshotConfig;

use std::fs::OpenOptions;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;

use lora_snapshot::{
    decode_snapshot as decode_database_snapshot, read_snapshot as read_database_snapshot,
    write_snapshot as write_database_snapshot, Compression, SnapshotCodecError,
    SnapshotCredentials, SnapshotInfo, SnapshotOptions, DATABASE_SNAPSHOT_MAGIC,
};
use lora_store::{InMemoryGraph, SnapshotMeta, SnapshotPayload};

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

pub(crate) fn snapshot_tmp_path(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".tmp");
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

/// Decode columnar snapshot bytes streamed from a reader. Used by
/// `Database::recover` at startup.
pub(crate) fn read_snapshot_from<R: Read>(
    reader: R,
    credentials: Option<&SnapshotCredentials>,
) -> Result<(SnapshotPayload, SnapshotInfo), SnapshotCodecError> {
    read_database_snapshot(reader, credentials)
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
    /// atomic via `<path>.tmp` + rename + parent-dir fsync.
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
    /// This uses the column-oriented `lora-snapshot` codec — the same one
    /// driven by `save_snapshot_to_with_options`, but without a WAL fence.
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
        let guard = self.read_store();
        let payload = guard.snapshot_payload();
        let mut bytes = Vec::new();
        let info = encode_snapshot_to(&mut bytes, &payload, None, options)?;
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
        let guard = self.read_store();

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);

        let payload = guard.snapshot_payload();
        let info = encode_snapshot_to(&mut writer, &payload, None, options)?;

        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        sync_file(&file)?;
        drop(file);

        std::fs::rename(&tmp, path)?;
        tmp_guard.commit();

        sync_parent_dir(path).map_err(|e| LoraError::new(LoraErrorCode::Io, e.to_string()))?;

        Ok(snapshot_info_to_meta(info))
    }

    /// Replace the current graph state from snapshot bytes (columnar
    /// `lora-snapshot` format).
    pub fn load_snapshot_from_bytes(&self, bytes: &[u8]) -> Result<SnapshotMeta, LoraError> {
        self.load_snapshot_from_bytes_with_credentials(bytes, None)
    }

    /// Replace the current graph state from snapshot bytes, supplying
    /// credentials when loading an encrypted database snapshot.
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
        let mut guard = self.write_store();
        let (payload, info) = decode_snapshot_bytes(bytes, credentials)?;
        let meta = snapshot_info_to_meta(info);
        guard.load_snapshot_payload(payload)?;
        // Publish the staged graph atomically into the live store; dropping
        // the guard without `publish` would discard the restore (rollback
        // semantics on the writer lease).
        guard.publish();
        Ok(meta)
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
    /// database constructed without a WAL — operators that just want
    /// a fence-less dump should use [`Self::save_snapshot_to`] instead.
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

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        let tmp_guard = TempFileGuard::new(tmp.clone());
        let mut writer = BufWriter::new(file);
        let payload = guard.snapshot_payload();
        let options = SnapshotOptions {
            compression: Compression::None,
            encryption: None,
        };
        let info = encode_snapshot_to(&mut writer, &payload, Some(snapshot_lsn.raw()), &options)?;
        let meta = snapshot_info_to_meta(info);

        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        sync_file(&file)?;
        drop(file);

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
        snapshots.checkpoint(&guard, recorder).map_err(Into::into)
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
