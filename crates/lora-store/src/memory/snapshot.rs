//! Snapshot payload helpers for the in-memory graph.
//!
//! `lora-store` no longer ships its own on-disk codec. The byte-level
//! columnar format lives in `lora-snapshot`; this module just bridges
//! between [`InMemoryGraph`] and the portable [`SnapshotPayload`]
//! vocabulary.

use std::collections::BTreeSet;

use crate::{SnapshotError, SnapshotMeta, SnapshotPayload};

use super::index_catalog::StoredIndexEntity;
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
        let mut vector_indexes = self
            .vector_indexes_read(StoredIndexEntity::Node)
            .to_snapshots(StoredIndexEntity::Node);
        vector_indexes.extend(
            self.vector_indexes_read(StoredIndexEntity::Relationship)
                .to_snapshots(StoredIndexEntity::Relationship),
        );
        SnapshotPayload {
            next_node_id: self.next_node_id,
            next_rel_id: self.next_rel_id,
            nodes: self.iter_node_records().cloned().collect(),
            relationships: self.iter_rel_records().cloned().collect(),
            indexes: self.index_catalog_read().list(),
            constraints: self.constraint_catalog_read().list(),
            vector_indexes,
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

        validate_payload_ids(&payload)?;

        // Build the restored graph in a fresh local instance and only
        // commit it into `self` at the very end. Capacity is based on live
        // entity count, not `next_*_id`: snapshots may contain tombstone gaps,
        // and hostile next-id values must not force huge allocations before
        // the checked slab-growth path validates each concrete record id.
        let node_capacity = payload.nodes.len();
        let relationship_capacity = payload.relationships.len();
        let mut rebuilt = Self::with_capacity_hint(node_capacity, relationship_capacity);
        rebuilt.next_node_id = payload.next_node_id;
        rebuilt.next_rel_id = payload.next_rel_id;

        for node in payload.nodes {
            let id = node.id;
            let labels = node.labels.clone();
            if rebuilt.node_at(id).is_some() {
                return Err(SnapshotError::Decode(format!(
                    "duplicate node id {id} in snapshot payload"
                )));
            }
            rebuilt
                .put_node_checked(id, node)
                .map_err(SnapshotError::Decode)?;
            for label in &labels {
                rebuilt.insert_node_label_index(id, label);
            }
        }

        for rel in payload.relationships {
            if rebuilt.rel_at(rel.id).is_some() {
                return Err(SnapshotError::Decode(format!(
                    "duplicate relationship id {} in snapshot payload",
                    rel.id
                )));
            }
            if rebuilt.node_at(rel.src).is_none() {
                return Err(SnapshotError::Decode(format!(
                    "relationship {} references missing source node {}",
                    rel.id, rel.src
                )));
            }
            if rebuilt.node_at(rel.dst).is_none() {
                return Err(SnapshotError::Decode(format!(
                    "relationship {} references missing target node {}",
                    rel.id, rel.dst
                )));
            }
            let id = rel.id;
            rebuilt
                .put_rel_checked(id, rel.clone())
                .map_err(SnapshotError::Decode)?;
            rebuilt.attach_relationship(&rel);
        }
        rebuilt.rebuild_property_indexes();

        let constraint_owned_indexes: BTreeSet<String> = payload
            .constraints
            .iter()
            .filter_map(|def| {
                def.owned_index
                    .clone()
                    .or_else(|| def.kind.requires_backing_index().then(|| def.name.clone()))
            })
            .collect();

        // Re-register every user-visible index in the catalog. Going through
        // `register_index` re-populates RANGE buckets and keeps the
        // `populate_index_data` invariant aligned with the catalog —
        // skipping it would leave RANGE indexes registered but never populated.
        // Constraint-owned backing indexes are restored by re-registering the
        // owning constraint below, which keeps catalog ownership explicit.
        for def in payload.indexes {
            if constraint_owned_indexes.contains(&def.name) {
                continue;
            }
            // Errors here would mean the snapshot itself is corrupt or
            // ambiguous; map them into Decode rather than panicking.
            rebuilt
                .register_index(
                    crate::memory::IndexRequest {
                        explicit_name: Some(def.name.clone()),
                        kind: def.kind,
                        entity: def.entity,
                        label: def.label.clone(),
                        additional_labels: def.additional_labels.clone(),
                        properties: def.properties.clone(),
                        options: def.options.clone(),
                    },
                    /*if_not_exists*/ true,
                )
                .map_err(|e| SnapshotError::Decode(format!("index `{}`: {e}", def.name)))?;
        }

        // Overlay persisted HNSW snapshots over the freshly-registered
        // (and freshly-backfilled) vector indexes. This is the
        // post-step that gives Phase 5 its raison d'être: instead of
        // paying O(n log n) to re-insert every vector through the
        // HNSW algorithm, we install the graph topology byte-for-byte.
        // Snapshots from versions before this trailer round-trip with
        // `vector_indexes = []` so the fallback path (the backfill
        // that already ran inside `register_index`) handles them
        // correctly.
        for snap in payload.vector_indexes {
            let entity = snap.entity;
            let mut registry = rebuilt.vector_indexes_write(entity);
            if !registry.restore_snapshot(snap) {
                // Catalog/snapshot mismatch — registry already
                // contains the populate-built backend, which is the
                // safe fallback. No further action.
            }
        }

        // Re-register constraints. Uniqueness / key constraints recreate
        // their own backing indexes as part of registration.
        for def in payload.constraints {
            rebuilt
                .register_constraint(
                    crate::memory::ConstraintRequest {
                        name: def.name.clone(),
                        kind: def.kind.clone(),
                        entity: def.entity,
                        label: def.label.clone(),
                        properties: def.properties.clone(),
                    },
                    /*if_not_exists*/ true,
                )
                .map_err(|e| SnapshotError::Decode(format!("constraint `{}`: {e}", def.name)))?;
        }

        // Preserve the existing recorder across the swap — observers of the
        // store's identity should not be silently detached by a restore,
        // same policy as `clear()`.
        rebuilt.recorder = self.recorder.take();
        *self = rebuilt;

        Ok(meta)
    }
}

fn validate_payload_ids(payload: &SnapshotPayload) -> Result<(), SnapshotError> {
    validate_next_id("node", payload.next_node_id)?;
    validate_next_id("relationship", payload.next_rel_id)?;

    for node in &payload.nodes {
        validate_entity_id("node", node.id, payload.next_node_id)?;
    }
    for rel in &payload.relationships {
        validate_entity_id("relationship", rel.id, payload.next_rel_id)?;
        validate_slot_id("relationship source node", rel.src)?;
        validate_slot_id("relationship target node", rel.dst)?;
    }

    Ok(())
}

fn validate_next_id(kind: &str, next_id: u64) -> Result<(), SnapshotError> {
    validate_slot_id(&format!("next {kind} id"), next_id)?;
    if next_id == u64::MAX {
        return Err(SnapshotError::Decode(format!(
            "next {kind} id {next_id} leaves no allocatable id"
        )));
    }
    Ok(())
}

fn validate_entity_id(kind: &str, id: u64, next_id: u64) -> Result<(), SnapshotError> {
    validate_slot_id(kind, id)?;
    if id >= next_id {
        return Err(SnapshotError::Decode(format!(
            "{kind} id {id} is not below next {kind} id {next_id}"
        )));
    }
    Ok(())
}

fn validate_slot_id(label: &str, id: u64) -> Result<(), SnapshotError> {
    let idx = usize::try_from(id).map_err(|_| {
        SnapshotError::Decode(format!(
            "{label} {id} does not fit in usize on this platform"
        ))
    })?;
    idx.checked_add(1)
        .ok_or_else(|| SnapshotError::Decode(format!("{label} {id} leaves no valid slab slot")))?;
    Ok(())
}
