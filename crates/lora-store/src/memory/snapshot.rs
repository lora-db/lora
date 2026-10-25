//! Snapshot payload helpers for the in-memory graph.
//!
//! `lora-store` no longer ships its own on-disk codec. The byte-level
//! columnar format lives in `lora-snapshot`; this module just bridges
//! between [`InMemoryGraph`] and the portable [`SnapshotPayload`]
//! vocabulary.

use std::collections::BTreeSet;

use crate::{
    NodeId, NodeRecord, NodeRef, RelRef, RelationshipId, RelationshipRecord, SnapshotError,
    SnapshotMeta, SnapshotPayload,
};

use super::distinct_stats::DistinctBuilder;
use super::index_catalog::StoredIndexEntity;
use super::{ConstraintDefinition, InMemoryGraph, IndexDefinition, VectorIndexSnapshot};

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
            nodes: self.iter_node_refs().map(|r| r.to_record()).collect(),
            relationships: self.iter_rel_refs().map(|r| r.to_record()).collect(),
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

        let mut builder = SnapshotGraphBuilder::new(payload.next_node_id, payload.next_rel_id)?;
        for node in payload.nodes {
            builder.push_node(node)?;
        }
        for rel in payload.relationships {
            builder.push_relationship(rel)?;
        }
        let mut rebuilt =
            builder.finish(payload.indexes, payload.constraints, payload.vector_indexes)?;

        // Preserve the existing recorder across the swap — observers of the
        // store's identity should not be silently detached by a restore,
        // same policy as `clear()`.
        rebuilt.recorder = self.recorder.take();
        *self = rebuilt;

        Ok(meta)
    }

    /// The node records, in id order, as borrowed views. With
    /// [`Self::snapshot_relationships`] and [`Self::snapshot_catalogs`] this
    /// is what a snapshot encoder reads instead of a [`SnapshotPayload`],
    /// which would hold a decoded copy of every record at once. Each call
    /// is a fresh pass, so an encoder that writes column by column can
    /// walk the graph once per column.
    pub fn snapshot_nodes(&self) -> impl Iterator<Item = NodeRef<'_>> + '_ {
        self.iter_node_refs()
    }

    /// The relationship records, in id order, as borrowed views.
    pub fn snapshot_relationships(&self) -> impl Iterator<Item = RelRef<'_>> + '_ {
        self.iter_rel_refs()
    }

    /// Everything a snapshot holds besides the records.
    pub fn snapshot_catalogs(&self) -> SnapshotCatalogs {
        let mut vector_indexes = self
            .vector_indexes_read(StoredIndexEntity::Node)
            .to_snapshots(StoredIndexEntity::Node);
        vector_indexes.extend(
            self.vector_indexes_read(StoredIndexEntity::Relationship)
                .to_snapshots(StoredIndexEntity::Relationship),
        );
        SnapshotCatalogs {
            next_node_id: self.next_node_id,
            next_rel_id: self.next_rel_id,
            indexes: self.index_catalog_read().list(),
            constraints: self.constraint_catalog_read().list(),
            vector_indexes,
        }
    }
}

/// The parts of a snapshot that are not records: the id counters and the
/// declared catalogs. Small next to the graph.
#[derive(Debug, Clone)]
pub struct SnapshotCatalogs {
    pub next_node_id: NodeId,
    pub next_rel_id: RelationshipId,
    pub indexes: Vec<IndexDefinition>,
    pub constraints: Vec<ConstraintDefinition>,
    pub vector_indexes: Vec<VectorIndexSnapshot>,
}

/// Builds an [`InMemoryGraph`] from snapshot records one at a time.
///
/// A decoder that streams a snapshot pushes each record as it decodes it,
/// so the records are never all held at once beside the graph they fill.
/// Push every node before the relationships that reference it, then call
/// [`Self::finish`] with the catalogs, which registers the indexes and
/// constraints over the loaded data.
pub struct SnapshotGraphBuilder {
    graph: InMemoryGraph,
    node_distinct: DistinctBuilder,
    rel_distinct: DistinctBuilder,
}

impl SnapshotGraphBuilder {
    pub fn new(next_node_id: NodeId, next_rel_id: RelationshipId) -> Result<Self, SnapshotError> {
        validate_next_id("node", next_node_id)?;
        validate_next_id("relationship", next_rel_id)?;
        let mut graph = InMemoryGraph::new();
        graph.next_node_id = next_node_id;
        graph.next_rel_id = next_rel_id;
        Ok(Self {
            graph,
            node_distinct: DistinctBuilder::default(),
            rel_distinct: DistinctBuilder::default(),
        })
    }

    pub fn push_node(&mut self, node: NodeRecord) -> Result<(), SnapshotError> {
        let id = node.id;
        validate_entity_id("node", id, self.graph.next_node_id)?;
        // Distinct-value sketches are not persisted: count the record as a
        // live create would, through the bulk builder.
        for label in &node.labels {
            self.node_distinct.add_all(label, &node.properties);
        }
        if self.graph.has_node_at(id) {
            return Err(SnapshotError::Decode(format!(
                "duplicate node id {id} in snapshot payload"
            )));
        }
        self.graph
            .put_node_checked(id, &node)
            .map_err(SnapshotError::Decode)?;
        for label in &node.labels {
            self.graph.insert_node_label_index(id, label);
        }
        Ok(())
    }

    pub fn push_relationship(&mut self, rel: RelationshipRecord) -> Result<(), SnapshotError> {
        validate_entity_id("relationship", rel.id, self.graph.next_rel_id)?;
        validate_slot_id("relationship source node", rel.src)?;
        validate_slot_id("relationship target node", rel.dst)?;
        if self.graph.has_rel_at(rel.id) {
            return Err(SnapshotError::Decode(format!(
                "duplicate relationship id {} in snapshot payload",
                rel.id
            )));
        }
        if !self.graph.has_node_at(rel.src) {
            return Err(SnapshotError::Decode(format!(
                "relationship {} references missing source node {}",
                rel.id, rel.src
            )));
        }
        if !self.graph.has_node_at(rel.dst) {
            return Err(SnapshotError::Decode(format!(
                "relationship {} references missing target node {}",
                rel.id, rel.dst
            )));
        }
        self.graph
            .put_rel_checked(rel.id, &rel)
            .map_err(SnapshotError::Decode)?;
        self.graph.attach_relationship(&rel);
        self.rel_distinct.add_all(&rel.rel_type, &rel.properties);
        Ok(())
    }

    pub fn finish(
        self,
        indexes: Vec<IndexDefinition>,
        constraints: Vec<ConstraintDefinition>,
        vector_indexes: Vec<VectorIndexSnapshot>,
    ) -> Result<InMemoryGraph, SnapshotError> {
        let Self {
            graph: mut rebuilt,
            node_distinct,
            rel_distinct,
        } = self;
        rebuilt.distinct_stats.nodes = node_distinct.finish();
        rebuilt.distinct_stats.relationships = rel_distinct.finish();
        // No hash property index is built here. Restart reproduces the
        // state of a fresh process: the declared RANGE indexes and the
        // uniqueness/key constraints below activate (and backfill) the
        // hash indexes they need through `register_index` /
        // `register_constraint`. Implicit, lookup-activated indexes are
        // rebuilt lazily by the first lookup that needs them.

        let constraint_owned_indexes: BTreeSet<String> = constraints
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
        for def in indexes {
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
        for snap in vector_indexes {
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
        for def in constraints {
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

        Ok(rebuilt)
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
