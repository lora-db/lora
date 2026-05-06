use std::path::PathBuf;

/// Durability mode for committed transactions.
///
/// Commits write WAL bytes to the OS before returning, then a background
/// flusher, explicit `force_fsync`, checkpoint, `Database::sync`, or clean
/// drop creates the storage durability boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Write commit bytes to the OS immediately, but defer `fsync` until an
    /// explicit durability boundary or the background flusher interval.
    /// Background fsync failures poison the WAL through `Wal::bg_failure`.
    GroupSync { interval_ms: u32 },
}

impl Default for SyncMode {
    fn default() -> Self {
        Self::GroupSync { interval_ms: 50 }
    }
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
    /// Quick constructor for the default `GroupSync` mode and an 8 MiB
    /// segment target.
    pub fn enabled(dir: impl Into<PathBuf>) -> Self {
        Self::Enabled {
            dir: dir.into(),
            sync_mode: SyncMode::default(),
            segment_target_bytes: 8 * 1024 * 1024,
        }
    }
}
