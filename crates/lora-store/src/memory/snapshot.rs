//! Snapshot payload helpers for the in-memory graph.
//!
//! `lora-store` no longer ships its own on-disk codec. The byte-level
//! columnar format lives in `lora-snapshot`; this module just bridges
//! between [`InMemoryGraph`] and the portable [`SnapshotPayload`]
//! vocabulary.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::dict::{Dicts, ValueDictionary};
use crate::encoded::{self, StoredNode, StoredRel};
use crate::{
    Name, NodeId, NodeRecord, NodeRef, PropertyValue, RelRef, RelationshipId, RelationshipRecord,
    SnapshotError, SnapshotMeta, SnapshotPayload, ValueRef,
};

use super::distinct_stats::DistinctBuilder;
use super::graph::DeferredData;
use super::index_catalog::StoredIndexEntity;
use super::record_slab::BuiltLeaves;
use super::{ConstraintDefinition, InMemoryGraph, IndexDefinition, VectorIndexSnapshot};
use crate::scratch::ScratchVec;

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

    /// Replace the graph with `restored`, one a snapshot decoder built
    /// (`lora_snapshot::read_graph_snapshot`), preserving the currently
    /// installed mutation recorder as [`Self::load_snapshot_payload`] does.
    pub fn replace_with_restored(&mut self, mut restored: InMemoryGraph) {
        restored.recorder = self.recorder.take();
        *self = restored;
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

/// A node record names one label twice: no write makes one, and the label
/// index would hold the node twice.
fn repeated_label(id: NodeId) -> SnapshotError {
    SnapshotError::Decode(format!("node {id} has a repeated label in snapshot"))
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
        if node
            .labels
            .iter()
            .enumerate()
            .any(|(i, label)| node.labels[..i].contains(label))
        {
            return Err(repeated_label(id));
        }
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
        //
        // Registration only fills the catalogs, in order; the indexes'
        // data and the constraint scans are left in `deferred` and done
        // together at the end, on every core.
        let mut deferred = DeferredData::default();
        for def in indexes {
            if constraint_owned_indexes.contains(&def.name) {
                continue;
            }
            // Errors here would mean the snapshot itself is corrupt or
            // ambiguous; map them into Decode rather than panicking.
            rebuilt
                .register_index_deferring(
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
                    /*record_event*/ true,
                    Some(&mut deferred),
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
                .register_constraint_deferring(
                    crate::memory::ConstraintRequest {
                        name: def.name.clone(),
                        kind: def.kind.clone(),
                        entity: def.entity,
                        label: def.label.clone(),
                        properties: def.properties.clone(),
                        backing: def.backing,
                    },
                    /*if_not_exists*/ true,
                    Some(&mut deferred),
                )
                .map_err(|e| SnapshotError::Decode(format!("constraint `{}`: {e}", def.name)))?;
        }

        rebuilt
            .run_deferred(deferred)
            .map_err(SnapshotError::Decode)?;
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

// ---------------------------------------------------------------------------
// Encoding on several threads
// ---------------------------------------------------------------------------

/// The dictionaries a graph numbers its records with, in number order.
/// A snapshot writer names records by these numbers, and a
/// [`SnapshotGraphBuilder`] primed with them ([`SnapshotGraphBuilder::prime`])
/// encodes records on any thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotNames {
    pub labels: Vec<Name>,
    pub types: Vec<Name>,
    pub keys: Vec<Name>,
    /// Which short string values the graph stores by number.
    pub values: ValueDictionary,
}

/// A stored node as a snapshot writer reads it: names by number in
/// [`SnapshotNames`], values borrowed.
#[derive(Clone, Copy)]
pub struct SnapshotNode<'a> {
    stored: StoredNode<'a>,
}

impl<'a> SnapshotNode<'a> {
    pub fn id(self) -> NodeId {
        self.stored.id
    }

    /// Numbers in [`SnapshotNames::labels`], in the node's order.
    pub fn label_ids(self) -> impl ExactSizeIterator<Item = u32> + 'a {
        let labels = self.stored.labels();
        let count = labels.len();
        ExactLen {
            inner: labels.ids(),
            remaining: count,
        }
    }

    /// `(number in` [`SnapshotNames::keys`]`, value)` in key-name order.
    pub fn properties(self) -> impl Iterator<Item = (u32, ValueRef<'a>)> + 'a {
        self.stored.properties().iter_ids()
    }

    pub fn property_count(self) -> usize {
        self.stored.properties().len()
    }
}

/// A stored relationship as a snapshot writer reads it.
#[derive(Clone, Copy)]
pub struct SnapshotRelationship<'a> {
    stored: StoredRel<'a>,
}

impl<'a> SnapshotRelationship<'a> {
    pub fn id(self) -> RelationshipId {
        self.stored.id
    }

    pub fn src(self) -> NodeId {
        self.stored.src
    }

    pub fn dst(self) -> NodeId {
        self.stored.dst
    }

    /// Number in [`SnapshotNames::types`].
    pub fn type_id(self) -> u32 {
        self.stored.type_id()
    }

    /// `(number in` [`SnapshotNames::keys`]`, value)` in key-name order.
    pub fn properties(self) -> impl Iterator<Item = (u32, ValueRef<'a>)> + 'a {
        self.stored.properties().iter_ids()
    }

    pub fn property_count(self) -> usize {
        self.stored.properties().len()
    }
}

/// An iterator of known length.
struct ExactLen<I> {
    inner: I,
    remaining: usize,
}

impl<I: Iterator> Iterator for ExactLen<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        let item = self.inner.next()?;
        self.remaining -= 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<I: Iterator> ExactSizeIterator for ExactLen<I> {}

impl InMemoryGraph {
    /// The graph's dictionaries. Every name a record holds is in them,
    /// and so may names no record holds any more.
    pub fn snapshot_names(&self) -> SnapshotNames {
        let names = |dict: &crate::dict::NameDict| {
            (0..dict.len() as u32)
                .map(|id| dict.name(id).clone())
                .collect()
        };
        SnapshotNames {
            labels: names(&self.dicts.labels),
            types: names(&self.dicts.types),
            keys: names(&self.dicts.keys),
            values: self
                .dicts
                .values
                .export(|key| self.dicts.keys.name(key).clone()),
        }
    }

    /// The nodes with an id in `ids`, in id order, numbered as
    /// [`Self::snapshot_names`] numbers them. Writers on several threads
    /// each take a range.
    pub fn snapshot_nodes_in(
        &self,
        ids: std::ops::Range<NodeId>,
    ) -> impl Iterator<Item = SnapshotNode<'_>> + '_ {
        let dicts = &self.dicts;
        let start = usize::try_from(ids.start).unwrap_or(usize::MAX);
        let end = ids.end;
        self.nodes
            .iter_from(start)
            .map(|(i, bytes)| (i as NodeId, bytes))
            .take_while(move |(id, _)| *id < end)
            .map(move |(id, bytes)| SnapshotNode {
                stored: StoredNode::new(id, bytes, dicts),
            })
    }

    /// The relationships with an id in `ids`, in id order.
    pub fn snapshot_relationships_in(
        &self,
        ids: std::ops::Range<RelationshipId>,
    ) -> impl Iterator<Item = SnapshotRelationship<'_>> + '_ {
        let dicts = &self.dicts;
        let start = usize::try_from(ids.start).unwrap_or(usize::MAX);
        let end = ids.end;
        self.relationships
            .iter_from(start)
            .map(|(i, bytes)| (i as RelationshipId, bytes))
            .take_while(move |(id, _)| *id < end)
            .map(move |(id, bytes)| SnapshotRelationship {
                stored: StoredRel::new(id, bytes, dicts),
            })
    }

    /// One past the highest node slot (an upper bound on node ids).
    pub fn snapshot_node_slots(&self) -> u64 {
        self.nodes.len() as u64
    }

    /// One past the highest relationship slot.
    pub fn snapshot_relationship_slots(&self) -> u64 {
        self.relationships.len() as u64
    }
}

/// Encodes snapshot records into a graph's stored form on any thread,
/// for the [`SnapshotGraphBuilder`] that [primed](SnapshotGraphBuilder::prime)
/// it. Cheap to clone.
#[derive(Clone)]
pub struct SnapshotEncoder {
    inner: Arc<EncoderTables>,
}

struct EncoderTables {
    /// The builder's dictionaries when it was primed. Encoding reads them
    /// only; a value they do not number yet is stored in full.
    dicts: Dicts,
    /// Snapshot label / type / key number -> graph number.
    labels: Vec<u32>,
    types: Vec<u32>,
    keys: Vec<u32>,
    /// The snapshot's key names, and each one's place in name order:
    /// records keep their properties in that order.
    key_names: Vec<Arc<str>>,
    key_ranks: Vec<u32>,
    label_names: Vec<Name>,
    type_names: Vec<Name>,
    next_node_id: NodeId,
    next_rel_id: RelationshipId,
}

/// Nodes encoded off the builder's thread, for
/// [`SnapshotGraphBuilder::push_nodes`]. Fill with [`SnapshotEncoder::push_node`].
pub struct NodeBatch {
    /// The encoded records, in whole leaves of the node slab.
    records: BuiltLeaves,
    by_label: Vec<(u32, Vec<NodeId>)>,
    distinct: DistinctBuilder,
    /// Reused per record.
    scratch: Vec<u8>,
    labels: Vec<u32>,
}

/// Relationships encoded off the builder's thread, for
/// [`SnapshotGraphBuilder::push_relationships`].
pub struct RelationshipBatch {
    /// The encoded records, in whole leaves of the relationship slab.
    records: BuiltLeaves,
    /// `(id, source, target, graph type number)` per relationship.
    links: ScratchVec<(RelationshipId, NodeId, NodeId, u32)>,
    by_type: Vec<(u32, Vec<RelationshipId>)>,
    distinct: DistinctBuilder,
    scratch: Vec<u8>,
}

impl NodeBatch {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.len() == 0
    }
}

impl RelationshipBatch {
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.len() == 0
    }
}

/// Add record `id` to a batch's leaves. Ids ascend within a batch, so
/// a repeated or falling one is refused here.
fn push_record(records: &mut BuiltLeaves, id: u64, bytes: &[u8]) -> Result<(), SnapshotError> {
    let slot = usize::try_from(id)
        .map_err(|_| SnapshotError::Decode(format!("id {id} does not fit in usize")))?;
    if records.last_slot().is_some_and(|last| last >= slot) {
        return Err(SnapshotError::Decode(format!(
            "record ids in a snapshot batch must ascend (id {id})"
        )));
    }
    records.push(slot, bytes);
    Ok(())
}

/// `ids` for `number` in a batch's by-name lists, which hold few names.
fn ids_for(lists: &mut Vec<(u32, Vec<u64>)>, number: u32) -> &mut Vec<u64> {
    let at = match lists.iter().rposition(|(n, _)| *n == number) {
        Some(at) => at,
        None => {
            lists.push((number, Vec::new()));
            lists.len() - 1
        }
    };
    &mut lists[at].1
}

impl SnapshotEncoder {
    /// A batch for about `capacity` records.
    pub fn node_batch(&self, capacity: usize) -> NodeBatch {
        let _ = capacity;
        NodeBatch {
            records: BuiltLeaves::default(),
            by_label: Vec::new(),
            distinct: DistinctBuilder::default(),
            scratch: Vec::new(),
            labels: Vec::new(),
        }
    }

    /// A batch for about `capacity` records. `capacity` comes from the
    /// snapshot being read, so at most a frame's worth is reserved up front.
    pub fn relationship_batch(&self, capacity: usize) -> RelationshipBatch {
        RelationshipBatch {
            records: BuiltLeaves::default(),
            links: ScratchVec::with_capacity(capacity.min(1 << 16)),
            by_type: Vec::new(),
            distinct: DistinctBuilder::default(),
            scratch: Vec::new(),
        }
    }

    /// Put `properties` (snapshot key numbers) in the order a record keeps
    /// them, the last of a repeated key winning as in [`Properties`], and
    /// renumber the keys for the graph.
    fn order_properties(
        &self,
        properties: &mut Vec<(u32, PropertyValue)>,
    ) -> Result<(), SnapshotError> {
        let tables = &*self.inner;
        let mut sorted = true;
        let mut last = None;
        for (key, _) in properties.iter() {
            let rank = *tables
                .key_ranks
                .get(*key as usize)
                .ok_or_else(|| SnapshotError::Decode("invalid property key number".into()))?;
            if last.is_some_and(|last| last >= rank) {
                sorted = false;
            }
            last = Some(rank);
        }
        if !sorted {
            // Stable, so of two equal keys the later stays later; keep it.
            properties.sort_by_key(|(key, _)| tables.key_ranks[*key as usize]);
            let mut i = properties.len();
            while i > 1 {
                i -= 1;
                let same = tables.key_ranks[properties[i].0 as usize]
                    == tables.key_ranks[properties[i - 1].0 as usize];
                if same {
                    properties.remove(i - 1);
                }
            }
        }
        Ok(())
    }

    /// Encode a node: `labels` and the keys of `properties` are numbers in
    /// the snapshot's [`SnapshotNames`]. Takes the values out of
    /// `properties`, leaving it empty for the next record.
    pub fn push_node(
        &self,
        batch: &mut NodeBatch,
        id: NodeId,
        labels: &[u32],
        properties: &mut Vec<(u32, PropertyValue)>,
    ) -> Result<(), SnapshotError> {
        let tables = &*self.inner;
        validate_entity_id("node", id, tables.next_node_id)?;
        self.order_properties(properties)?;
        batch.labels.clear();
        for &label in labels {
            let number = *tables
                .labels
                .get(label as usize)
                .ok_or_else(|| SnapshotError::Decode("invalid label dictionary id".into()))?;
            if batch.labels.contains(&number) {
                return Err(repeated_label(id));
            }
            batch.labels.push(number);
            ids_for(&mut batch.by_label, number).push(id);
        }
        for &label in labels {
            let name = &tables.label_names[label as usize];
            batch.distinct.add_all(
                name,
                properties
                    .iter()
                    .map(|(key, value)| (&tables.key_names[*key as usize], value)),
            );
        }
        for (key, _) in properties.iter_mut() {
            *key = tables.keys[*key as usize];
        }
        encoded::encode_node_ids(&batch.labels, properties, &tables.dicts, &mut batch.scratch);
        properties.clear();
        push_record(&mut batch.records, id, &batch.scratch)
    }

    /// Encode a relationship: `rel_type` and the keys of `properties` are
    /// numbers in the snapshot's [`SnapshotNames`].
    pub fn push_relationship(
        &self,
        batch: &mut RelationshipBatch,
        id: RelationshipId,
        src: NodeId,
        dst: NodeId,
        rel_type: u32,
        properties: &mut Vec<(u32, PropertyValue)>,
    ) -> Result<(), SnapshotError> {
        let tables = &*self.inner;
        validate_entity_id("relationship", id, tables.next_rel_id)?;
        validate_slot_id("relationship source node", src)?;
        validate_slot_id("relationship target node", dst)?;
        let type_id = *tables
            .types
            .get(rel_type as usize)
            .ok_or_else(|| SnapshotError::Decode("invalid relationship type id".into()))?;
        self.order_properties(properties)?;
        batch.distinct.add_all(
            &tables.type_names[rel_type as usize],
            properties
                .iter()
                .map(|(key, value)| (&tables.key_names[*key as usize], value)),
        );
        for (key, _) in properties.iter_mut() {
            *key = tables.keys[*key as usize];
        }
        encoded::encode_rel_ids(
            src,
            dst,
            type_id,
            properties,
            &tables.dicts,
            &mut batch.scratch,
        );
        properties.clear();
        push_record(&mut batch.records, id, &batch.scratch)?;
        batch.links.push((id, src, dst, type_id));
        ids_for(&mut batch.by_type, type_id).push(id);
        Ok(())
    }
}

impl SnapshotGraphBuilder {
    /// Number the snapshot's names in the graph being built, and return
    /// an encoder that turns records named by those numbers into the
    /// graph's stored form on any thread. Call before pushing records.
    pub fn prime(&mut self, names: &SnapshotNames) -> Result<SnapshotEncoder, SnapshotError> {
        let dicts = &mut self.graph.dicts;
        let labels = names
            .labels
            .iter()
            .map(|name| dicts.labels.id_or_insert(name))
            .collect();
        let types = names
            .types
            .iter()
            .map(|name| dicts.types.id_or_insert(name))
            .collect();
        let keys: Vec<u32> = names
            .keys
            .iter()
            .map(|name| dicts.keys.id_or_insert(name))
            .collect();
        let mut by_name: Vec<u32> = (0..names.keys.len() as u32).collect();
        by_name.sort_by(|a, b| {
            names.keys[*a as usize]
                .as_str()
                .cmp(names.keys[*b as usize].as_str())
        });
        let mut key_ranks = vec![0u32; names.keys.len()];
        let mut rank = 0u32;
        for (i, &key) in by_name.iter().enumerate() {
            if i > 0
                && names.keys[by_name[i - 1] as usize].as_str() != names.keys[key as usize].as_str()
            {
                rank += 1;
            }
            key_ranks[key as usize] = rank;
        }
        let keys_dict = &mut dicts.keys;
        dicts
            .values
            .import(&names.values, |name| keys_dict.id_or_insert(name))
            .map_err(SnapshotError::Decode)?;
        Ok(SnapshotEncoder {
            inner: Arc::new(EncoderTables {
                dicts: dicts.clone(),
                labels,
                types,
                keys,
                key_names: names
                    .keys
                    .iter()
                    .map(|name| name.as_arc().clone())
                    .collect(),
                key_ranks,
                label_names: names.labels.clone(),
                type_names: names.types.clone(),
                next_node_id: self.graph.next_node_id,
                next_rel_id: self.graph.next_rel_id,
            }),
        })
    }

    /// Store nodes an encoder from [`Self::prime`] encoded.
    pub fn push_nodes(&mut self, batch: NodeBatch) -> Result<(), SnapshotError> {
        self.graph
            .put_encoded_node_leaves(batch.records)
            .map_err(SnapshotError::Decode)?;
        for (label, ids) in &batch.by_label {
            self.graph.extend_node_label_index(*label, ids);
        }
        self.node_distinct.merge(batch.distinct);
        Ok(())
    }

    /// Store relationships an encoder from [`Self::prime`] encoded. Their
    /// endpoints must already be pushed.
    pub fn push_relationships(&mut self, batch: RelationshipBatch) -> Result<(), SnapshotError> {
        self.push_relationship_batches(vec![batch])
    }

    /// [`Self::push_relationships`] for several batches, in order: their
    /// adjacency is linked in one pass, on every core when the build
    /// allows.
    pub fn push_relationship_batches(
        &mut self,
        batches: Vec<RelationshipBatch>,
    ) -> Result<(), SnapshotError> {
        let mut links = ScratchVec::with_capacity(batches.iter().map(|b| b.links.len()).sum());
        for batch in batches {
            self.graph
                .put_encoded_relationship_leaves(batch.records, batch.links.as_slice())
                .map_err(SnapshotError::Decode)?;
            for (rel_type, ids) in &batch.by_type {
                self.graph.extend_relationship_type_index(*rel_type, ids);
            }
            self.rel_distinct.merge(batch.distinct);
            links.extend_from(&batch.links);
        }
        self.graph.link_relationships(links.as_slice());
        Ok(())
    }
}
