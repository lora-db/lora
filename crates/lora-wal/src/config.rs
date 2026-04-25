use std::path::PathBuf;

/// Durability mode for committed transactions.
///
/// The single-mutex engine has at most one concurrent committer at a time,
/// so the classical group-commit win — overlapping fsyncs across many
/// committers — does not apply here. [`SyncMode::PerCommit`] is the
/// default. The other two modes exist for narrow operational profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// `fsync` the active segment before the committing thread releases
    /// the engine mutex. The strongest durability guarantee the WAL
    /// offers; every observed query result is fully durable.
    #[default]
    PerCommit,

    /// Buffer commits and `fsync` on a fixed cadence on a background
    /// thread. Trades the last `interval_ms` of commits for higher
    /// throughput on bulk-load workloads.
    ///
    /// A background fsync failure poisons the WAL: the next `commit` /
    /// `flush` / `force_fsync` returns [`crate::WalError::Poisoned`] and
    /// `Wal::bg_failure` reports the underlying cause. Operators
    /// inspect that via `/admin/wal/status` (`bgFailure`) and recover
    /// by restarting from the last consistent snapshot + WAL.
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
