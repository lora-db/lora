//! Snapshot value types — the portable payload + metadata + error
//! vocabulary every backend speaks.
//!
//! The on-disk snapshot container lives in the [`lora-snapshot`] crate
//! (column-oriented, optionally compressed and authenticated). Backends
//! produce a [`SnapshotPayload`] through their inherent helpers (e.g.
//! [`super::InMemoryGraph::snapshot_payload`]); `lora-snapshot` encodes
//! that payload and reuses the small store-owned binary codec for nested
//! value/catalog records.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::memory::{ConstraintDefinition, IndexDefinition};
use crate::{NodeId, NodeRecord, RelationshipId, RelationshipRecord};

/// Portable representation of an entire store state.
///
/// Backends produce and consume this struct through inherent helpers;
/// the byte-level codec is owned by `lora-snapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPayload {
    pub next_node_id: NodeId,
    pub next_rel_id: RelationshipId,
    pub nodes: Vec<NodeRecord>,
    pub relationships: Vec<RelationshipRecord>,
    /// Catalog of explicitly-declared indexes. Defaulted to empty so
    /// older snapshots that lack the trailer round-trip cleanly.
    #[serde(default)]
    pub indexes: Vec<IndexDefinition>,
    /// Catalog of explicitly-declared constraints. Defaulted to empty
    /// so snapshots from versions before constraint support survive
    /// the round-trip.
    #[serde(default)]
    pub constraints: Vec<ConstraintDefinition>,
}

impl SnapshotPayload {
    pub fn empty() -> Self {
        Self {
            next_node_id: 0,
            next_rel_id: 0,
            nodes: Vec::new(),
            relationships: Vec::new(),
            indexes: Vec::new(),
            constraints: Vec::new(),
        }
    }
}

/// Metadata reported by snapshot encode / decode entry points. Kept
/// small and stable so callers can log / diff it without reflecting on
/// the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// Format version the payload is written in.
    pub format_version: u32,
    /// Number of nodes in the snapshot.
    pub node_count: usize,
    /// Number of relationships in the snapshot.
    pub relationship_count: usize,
    /// WAL log position captured alongside the snapshot, if any. `None` for
    /// pure (non-checkpoint) snapshots.
    pub wal_lsn: Option<u64>,
}

/// Errors a backend may surface while building or restoring a snapshot
/// payload. Codec-level errors live in [`lora-snapshot`]; these are the
/// store-side payload-shaped failures.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("snapshot payload could not be decoded: {0}")]
    Decode(String),

    #[error("snapshot payload could not be encoded: {0}")]
    Encode(String),
}
