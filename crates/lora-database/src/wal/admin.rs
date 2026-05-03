//! Storage-agnostic admin surface for the WAL.
//!
//! Transports (`lora-server`, language bindings) type-erase on
//! `Arc<dyn WalAdmin>` so they don't need to name the database's
//! backend type parameter. All LSNs cross the trait boundary as raw
//! `u64` so callers don't pull a direct dependency on `lora-wal`.

use std::path::Path;

use anyhow::{anyhow, Result};
use lora_store::{InMemoryGraph, SnapshotMeta};
use lora_wal::Lsn;

use crate::database::Database;

/// Storage-agnostic admin surface for the WAL.
///
/// `Database<InMemoryGraph>` picks up the impl below when a WAL is
/// attached.
pub trait WalAdmin: Send + Sync + 'static {
    /// Take a checkpoint at `path`. The snapshot's header is stamped
    /// with the WAL's `durable_lsn`; older sealed segments are then
    /// dropped.
    fn checkpoint(&self, path: &Path) -> Result<SnapshotMeta>;

    /// Snapshot of the WAL's current state — durable / next LSN,
    /// active / oldest segment id. Cheap; a single WAL mutex acquisition.
    fn wal_status(&self) -> Result<WalStatus>;

    /// Drop sealed segments at or below `fence_lsn`. Idempotent.
    fn wal_truncate(&self, fence_lsn: u64) -> Result<()>;
}

/// Snapshot of WAL state returned by [`WalAdmin::wal_status`].
///
/// `bg_failure` is the latched fsync error from the background flusher
/// (only meaningful under `SyncMode::Group`). When `Some`, the WAL is
/// poisoned and every subsequent commit will fail loudly until the
/// operator restarts from the last consistent snapshot + WAL.
#[derive(Debug, Clone)]
pub struct WalStatus {
    pub durable_lsn: u64,
    pub next_lsn: u64,
    pub active_segment_id: u64,
    pub oldest_segment_id: u64,
    pub bg_failure: Option<String>,
}

impl WalAdmin for Database<InMemoryGraph> {
    fn checkpoint(&self, path: &Path) -> Result<SnapshotMeta> {
        self.checkpoint_to(path)
    }

    fn wal_status(&self) -> Result<WalStatus> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("WAL not enabled"))?;
        let wal = recorder.wal();
        Ok(WalStatus {
            durable_lsn: wal.durable_lsn().raw(),
            next_lsn: wal.next_lsn().raw(),
            active_segment_id: wal.active_segment_id(),
            oldest_segment_id: wal.oldest_segment_id(),
            bg_failure: wal.bg_failure(),
        })
    }

    fn wal_truncate(&self, fence_lsn: u64) -> Result<()> {
        let recorder = self
            .wal
            .as_ref()
            .ok_or_else(|| anyhow!("WAL not enabled"))?;
        recorder.truncate_up_to(Lsn::new(fence_lsn))?;
        Ok(())
    }
}
