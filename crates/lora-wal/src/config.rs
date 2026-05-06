use std::path::PathBuf;

/// Durability mode for committed transactions.
///
/// The current release has at most one committer at a time. The enum still
/// names the durability strategies we want long term, but no mode requires a
/// background thread today. [`SyncMode::PerCommit`] is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// `fsync` the active segment before the committing thread releases the
    /// store write lock. The strongest durability guarantee the WAL offers;
    /// every observed query result is fully durable on native filesystems.
    /// On `wasm32-unknown-unknown`, fsync is intentionally a no-op.
    #[default]
    PerCommit,

    /// Write commit bytes to the OS immediately, but defer `fsync` until an
    /// explicit `force_fsync`, checkpoint, `Database::sync`, or clean WAL
    /// drop. The interval is retained as part of the public configuration so a
    /// later release can add a background/group flusher without changing
    /// callers, but it is not scheduled in the single-threaded release.
    ///
    /// A future background fsync failure will poison the WAL through the
    /// existing `Wal::bg_failure` surface. In this release, Group mode is
    /// cooperative, so that field remains `None` unless another caller poisons
    /// the WAL.
    Group { interval_ms: u32 },

    /// Append but never `fsync` from the WAL; rely on whatever the OS
    /// happens to flush. Intended for CDC-only deployments where the WAL
    /// is consumed by an external reader and durability is provided
    /// elsewhere.
    None,
}

/// Configuration for opening a [`Wal`](crate::wal::Wal).
///
/// `Disabled` is the zero-overhead variant `lora-database` falls back to
/// when the operator has not configured a WAL directory; it does not
/// install a recorder or open any files.
#[derive(Debug, Clone, Default)]
pub enum WalConfig {
    #[default]
    Disabled,
    Enabled {
        dir: PathBuf,
        sync_mode: SyncMode,
        /// Target size of an active segment before rotation. Rotation
        /// only happens at a `TxBegin` boundary so a transaction never
        /// spans segments.
        segment_target_bytes: u64,
    },
}

impl WalConfig {
    /// Quick constructor for the default `PerCommit` mode and an 8 MiB
    /// segment target.
    pub fn enabled(dir: impl Into<PathBuf>) -> Self {
        Self::Enabled {
            dir: dir.into(),
            sync_mode: SyncMode::PerCommit,
            segment_target_bytes: 8 * 1024 * 1024,
        }
    }
}
