//! Snapshot payload helpers for the in-memory graph.
//!
//! `lora-store` no longer ships its own on-disk codec. The byte-level
//! columnar format lives in `lora-snapshot`; this module just bridges
//! between [`InMemoryGraph`] and the portable [`SnapshotPayload`]
//! vocabulary.

use crate::{SnapshotError, SnapshotMeta, SnapshotPayload};

use super::InMemoryGraph;

/// Format-version stamp surfaced through [`SnapshotMeta::format_version`]
/// for payloads produced via the inherent helpers below. Kept stable
/// across `lora-snapshot` codec versions because the payload shape
/// itself has not changed; only the on-disk encoding has.
pub(super) const PAYLOAD_FORMAT_VERSION: u32 = 1;

impl InMemoryGraph {
    /// Return the portable graph-state payload. Callers downstream of
    /// `lora-store` (typically `lora-database`) feed this into
    /// `lora-snapshot` for byte-level encoding.
    pub fn snapshot_payload(&self) -> SnapshotPayload {
        SnapshotPayload {
            next_node_id: self.next_node_id,
            next_rel_id: self.next_rel_id,
            nodes: self.iter_node_records().cloned().collect(),
            relationships: self.iter_rel_records().cloned().collect(),
        }
    }

    /// Replace the graph from a portable graph-state payload, preserving the
    /// currently installed mutation recorder across the swap.
    pub fn load_snapshot_payload(
        &mut self,
        payload: SnapshotPayload,
    ) -> Result<SnapshotMeta, SnapshotError> {
        let meta = SnapshotMeta {
            format_version: PAYLOAD_FORMAT_VERSION,
            node_count: payload.nodes.len(),
            relationship_count: payload.relationships.len(),
            wal_lsn: None,
        };

        // Build the restored graph in a fresh local instance and only
        // commit it into `self` at the very end. If a panic fires mid-
        // rebuild (e.g. OOM on a HashMap grow) the caller's graph is
        // untouched — we never observe a half-populated store.
        let node_capacity = payload.nodes.len().max(payload.next_node_id as usize);
        let relationship_capacity = payload
            .relationships
            .len()
            .max(payload.next_rel_id as usize);
        let mut rebuilt = Self::with_capacity_hint(node_capacity, relationship_capacity);
        rebuilt.next_node_id = payload.next_node_id;
        rebuilt.next_rel_id = payload.next_rel_id;

        for node in payload.nodes {
            let id = node.id;
            let labels = node.labels.clone();
            rebuilt.put_node(id, node);
            for label in &labels {
                rebuilt.insert_node_label_index(id, label);
            }
        }

        for rel in payload.relationships {
            rebuilt.attach_relationship(&rel);
            let id = rel.id;
            rebuilt.put_rel(id, rel);
        }
        rebuilt.rebuild_property_indexes();

        // Preserve the existing recorder across the swap — observers of the
        // store's identity should not be silently detached by a restore,
        // same policy as `clear()`.
        rebuilt.recorder = self.recorder.take();
        *self = rebuilt;

        Ok(meta)
    }
}
