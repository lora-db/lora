//! The [`InMemoryGraph`] data structure: slot-indexed node/relationship
//! storage, adjacency lists, label/type indexes, and the inherent
//! helpers that the trait impls in `super::impls` delegate to.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use lora_ast::Direction;

use crate::{
    DeletedRecordSink, Labels, LoraPoint, MutationEvent, MutationRecorder, NodeId, NodeRecord,
    Properties, PropertyValue, RelationshipId, RelationshipRecord,
};

use super::adjacency::{AdjEntry, AdjList, TypeFilter};
use super::chunked_vec::ChunkedVec;
use super::constraint_catalog::{
    ConstraintCatalog, ConstraintRequest, CreateConstraintError, CreateConstraintOutcome,
    DropConstraintError, DropConstraintOutcome,
};
use super::distinct_stats::DistinctStatsRegistry;
use super::entity_index_store::{
    read_shared, share, write_shared, IndexBundle, IndexRead, IndexWrite,
};
use super::fulltext_index::FulltextRegistry;
use super::hnsw::HnswParams;
use super::index_catalog::{
    CreateIndexError, CreateIndexOutcome, DropIndexError, DropIndexOutcome, IndexCatalog,
    IndexDefinition, IndexRequest, StoredIndexEntity, StoredIndexKind, StoredIndexState,
};
use super::point_index::PointRegistry;
#[cfg(test)]
use super::property_index::PropertyIndexState;
use super::property_index::{PropertyIndexRegistry, UNLABELLED};
use super::secondary_index_maintenance::SecondaryIndexMutation;
use super::sorted_property_index::SortedPropertyIndex;
use super::stats::GraphStats;
use super::text_index::TrigramRegistry;
use super::vector_index::{VectorIndexProvider, VectorIndexRegistry, VectorSimilarity};
use crate::dict::Dicts;
use crate::encoded::{self, Blob, StoredNode, StoredRel};
use crate::{NodeRef, RelRef};

#[derive(Default)]
pub struct InMemoryGraph {
    pub(super) next_node_id: NodeId,
    pub(super) next_rel_id: RelationshipId,

    /// Slot-indexed node storage: `nodes[id as usize]` is the node `id`,
    /// encoded (see [`crate::encoded`]). `None` slots are tombstones from
    /// deletes (we don't compact). Because `next_node_id` is monotonic the
    /// slot at `id` is initialized exactly when `id < next_node_id`.
    ///
    /// [`Self::clone`] (called on every staged write to build a working
    /// copy, and for every reader snapshot) shares this storage: a
    /// [`ChunkedVec`] clone is one refcount bump, so the whole-graph
    /// clone is O(#labels + #relationship types), not O(N). A write
    /// copies only the chunk path it touches, and copying a chunk bumps
    /// one count per record. A record is immutable: changing it stores a
    /// new encoding in its slot (see [`Self::update_node`]).
    pub(super) nodes: ChunkedVec<Option<Blob>>,
    pub(super) relationships: ChunkedVec<Option<Blob>>,
    /// Numbers the labels, relationship types and property keys the
    /// records and adjacency entries refer to. Shared with clones until
    /// one of them sees a new name.
    pub(super) dicts: Dicts,
    /// Live (non-tombstoned) counts kept in sync with `put_*`/`take_*` so
    /// `node_count` / `relationship_count` stay O(1) — without a counter
    /// they'd have to scan the slab.
    pub(super) live_node_count: usize,
    pub(super) live_rel_count: usize,

    /// Adjacency keyed by NodeId. `outgoing[id]` lists the relationships
    /// that leave `id` as `(type, target, relationship id)`; `incoming[id]`
    /// mirrors it with the source as the neighbour. A hop reads only the
    /// list: it never touches a relationship record. See
    /// [`super::adjacency`].
    pub(super) outgoing: ChunkedVec<AdjList>,
    pub(super) incoming: ChunkedVec<AdjList>,

    // secondary indexes
    /// Label -> the (unique, monotonic) node ids that carry it. The inner
    /// `Vec` instead of `BTreeSet` because every node id is inserted at most
    /// once per label (no dedup needed) and every consumer iterates the
    /// whole list anyway — contiguous storage iterates faster than a
    /// tree-of-pointers, and removes via `swap_remove` stay O(degree-of-label).
    pub(super) nodes_by_label: BTreeMap<String, ChunkedVec<NodeId>>,
    pub(super) relationships_by_type: BTreeMap<String, ChunkedVec<RelationshipId>>,

    /// All index machinery — the declared-index catalog, hash-bucket
    /// property registry, and the per-entity-kind secondary index
    /// registries (text, sorted, point, fulltext) plus their active
    /// counters — collapsed into one bundle. See [`IndexBundle`] for
    /// the rationale. The bundle is a packaging-only abstraction:
    /// every field accessed through `self.indexes.<x>` lives at the
    /// same address it would have as a top-level field.
    pub(super) indexes: IndexBundle,

    /// Per-(label / type, property key) distinct-value sketches for the
    /// planner (see [`super::distinct_stats`]). Kept for every key, not
    /// only indexed ones, and a function of the live data alone, so
    /// plans don't depend on index activation, lookup history or
    /// restarts. Cloning is four refcount bumps.
    pub(super) distinct_stats: DistinctStatsRegistry,

    /// Catalog of explicitly-created constraints (CREATE CONSTRAINT).
    /// Deliberately not part of [`IndexBundle`] — constraints describe
    /// data invariants, not indexed access. The fact that uniqueness /
    /// key constraints back range indexes is handled in the
    /// constraint code path, not by the bundle's layout.
    pub(super) constraint_catalog: RwLock<Arc<ConstraintCatalog>>,
    /// Fast-path counter for mutation-time constraint checks. Most
    /// workloads have no constraints installed; this lets the executor
    /// skip taking the catalog lock in that case.
    pub(super) active_constraints: AtomicUsize,

    /// Optional mutation observer. When `Some`, every committed mutation
    /// fans out to this recorder *after* the in-memory state has been
    /// updated. The recorder is not part of the graph's identity, so Clone
    /// and snapshot restore both reset it to `None`.
    pub(super) recorder: Option<Arc<dyn MutationRecorder>>,

    /// Optional sink that sees each node / relationship record just before
    /// a delete drops it. Change feeds use it to report deleted entities
    /// without copying the graph. Like the recorder, it is not part of the
    /// graph's identity and is dropped on clone.
    pub(super) deleted_sink: Option<Arc<dyn DeletedRecordSink>>,
}

impl std::fmt::Debug for InMemoryGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryGraph")
            .field("next_node_id", &self.next_node_id)
            .field("next_rel_id", &self.next_rel_id)
            .field(
                "nodes",
                &self.iter_nodes().map(|(_, n)| n).collect::<Vec<_>>(),
            )
            .field(
                "relationships",
                &self.iter_rels().map(|(_, r)| r).collect::<Vec<_>>(),
            )
            .field("outgoing", &self.outgoing)
            .field("incoming", &self.incoming)
            .field("nodes_by_label", &self.nodes_by_label)
            .field("relationships_by_type", &self.relationships_by_type)
            .field("indexes", &self.indexes)
            .field(
                "active_node_property_indexes",
                &self.active_node_property_index_count(),
            )
            .field(
                "active_relationship_property_indexes",
                &self.active_relationship_property_index_count(),
            )
            .field(
                "index_catalog_entries",
                &self
                    .indexes
                    .catalog
                    .read()
                    .map(|c| c.list().len())
                    .unwrap_or(0),
            )
            .field("active_constraints", &self.active_constraint_count())
            .field(
                "active_fulltext_indexes",
                &self.active_fulltext_index_count(),
            )
            .field("recorder", &self.recorder.as_ref().map(|_| "installed"))
            .finish()
    }
}

impl Clone for InMemoryGraph {
    fn clone(&self) -> Self {
        // Deliberately drop the recorder on clone: a cloned store is a
        // separate identity; it should not silently share the observer.
        Self {
            next_node_id: self.next_node_id,
            next_rel_id: self.next_rel_id,
            nodes: self.nodes.clone(),
            relationships: self.relationships.clone(),
            live_node_count: self.live_node_count,
            live_rel_count: self.live_rel_count,
            outgoing: self.outgoing.clone(),
            incoming: self.incoming.clone(),
            dicts: self.dicts.clone(),
            nodes_by_label: self.nodes_by_label.clone(),
            relationships_by_type: self.relationships_by_type.clone(),
            // Shares every registry and catalog; each is copied on its
            // first write (see `IndexBundle`).
            indexes: self.indexes.clone(),
            distinct_stats: self.distinct_stats.clone(),
            constraint_catalog: share(&self.constraint_catalog),
            active_constraints: AtomicUsize::new(self.active_constraint_count()),
            recorder: None,
            deleted_sink: None,
        }
    }
}

impl InMemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Kept for API compatibility. The chunked storage allocates full
    /// chunks as the graph grows, so there is no repeated doubling for a
    /// capacity hint to avoid.
    pub fn with_capacity_hint(_nodes: usize, _relationships: usize) -> Self {
        Self::default()
    }

    pub fn contains_node(&self, node_id: NodeId) -> bool {
        self.node_at(node_id).is_some()
    }

    pub fn contains_relationship(&self, rel_id: RelationshipId) -> bool {
        self.rel_at(rel_id).is_some()
    }

    /// Install (or clear) the mutation recorder. Passing `None` detaches any
    /// currently-installed recorder. The recorder observes every committed
    /// mutation *after* it has been applied.
    pub fn set_mutation_recorder(&mut self, recorder: Option<Arc<dyn MutationRecorder>>) {
        self.recorder = recorder;
    }

    /// Install (or clear) the [`DeletedRecordSink`].
    pub fn set_deleted_record_sink(&mut self, sink: Option<Arc<dyn DeletedRecordSink>>) {
        self.deleted_sink = sink;
    }

    /// Handle to the currently-installed recorder, if any.
    pub fn mutation_recorder(&self) -> Option<&Arc<dyn MutationRecorder>> {
        self.recorder.as_ref()
    }

    /// Emit a mutation event only if a recorder is installed. The event is
    /// built lazily — callers pass a closure, so when no recorder is
    /// attached we pay only a `None` check and the cost of constructing the
    /// event (labels/properties clones) is avoided.
    #[inline]
    pub(super) fn emit<F: FnOnce() -> MutationEvent>(&self, build: F) {
        if let Some(rec) = &self.recorder {
            rec.record(build());
        }
    }

    fn bump_next_node_id_past(&mut self, id: NodeId) -> Result<(), String> {
        let next = id
            .checked_add(1)
            .ok_or_else(|| format!("node id {id} leaves no valid next node id"))?;
        self.next_node_id = self.next_node_id.max(next);
        Ok(())
    }

    fn bump_next_rel_id_past(&mut self, id: RelationshipId) -> Result<(), String> {
        let next = id
            .checked_add(1)
            .ok_or_else(|| format!("relationship id {id} leaves no valid next relationship id"))?;
        self.next_rel_id = self.next_rel_id.max(next);
        Ok(())
    }

    pub(super) fn try_reserve_next_node_slot(&mut self) -> Option<(NodeId, usize)> {
        let id = self.next_node_id;
        let idx = self.ensure_node_slot_checked(id).ok()?;
        self.bump_next_node_id_past(id).ok()?;
        Some((id, idx))
    }

    pub(super) fn try_reserve_next_rel_slot(&mut self) -> Option<(RelationshipId, usize)> {
        let id = self.next_rel_id;
        let idx = self.ensure_rel_slot_checked(id).ok()?;
        self.bump_next_rel_id_past(id).ok()?;
        Some((id, idx))
    }

    // ---------- Slab access helpers ----------
    //
    // Slots are indexed by id. Readers get views over the encoded
    // records; nothing here hands out the bytes.

    #[inline]
    pub(super) fn node_at(&self, id: NodeId) -> Option<NodeRef<'_>> {
        let blob = self.nodes.get(Self::slot_index(id)?)?.as_ref()?;
        Some(NodeRef::stored(StoredNode::new(id, blob, &self.dicts)))
    }

    #[inline]
    pub(super) fn has_node_at(&self, id: NodeId) -> bool {
        Self::slot_index(id)
            .and_then(|idx| self.nodes.get(idx))
            .is_some_and(|slot| slot.is_some())
    }

    #[inline]
    pub(super) fn rel_at(&self, id: RelationshipId) -> Option<RelRef<'_>> {
        let blob = self.relationships.get(Self::slot_index(id)?)?.as_ref()?;
        Some(RelRef::stored(StoredRel::new(id, blob, &self.dicts)))
    }

    #[inline]
    pub(super) fn has_rel_at(&self, id: RelationshipId) -> bool {
        Self::slot_index(id)
            .and_then(|idx| self.relationships.get(idx))
            .is_some_and(|slot| slot.is_some())
    }

    /// `(source, target)` of a relationship, reading only the start of
    /// its record.
    #[inline]
    pub(super) fn rel_endpoints_at(&self, id: RelationshipId) -> Option<(NodeId, NodeId)> {
        let blob = self.relationships.get(Self::slot_index(id)?)?.as_ref()?;
        Some(encoded::rel_endpoints(blob))
    }

    /// Change a node: decode it, let `change` edit the record, and store
    /// the new encoding. Records are immutable, so snapshots that share
    /// the old one keep reading it.
    pub(super) fn update_node<R>(
        &mut self,
        id: NodeId,
        change: impl FnOnce(&mut NodeRecord) -> R,
    ) -> Option<R> {
        let idx = Self::slot_index(id)?;
        let mut record = self.node_at(id)?.to_record();
        let result = change(&mut record);
        let blob = encoded::encode_node(&record, &mut self.dicts);
        self.nodes[idx] = Some(blob);
        Some(result)
    }

    /// Set or remove one property of a node, rewriting only that entry of
    /// its record. Returns the property's previous value; `None` when the
    /// node does not exist or there was nothing to remove.
    pub(super) fn edit_node_property(
        &mut self,
        id: NodeId,
        key: &str,
        edit: encoded::PropEdit<'_>,
    ) -> Option<Option<PropertyValue>> {
        let idx = Self::slot_index(id)?;
        let blob = self.nodes.get(idx)?.as_ref()?;
        let (blob, old) = encoded::edit_property(
            blob,
            encoded::RecordKind::Node,
            &mut self.dicts.keys,
            key,
            edit,
        )?;
        self.nodes[idx] = Some(blob);
        Some(old)
    }

    pub(super) fn edit_rel_property(
        &mut self,
        id: RelationshipId,
        key: &str,
        edit: encoded::PropEdit<'_>,
    ) -> Option<Option<PropertyValue>> {
        let idx = Self::slot_index(id)?;
        let blob = self.relationships.get(idx)?.as_ref()?;
        let (blob, old) = encoded::edit_property(
            blob,
            encoded::RecordKind::Relationship,
            &mut self.dicts.keys,
            key,
            edit,
        )?;
        self.relationships[idx] = Some(blob);
        Some(old)
    }

    /// Resize the node-keyed Vecs so `id as usize` is in range. Adjacency
    /// lists are kept in lockstep with `nodes`, so a freshly-grown slot has
    /// empty outgoing/incoming Vecs ready to receive edges.
    fn slot_len_for_id(id: u64, kind: &str) -> Result<usize, String> {
        let idx = usize::try_from(id)
            .map_err(|_| format!("{kind} id {id} does not fit in usize on this platform"))?;
        idx.checked_add(1)
            .ok_or_else(|| format!("{kind} id {id} leaves no valid slab slot"))
    }

    #[inline]
    fn slot_index(id: u64) -> Option<usize> {
        usize::try_from(id).ok()
    }

    fn ensure_node_slot_checked(&mut self, id: NodeId) -> Result<usize, String> {
        let target = Self::slot_len_for_id(id, "node")?;
        if self.nodes.len() < target {
            let additional = target - self.nodes.len();
            self.nodes.try_reserve_exact(additional).map_err(|e| {
                format!("node id {id} requires {target} slots, but allocation failed: {e}")
            })?;
            self.outgoing.try_reserve_exact(additional).map_err(|e| {
                format!(
                    "node id {id} requires {target} adjacency slots, but allocation failed: {e}"
                )
            })?;
            self.incoming.try_reserve_exact(additional).map_err(|e| {
                format!(
                    "node id {id} requires {target} adjacency slots, but allocation failed: {e}"
                )
            })?;
            self.nodes.resize_with(target, || None);
            self.outgoing.resize_with(target, AdjList::new);
            self.incoming.resize_with(target, AdjList::new);
        }
        Ok(target - 1)
    }

    fn ensure_rel_slot_checked(&mut self, id: RelationshipId) -> Result<usize, String> {
        let target = Self::slot_len_for_id(id, "relationship")?;
        if self.relationships.len() < target {
            self.relationships
                .try_reserve_exact(target - self.relationships.len())
                .map_err(|e| {
                    format!(
                        "relationship id {id} requires {target} slots, but allocation failed: {e}"
                    )
                })?;
            self.relationships.resize_with(target, || None);
        }
        Ok(target - 1)
    }

    pub(super) fn put_node_checked(&mut self, id: NodeId, node: &NodeRecord) -> Result<(), String> {
        let idx = self.ensure_node_slot_checked(id)?;
        self.put_node_at_slot(idx, node);
        Ok(())
    }

    pub(super) fn put_rel_checked(
        &mut self,
        id: RelationshipId,
        rel: &RelationshipRecord,
    ) -> Result<(), String> {
        let idx = self.ensure_rel_slot_checked(id)?;
        self.put_rel_at_slot(idx, rel);
        Ok(())
    }

    pub(super) fn put_node_at_slot(&mut self, idx: usize, node: &NodeRecord) {
        let was_present = self.nodes[idx].is_some();
        self.nodes[idx] = Some(encoded::encode_node(node, &mut self.dicts));
        if !was_present {
            self.live_node_count += 1;
        }
    }

    pub(super) fn put_rel_at_slot(&mut self, idx: usize, rel: &RelationshipRecord) {
        let was_present = self.relationships[idx].is_some();
        self.relationships[idx] = Some(encoded::encode_rel(rel, &mut self.dicts));
        if !was_present {
            self.live_rel_count += 1;
        }
    }

    pub(super) fn take_node(&mut self, id: NodeId) -> Option<NodeRecord> {
        let idx = Self::slot_index(id)?;
        let removed = self.nodes.get_mut(idx).and_then(|s| s.take());
        if removed.is_some() {
            self.live_node_count -= 1;
            // Also clear the per-id adjacency entries so the memory is reclaimed
            // on the typical "delete every node" pattern. We deliberately do not
            // shrink the outer Vec — leaving the slot lets new ids reuse the
            // same index without growth churn (and `next_node_id` is monotonic
            // anyway, so no immediate reuse).
            if let Some(out) = self.outgoing.get_mut(idx) {
                out.clear();
            }
            if let Some(inc) = self.incoming.get_mut(idx) {
                inc.clear();
            }
        }
        removed.map(|blob| StoredNode::new(id, &blob, &self.dicts).to_record())
    }

    pub(super) fn take_rel(&mut self, id: RelationshipId) -> Option<RelationshipRecord> {
        let idx = Self::slot_index(id)?;
        let removed = self.relationships.get_mut(idx).and_then(|s| s.take());
        if removed.is_some() {
            self.live_rel_count -= 1;
        }
        removed.map(|blob| StoredRel::new(id, &blob, &self.dicts).to_record())
    }

    #[inline]
    pub(super) fn outgoing_at(&self, id: NodeId) -> Option<&AdjList> {
        self.outgoing.get(Self::slot_index(id)?)
    }

    #[inline]
    pub(super) fn incoming_at(&self, id: NodeId) -> Option<&AdjList> {
        self.incoming.get(Self::slot_index(id)?)
    }

    /// Visit the entries of one adjacency list that pass `filter`.
    /// `skip_self_loops` is set for the incoming half of an undirected
    /// walk, whose outgoing half already reported the node's self-loops.
    #[inline]
    fn try_for_each_adjacent_entry<F, E>(
        node_id: NodeId,
        filter: &TypeFilter,
        adj: &AdjList,
        skip_self_loops: bool,
        visit: &mut F,
    ) -> Result<(), E>
    where
        F: FnMut(RelationshipId, NodeId) -> Result<(), E>,
    {
        for entry in adj.iter() {
            if skip_self_loops && entry.neighbour == node_id {
                continue;
            }
            if !filter.matches(entry.type_id) {
                continue;
            }
            visit(entry.rel, entry.neighbour)?;
        }
        Ok(())
    }

    #[inline]
    pub(super) fn try_for_each_adjacent_id_unchecked<F, E>(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
        mut visit: F,
    ) -> Result<(), E>
    where
        F: FnMut(RelationshipId, NodeId) -> Result<(), E>,
    {
        let filter = TypeFilter::resolve(&self.dicts.types, types);
        if matches!(filter, TypeFilter::Nothing) {
            return Ok(());
        }
        if !matches!(direction, Direction::Left) {
            if let Some(adj) = self.outgoing_at(node_id) {
                Self::try_for_each_adjacent_entry(node_id, &filter, adj, false, &mut visit)?;
            }
        }
        if !matches!(direction, Direction::Right) {
            if let Some(adj) = self.incoming_at(node_id) {
                let skip_self_loops = matches!(direction, Direction::Undirected);
                Self::try_for_each_adjacent_entry(
                    node_id,
                    &filter,
                    adj,
                    skip_self_loops,
                    &mut visit,
                )?;
            }
        }

        Ok(())
    }

    #[inline]
    pub(super) fn try_for_each_adjacent_id<F, E>(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
        visit: F,
    ) -> Result<(), E>
    where
        F: FnMut(RelationshipId, NodeId) -> Result<(), E>,
    {
        if self.node_at(node_id).is_none() {
            return Ok(());
        }
        self.try_for_each_adjacent_id_unchecked(node_id, direction, types, visit)
    }

    pub(super) fn iter_node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| i as NodeId))
    }

    pub(super) fn iter_node_refs(&self) -> impl Iterator<Item = NodeRef<'_>> + '_ {
        self.iter_nodes().map(|(_, node)| node)
    }

    pub(super) fn iter_rel_ids(&self) -> impl Iterator<Item = RelationshipId> + '_ {
        self.relationships
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| i as RelationshipId))
    }

    pub(super) fn iter_rel_refs(&self) -> impl Iterator<Item = RelRef<'_>> + '_ {
        self.iter_rels().map(|(_, rel)| rel)
    }

    pub(super) fn iter_nodes(&self) -> impl Iterator<Item = (NodeId, NodeRef<'_>)> + '_ {
        let dicts = &self.dicts;
        self.nodes.iter().enumerate().filter_map(move |(i, slot)| {
            let blob = slot.as_ref()?;
            let id = i as NodeId;
            Some((id, NodeRef::stored(StoredNode::new(id, blob, dicts))))
        })
    }

    pub(super) fn iter_rels(&self) -> impl Iterator<Item = (RelationshipId, RelRef<'_>)> + '_ {
        let dicts = &self.dicts;
        self.relationships
            .iter()
            .enumerate()
            .filter_map(move |(i, slot)| {
                let blob = slot.as_ref()?;
                let id = i as RelationshipId;
                Some((id, RelRef::stored(StoredRel::new(id, blob, dicts))))
            })
    }

    /// Add an entry to `node_id`'s outgoing (or incoming) list. Relies on
    /// the monotonic-id invariant: relationship ids are allocated once and
    /// never re-used, so the list can never see a duplicate.
    fn adjacency_push(&mut self, node_id: NodeId, outgoing: bool, entry: AdjEntry) {
        if let Ok(idx) = self.ensure_node_slot_checked(node_id) {
            let lists = if outgoing {
                &mut self.outgoing
            } else {
                &mut self.incoming
            };
            lists[idx].push(entry);
        }
    }

    fn adjacency_remove(&mut self, node_id: NodeId, outgoing: bool, rel_id: RelationshipId) {
        let lists = if outgoing {
            &mut self.outgoing
        } else {
            &mut self.incoming
        };
        if let Some(list) = Self::slot_index(node_id).and_then(|idx| lists.get_mut(idx)) {
            list.remove(rel_id);
        }
    }

    pub(super) fn normalize_labels(labels: Vec<String>) -> Labels {
        let mut out = Labels::new();
        for label in &labels {
            let label = label.trim();
            if !label.is_empty() && !out.has(label) {
                out.push(label);
            }
        }
        out
    }

    pub(super) fn insert_node_label_index(&mut self, node_id: NodeId, label: &str) {
        // Hot path: skip the `String` alloc when the label bucket already
        // exists. The monotonic-id invariant on the create path guarantees
        // `node_id` is unique, so we push unconditionally; the previous
        // `contains` guard turned bulk CREATE into O(n²).
        if let Some(bucket) = self.nodes_by_label.get_mut(label) {
            bucket.push(node_id);
        } else {
            self.nodes_by_label
                .insert(label.to_string(), std::iter::once(node_id).collect());
        }
    }

    fn remove_node_label_index(&mut self, node_id: NodeId, label: &str) {
        if let Some(ids) = self.nodes_by_label.get_mut(label) {
            let pos = ids.iter().position(|&id| id == node_id);
            if let Some(pos) = pos {
                ids.swap_remove(pos);
            }
            if ids.is_empty() {
                self.nodes_by_label.remove(label);
            }
        }
    }

    fn insert_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        // See `insert_node_label_index` for the same hot-path rationale.
        if let Some(bucket) = self.relationships_by_type.get_mut(rel_type) {
            bucket.push(rel_id);
        } else {
            self.relationships_by_type
                .insert(rel_type.to_string(), std::iter::once(rel_id).collect());
        }
    }

    fn remove_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        if let Some(ids) = self.relationships_by_type.get_mut(rel_type) {
            let pos = ids.iter().position(|&id| id == rel_id);
            if let Some(pos) = pos {
                ids.swap_remove(pos);
            }
            if ids.is_empty() {
                self.relationships_by_type.remove(rel_type);
            }
        }
    }

    /// Ids of `label` nodes whose `key` equals `value`, from the scoped
    /// hash index. `None` when that index is not active for `key` or
    /// `value` has no index image; the caller must then scan.
    pub(super) fn indexed_node_ids(
        &self,
        label: &str,
        key: &str,
        value: &PropertyValue,
    ) -> Option<Vec<NodeId>> {
        super::property_index::PropertyIndexKey::from_value(value)?;
        let indexes = self.indexes_read();
        if !indexes.node_properties.is_active(key) {
            return None;
        }
        Some(
            indexes
                .node_properties
                .scoped_ids_for(label, key, value)
                .map(|ids| ids.to_vec())
                .unwrap_or_default(),
        )
    }

    /// Relationship counterpart of [`Self::indexed_node_ids`].
    pub(super) fn indexed_rel_ids(
        &self,
        rel_type: &str,
        key: &str,
        value: &PropertyValue,
    ) -> Option<Vec<RelationshipId>> {
        super::property_index::PropertyIndexKey::from_value(value)?;
        let indexes = self.indexes_read();
        if !indexes.relationship_properties.is_active(key) {
            return None;
        }
        Some(
            indexes
                .relationship_properties
                .scoped_ids_for(rel_type, key, value)
                .map(|ids| ids.to_vec())
                .unwrap_or_default(),
        )
    }

    pub(super) fn indexes_read(&self) -> std::sync::RwLockReadGuard<'_, PropertyIndexRegistry> {
        self.indexes
            .properties
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn indexes_write(&self) -> RwLockWriteGuard<'_, PropertyIndexRegistry> {
        self.indexes
            .properties
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn indexes_mut(&mut self) -> &mut PropertyIndexRegistry {
        self.indexes
            .properties
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[inline]
    pub(super) fn active_node_property_index_count(&self) -> usize {
        self.indexes
            .active_node_property_indexes
            .load(Ordering::Relaxed)
    }

    #[inline]
    pub(super) fn active_relationship_property_index_count(&self) -> usize {
        self.indexes
            .active_relationship_property_indexes
            .load(Ordering::Relaxed)
    }

    #[inline]
    pub(super) fn active_constraint_count(&self) -> usize {
        self.active_constraints.load(Ordering::Relaxed)
    }

    #[inline]
    pub(super) fn has_active_constraints(&self) -> bool {
        self.active_constraint_count() != 0
    }

    #[inline]
    pub(super) fn active_fulltext_index_count(&self) -> usize {
        self.indexes.active_fulltext_indexes.load(Ordering::Relaxed)
    }

    #[inline]
    pub(super) fn has_active_fulltext_indexes(&self) -> bool {
        self.active_fulltext_index_count() != 0
    }

    pub(super) fn node_property_index_is_active(&mut self, key: &str) -> bool {
        self.active_node_property_index_count() != 0
            && self.indexes_mut().node_properties.is_active(key)
    }

    pub(super) fn relationship_property_index_is_active(&mut self, key: &str) -> bool {
        self.active_relationship_property_index_count() != 0
            && self.indexes_mut().relationship_properties.is_active(key)
    }

    /// Give `key` its across-labels map, for lookups that name no label.
    /// Call after [`Self::ensure_node_property_index`].
    pub(super) fn ensure_any_scope_node_property_index(&self, key: &str) {
        if self.indexes_read().node_properties.any_scope_is_active(key) {
            return;
        }
        self.indexes_write().node_properties.activate_any_scope(key);
    }

    pub(super) fn ensure_any_scope_relationship_property_index(&self, key: &str) {
        if self
            .indexes_read()
            .relationship_properties
            .any_scope_is_active(key)
        {
            return;
        }
        self.indexes_write()
            .relationship_properties
            .activate_any_scope(key);
    }

    pub(super) fn ensure_node_property_index(&self, key: &str) {
        {
            let indexes = self.indexes_read();
            if indexes.node_properties.is_active(key) {
                return;
            }
        }

        let mut indexes = self.indexes_write();
        if indexes.node_properties.is_active(key) {
            return;
        }

        indexes.node_properties.insert_bulk(key, || {
            self.iter_nodes().filter_map(|(id, node)| {
                let value = node.properties().get(key)?;
                Some((id, node.labels().strs(), value))
            })
        });
        if indexes.node_properties.activate(key) {
            self.indexes
                .active_node_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn ensure_relationship_property_index(&self, key: &str) {
        {
            let indexes = self.indexes_read();
            if indexes.relationship_properties.is_active(key) {
                return;
            }
        }

        let mut indexes = self.indexes_write();
        if indexes.relationship_properties.is_active(key) {
            return;
        }

        indexes.relationship_properties.insert_bulk(key, || {
            self.iter_rels().filter_map(|(id, rel)| {
                let value = rel.properties().get(key)?;
                Some((id, [rel.rel_type()], value))
            })
        });
        if indexes.relationship_properties.activate(key) {
            self.indexes
                .active_relationship_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn index_catalog_read(&self) -> IndexRead<'_, IndexCatalog> {
        read_shared(&self.indexes.catalog)
    }

    /// Mutable access copies the catalog first if a clone still shares it.
    pub(super) fn index_catalog_write(&self) -> IndexWrite<'_, IndexCatalog> {
        write_shared(&self.indexes.catalog)
    }

    pub(super) fn constraint_catalog_read(&self) -> IndexRead<'_, ConstraintCatalog> {
        read_shared(&self.constraint_catalog)
    }

    /// Mutable access copies the catalog first if a clone still shares it.
    pub(super) fn constraint_catalog_write(&self) -> IndexWrite<'_, ConstraintCatalog> {
        write_shared(&self.constraint_catalog)
    }

    /// Register an explicitly-declared index in the catalog and, when
    /// applicable, force the underlying property-index buckets to be
    /// populated so equality lookups can use them immediately.
    ///
    /// Named with a `register_` prefix to avoid colliding with the
    /// trait method `GraphStorageMut::create_index` — the trait impl
    /// in `impls.rs` delegates here.
    #[allow(clippy::result_large_err)]
    pub(super) fn register_index(
        &self,
        request: IndexRequest,
        if_not_exists: bool,
    ) -> Result<CreateIndexOutcome, CreateIndexError> {
        self.register_index_with_recording(request, if_not_exists, true)
    }

    #[allow(clippy::result_large_err)]
    fn register_index_with_recording(
        &self,
        request: IndexRequest,
        if_not_exists: bool,
        record_event: bool,
    ) -> Result<CreateIndexOutcome, CreateIndexError> {
        let request_for_event = record_event.then(|| request.clone());
        let outcome = {
            let mut catalog = self.index_catalog_write();
            catalog.try_create(request, if_not_exists)?
        };

        if let CreateIndexOutcome::Created(def) = &outcome {
            self.populate_index_data(def);
        }

        // Both Created and NoOpExists are committed catalog states; we
        // log only Created because NoOpExists implies a redundant DDL
        // that adds nothing to durable state.
        if matches!(outcome, CreateIndexOutcome::Created(_)) {
            if let Some(request_for_event) = request_for_event {
                self.emit(|| crate::MutationEvent::CreateIndex {
                    request: request_for_event,
                    if_not_exists,
                });
            }
        }

        Ok(outcome)
    }

    /// Replay a CreateIndex event against an empty graph. Mirrors the
    /// `replay_create_node` shape: callers must invoke before installing
    /// a recorder so we don't re-emit during recovery.
    #[doc(hidden)]
    pub fn replay_create_index(
        &mut self,
        request: IndexRequest,
        if_not_exists: bool,
    ) -> Result<(), String> {
        if self.recorder.is_some() {
            return Err("cannot replay create_index while a mutation recorder is installed".into());
        }
        self.register_index(request, if_not_exists)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Replay a DropIndex event.
    #[doc(hidden)]
    pub fn replay_drop_index(&mut self, name: &str, if_exists: bool) -> Result<(), String> {
        if self.recorder.is_some() {
            return Err("cannot replay drop_index while a mutation recorder is installed".into());
        }
        self.drop_named_index(name, if_exists)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Register a constraint. For uniqueness/key kinds this also
    /// registers a backing RANGE index in the index catalog under the
    /// same name. Validation of existing data is the caller's
    /// responsibility (the enforcement layer runs a pre-create scan
    /// before this method commits).
    pub(super) fn register_constraint(
        &self,
        request: ConstraintRequest,
        if_not_exists: bool,
    ) -> Result<CreateConstraintOutcome, CreateConstraintError> {
        // Constraint-level conflicts (22N65/66/67) take precedence over
        // index-catalog conflicts: if the request collides with an
        // existing *constraint* shape or name, we never get to the
        // backing-index step.
        {
            let constraint_catalog = self.constraint_catalog_read();
            if let Some(existing) = constraint_catalog.find_equivalent(&request) {
                let cloned = existing.clone();
                drop(constraint_catalog);
                if if_not_exists {
                    return Ok(CreateConstraintOutcome::NoOpExists(cloned));
                }
                return Err(CreateConstraintError::EquivalentConstraintExists(
                    cloned.name,
                ));
            }
            if let Some(existing) = constraint_catalog.get(&request.name) {
                let cloned = existing.clone();
                drop(constraint_catalog);
                if if_not_exists {
                    return Ok(CreateConstraintOutcome::NoOpExists(cloned));
                }
                return Err(CreateConstraintError::DuplicateName(cloned.name));
            }
            if let Some(existing) = constraint_catalog.find_same_schema(&request) {
                let cloned = existing.clone();
                drop(constraint_catalog);
                if super::constraint_catalog::kinds_conflict_for_validation(
                    &cloned.kind,
                    &request.kind,
                ) {
                    if if_not_exists {
                        return Ok(CreateConstraintOutcome::NoOpExists(cloned));
                    }
                    return Err(CreateConstraintError::ConflictingConstraint(cloned.name));
                }
            }
        }

        // Index-catalog conflicts only matter for constraints that need
        // a backing range index. The catalog won't yet own one for this
        // request — that registration happens below — so any existing
        // entry under the same name or schema is from a foreign index.
        if request.kind.requires_backing_index() {
            let idx_catalog = self.index_catalog_read();
            if idx_catalog.get(&request.name).is_some() {
                return Err(CreateConstraintError::DuplicateIndexName(
                    request.name.clone(),
                ));
            }
            let conflict = idx_catalog.list().into_iter().find(|def| {
                def.kind == StoredIndexKind::Range
                    && def.entity == request.entity
                    && def.label.as_deref() == Some(request.label.as_str())
                    && def.properties == request.properties
                    && def.name != request.name
            });
            drop(idx_catalog);
            if let Some(def) = conflict {
                return Err(CreateConstraintError::BackingIndexConflict(format!(
                    "(:{} {{{}}}) already covered by index `{}`",
                    request.label,
                    request.properties.join(", "),
                    def.name,
                )));
            }
        }

        let owns_backing = request.kind.requires_backing_index();
        let request_for_event = request.clone();
        let outcome = {
            let mut catalog = self.constraint_catalog_write();
            catalog.try_create(request, if_not_exists)?
        };

        if let CreateConstraintOutcome::Created(def) = &outcome {
            // Pre-create data scan: if the live graph already violates
            // the constraint, fail and roll back the catalog write.
            // Creating constraints when conflicting data exists fails
            // before the catalog change is retained (22N77/79/80).
            if let Err(violation) = self.validate_existing_data_for_constraint(def) {
                let mut catalog = self.constraint_catalog_write();
                let _ = catalog.try_drop(&def.name, true);
                return Err(CreateConstraintError::DataViolation(violation.to_string()));
            }
        }

        if let CreateConstraintOutcome::Created(def) = &outcome {
            if owns_backing {
                // Register a backing RANGE index under the same name. This
                // is an implementation detail of the constraint, so WAL and
                // snapshot replay record only the constraint mutation.
                let idx_request = IndexRequest {
                    explicit_name: Some(def.name.clone()),
                    kind: StoredIndexKind::Range,
                    entity: def.entity,
                    label: Some(def.label.clone()),
                    additional_labels: Vec::new(),
                    properties: def.properties.clone(),
                    options: Default::default(),
                };
                // Errors from the backing-index registration unwind the
                // constraint registration to keep the two catalogs in
                // step.
                if let Err(err) = self.register_index_with_recording(idx_request, true, false) {
                    let mut catalog = self.constraint_catalog_write();
                    let _ = catalog.try_drop(&def.name, true);
                    return Err(CreateConstraintError::BackingIndexConflict(err.to_string()));
                }
            }
            self.emit(|| crate::MutationEvent::CreateConstraint {
                request: request_for_event,
                if_not_exists,
            });
            self.active_constraints.fetch_add(1, Ordering::Relaxed);
        }

        Ok(outcome)
    }

    /// Replay a CreateConstraint event against a recorder-detached graph.
    #[doc(hidden)]
    pub fn replay_create_constraint(
        &mut self,
        request: ConstraintRequest,
        if_not_exists: bool,
    ) -> Result<(), String> {
        if self.recorder.is_some() {
            return Err(
                "cannot replay create_constraint while a mutation recorder is installed".into(),
            );
        }
        self.register_constraint(request, if_not_exists)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Replay a DropConstraint event.
    #[doc(hidden)]
    pub fn replay_drop_constraint(&mut self, name: &str, if_exists: bool) -> Result<(), String> {
        if self.recorder.is_some() {
            return Err(
                "cannot replay drop_constraint while a mutation recorder is installed".into(),
            );
        }
        self.drop_named_constraint(name, if_exists)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Inverse of [`Self::register_constraint`]. Cascades to the backing
    /// range index when one is owned.
    pub(super) fn drop_named_constraint(
        &self,
        name: &str,
        if_exists: bool,
    ) -> Result<DropConstraintOutcome, DropConstraintError> {
        let outcome = {
            let mut catalog = self.constraint_catalog_write();
            catalog.try_drop(name, if_exists)?
        };
        if let DropConstraintOutcome::Dropped(def) = &outcome {
            if let Some(index_name) = def.owned_index.as_deref() {
                // The backing index is owned exclusively by the
                // constraint, so dropping it is unconditional.
                let _ = self.drop_named_index_inner(index_name, true, false);
            }
            self.active_constraints.fetch_sub(1, Ordering::Relaxed);
            self.emit(|| crate::MutationEvent::DropConstraint {
                name: name.to_string(),
                if_exists,
            });
        }
        Ok(outcome)
    }

    /// Inverse of [`Self::register_index`]. Removes the catalog entry
    /// and (for RANGE) leaves the underlying property-index buckets in
    /// place — they may still be needed for lazy-activation lookups
    /// even after the explicit DDL declaration is gone.
    pub(super) fn drop_named_index(
        &self,
        name: &str,
        if_exists: bool,
    ) -> Result<DropIndexOutcome, DropIndexError> {
        self.drop_named_index_inner(name, if_exists, true)
    }

    fn drop_named_index_inner(
        &self,
        name: &str,
        if_exists: bool,
        emit_event: bool,
    ) -> Result<DropIndexOutcome, DropIndexError> {
        if let Some(owner) = self
            .constraint_catalog_read()
            .constraint_owning_index(name)
            .cloned()
        {
            return Err(DropIndexError::ConstraintOwned {
                index: name.to_string(),
                constraint: owner.name,
            });
        }

        let outcome = {
            let mut catalog = self.index_catalog_write();
            catalog.try_drop(name, if_exists)?
        };
        if let DropIndexOutcome::Dropped(def) = &outcome {
            // Release backing structures keyed off the dropped def.
            match def.kind {
                StoredIndexKind::Text => {
                    if let Some(label) = def.label.as_deref() {
                        for prop in &def.properties {
                            self.deactivate_text_scope(def.entity, label, prop);
                        }
                    }
                }
                StoredIndexKind::Range => {
                    if let Some(label) = def.label.as_deref() {
                        for prop in &def.properties {
                            self.deactivate_sorted_scope(def.entity, label, prop);
                        }
                    }
                }
                StoredIndexKind::Point => {
                    if let Some(label) = def.label.as_deref() {
                        for prop in &def.properties {
                            self.deactivate_point_scope(def.entity, label, prop);
                        }
                    }
                }
                StoredIndexKind::Lookup => {
                    // Lookup rides on the eagerly-maintained label/type
                    // indexes; nothing to release.
                }
                StoredIndexKind::Vector => {
                    self.deactivate_vector_index(def.entity, &def.name);
                }
                StoredIndexKind::Fulltext => {
                    self.deactivate_fulltext_index(def.entity, &def.name);
                }
            }
            if emit_event {
                self.emit(|| crate::MutationEvent::DropIndex {
                    name: name.to_string(),
                    if_exists,
                });
            }
        }
        Ok(outcome)
    }

    /// `(entity, key)` pairs whose hash property index a declaration keeps
    /// active: every property of a RANGE index in the catalog, which
    /// includes the backing indexes of uniqueness / key constraints. These
    /// are the only hash indexes a restart rebuilds; any other active key
    /// was activated implicitly by an equality lookup.
    pub(super) fn declared_property_index_keys(
        &self,
    ) -> std::collections::BTreeSet<(StoredIndexEntity, String)> {
        self.index_catalog_read()
            .list()
            .into_iter()
            .filter(|def| def.kind == StoredIndexKind::Range)
            .flat_map(|def| {
                let entity = def.entity;
                def.properties.into_iter().map(move |p| (entity, p))
            })
            .collect()
    }

    fn populate_index_data(&self, def: &IndexDefinition) {
        // RANGE: piggy-back on the existing lazy property-index buckets.
        // TEXT: build a trigram inverted index over the existing entity
        //       data for the (label, property) tuple.
        // POINT: build a grid-bucket spatial index over the existing
        //        entity data.
        // LOOKUP: catalog-only; existing label/type indexes already
        //         answer the predicates.
        match def.kind {
            StoredIndexKind::Range => {
                for key in &def.properties {
                    match def.entity {
                        StoredIndexEntity::Node => self.ensure_node_property_index(key),
                        StoredIndexEntity::Relationship => {
                            self.ensure_relationship_property_index(key)
                        }
                    }
                    if let Some(label) = def.label.as_deref() {
                        self.activate_sorted_scope(def.entity, label, key);
                    }
                }
            }
            StoredIndexKind::Text => {
                let label = match def.label.as_deref() {
                    Some(l) => l,
                    None => return,
                };
                for property in &def.properties {
                    self.activate_text_scope(def.entity, label, property);
                }
            }
            StoredIndexKind::Point => {
                let label = match def.label.as_deref() {
                    Some(l) => l,
                    None => return,
                };
                let cell_size = PointRegistry::cell_size_from_options(&def.options);
                for property in &def.properties {
                    self.activate_point_scope(def.entity, label, property, cell_size);
                }
            }
            StoredIndexKind::Fulltext => {
                let labels: Vec<String> = def.all_labels().map(String::from).collect();
                if labels.is_empty() {
                    return;
                }
                self.activate_fulltext_index(def.entity, &def.name, &labels, &def.properties);
            }
            StoredIndexKind::Vector => {
                let label = match def.label.as_deref() {
                    Some(l) => l,
                    None => return,
                };
                let property = match def.properties.first() {
                    Some(p) => p.as_str(),
                    None => return,
                };
                let similarity = VectorSimilarity::from_options(&def.options)
                    .unwrap_or(VectorSimilarity::Cosine);
                let provider = VectorIndexProvider::from_options(&def.options)
                    .unwrap_or(VectorIndexProvider::Flat);
                let hnsw = HnswParams::from_options(&def.options);
                let lazy = matches!(
                    def.options.get("vector.populate.async"),
                    Some(super::index_catalog::IndexConfigValue::Bool(true))
                );
                self.activate_vector_index(
                    def.entity, &def.name, label, property, similarity, provider, hnsw, lazy,
                );
                if lazy {
                    self.index_catalog_write()
                        .set_state(&def.name, StoredIndexState::Populating);
                }
            }
            // LOOKUP rides on the label/type indexes maintained eagerly.
            StoredIndexKind::Lookup => {}
        }
    }

    pub(super) fn text_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexRead<'_, TrigramRegistry> {
        self.indexes.text.read(entity)
    }

    pub(super) fn text_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexWrite<'_, TrigramRegistry> {
        self.indexes.text.write(entity)
    }

    pub(super) fn fulltext_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexRead<'_, FulltextRegistry> {
        self.indexes.fulltext.read(entity)
    }

    #[allow(dead_code)]
    pub(super) fn fulltext_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexWrite<'_, FulltextRegistry> {
        self.indexes.fulltext.write(entity)
    }

    fn activate_text_scope(&self, entity: StoredIndexEntity, label: &str, property: &str) {
        if !self.text_indexes_write(entity).add_scope(label, property) {
            return;
        }

        let backfill: Vec<(u64, String)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| node.labels().strs().any(|l| l == label))
                .filter_map(|(id, node)| match node.properties().get(property) {
                    Some(crate::ValueRef::String(value)) => Some((id, value.to_owned())),
                    _ => None,
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type() == label)
                .filter_map(|(id, rel)| match rel.properties().get(property) {
                    Some(crate::ValueRef::String(value)) => Some((id, value.to_owned())),
                    _ => None,
                })
                .collect(),
        };

        let mut registry = self.text_indexes_write(entity);
        for (id, value) in backfill {
            registry.insert(label, property, id, &value);
        }
    }

    /// Drop a (label, property) text scope, decrementing the refcount.
    pub(super) fn deactivate_text_scope(
        &self,
        entity: StoredIndexEntity,
        label: &str,
        property: &str,
    ) {
        self.text_indexes_write(entity)
            .remove_scope(label, property);
    }

    fn activate_fulltext_index(
        &self,
        entity: StoredIndexEntity,
        name: &str,
        labels: &[String],
        properties: &[String],
    ) {
        use super::fulltext_index::{term_counts_for_properties, TermCounts};

        {
            let mut registry = self.fulltext_indexes_write(entity);
            registry.register(name.to_string(), labels.to_vec(), properties.to_vec());
        }
        self.indexes
            .active_fulltext_indexes
            .fetch_add(1, Ordering::Relaxed);

        // Backfill: walk every entity matching any label, tokenise covered
        // string properties, install one posting batch per entity.
        let backfill: Vec<(u64, TermCounts)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| {
                    labels
                        .iter()
                        .any(|wanted| node.labels().strs().any(|l| l == wanted))
                })
                .map(|(id, node)| {
                    let counts = term_counts_for_properties(node.properties(), properties);
                    (id, counts)
                })
                .filter(|(_, c)| !c.is_empty())
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| labels.iter().any(|wanted| wanted == rel.rel_type()))
                .map(|(id, rel)| {
                    let counts = term_counts_for_properties(rel.properties(), properties);
                    (id, counts)
                })
                .filter(|(_, c)| !c.is_empty())
                .collect(),
        };

        let mut registry = self.fulltext_indexes_write(entity);
        if let Some(index) = registry.get_mut(name) {
            for (id, counts) in backfill {
                index.reindex_entity(id, counts);
            }
        }
    }

    pub(super) fn deactivate_fulltext_index(&self, entity: StoredIndexEntity, name: &str) {
        self.fulltext_indexes_write(entity).deregister(name);
        self.indexes
            .active_fulltext_indexes
            .fetch_sub(1, Ordering::Relaxed);
    }

    pub(super) fn sorted_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexRead<'_, SortedPropertyIndex> {
        self.indexes.sorted.read(entity)
    }

    pub(super) fn sorted_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexWrite<'_, SortedPropertyIndex> {
        self.indexes.sorted.write(entity)
    }

    fn activate_sorted_scope(&self, entity: StoredIndexEntity, label: &str, property: &str) {
        if !self.sorted_indexes_write(entity).add_scope(label, property) {
            return;
        }

        let backfill: Vec<(u64, PropertyValue)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| node.labels().strs().any(|l| l == label))
                .filter_map(|(id, node)| {
                    node.properties()
                        .get(property)
                        .map(|value| (id, value.to_owned()))
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type() == label)
                .filter_map(|(id, rel)| {
                    rel.properties()
                        .get(property)
                        .map(|value| (id, value.to_owned()))
                })
                .collect(),
        };

        let mut registry = self.sorted_indexes_write(entity);
        for (id, value) in backfill {
            registry.insert(label, property, id, &value);
        }
    }

    pub(super) fn deactivate_sorted_scope(
        &self,
        entity: StoredIndexEntity,
        label: &str,
        property: &str,
    ) {
        self.sorted_indexes_write(entity)
            .remove_scope(label, property);
    }

    pub(super) fn point_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexRead<'_, PointRegistry> {
        self.indexes.point.read(entity)
    }

    pub(super) fn point_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexWrite<'_, PointRegistry> {
        self.indexes.point.write(entity)
    }

    fn activate_point_scope(
        &self,
        entity: StoredIndexEntity,
        label: &str,
        property: &str,
        cell_size: Option<f64>,
    ) {
        if !self
            .point_indexes_write(entity)
            .add_scope(label, property, cell_size)
        {
            return;
        }

        let backfill: Vec<(u64, LoraPoint)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| node.labels().strs().any(|l| l == label))
                .filter_map(|(id, node)| {
                    match node.properties().get(property).map(|v| v.to_owned()) {
                        Some(PropertyValue::Point(point)) => Some((id, point)),
                        _ => None,
                    }
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type() == label)
                .filter_map(|(id, rel)| {
                    match rel.properties().get(property).map(|v| v.to_owned()) {
                        Some(PropertyValue::Point(point)) => Some((id, point)),
                        _ => None,
                    }
                })
                .collect(),
        };

        let mut registry = self.point_indexes_write(entity);
        for (id, point) in backfill {
            registry.insert(label, property, id, point);
        }
    }

    pub(super) fn deactivate_point_scope(
        &self,
        entity: StoredIndexEntity,
        label: &str,
        property: &str,
    ) {
        self.point_indexes_write(entity)
            .remove_scope(label, property);
    }

    pub(super) fn vector_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexRead<'_, VectorIndexRegistry> {
        self.indexes.vector.read(entity)
    }

    pub(super) fn vector_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> IndexWrite<'_, VectorIndexRegistry> {
        self.indexes.vector.write(entity)
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_vector_index(
        &self,
        entity: StoredIndexEntity,
        name: &str,
        label: &str,
        property: &str,
        similarity: VectorSimilarity,
        provider: VectorIndexProvider,
        hnsw: HnswParams,
        lazy: bool,
    ) {
        {
            let mut registry = self.vector_indexes_write(entity);
            registry.register(
                name.to_string(),
                label.to_string(),
                property.to_string(),
                similarity,
                provider,
                hnsw,
            );
        }

        if lazy {
            // Skip the initial backfill — the catalog state is flipped
            // to Populating by the caller and the first query routed
            // to this index triggers `lazy_populate_vector_index`.
            // Mutations between CREATE and first query still feed the
            // registry via the maintenance hook, so the lazy phase
            // only handles vectors that existed before CREATE.
            return;
        }

        self.backfill_vector_index(entity, label, property);
    }

    /// Walk the property store for vectors matching this index's
    /// `(label, property)` scope and replay them into the registry.
    /// Shared by sync CREATE and lazy-populate flows.
    fn backfill_vector_index(&self, entity: StoredIndexEntity, label: &str, property: &str) {
        let backfill: Vec<(u64, crate::LoraVector)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| node.labels().strs().any(|l| l == label))
                .filter_map(|(id, node)| {
                    match node.properties().get(property).map(|v| v.to_owned()) {
                        Some(PropertyValue::Vector(v)) => Some((id, v)),
                        _ => None,
                    }
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type() == label)
                .filter_map(|(id, rel)| {
                    match rel.properties().get(property).map(|v| v.to_owned()) {
                        Some(PropertyValue::Vector(v)) => Some((id, v)),
                        _ => None,
                    }
                })
                .collect(),
        };

        let mut registry = self.vector_indexes_write(entity);
        for (id, vector) in backfill {
            registry.insert_for(label, property, id, &vector);
        }
    }

    /// Lazy-populate a `Populating` vector index: backfill from the
    /// property store, then flip catalog state to `Online`. Called
    /// from `GraphStorage::vector_search` on the first request that
    /// hits a still-populating index. Idempotent: a second concurrent
    /// caller finds the state already `Online` and does no work.
    pub(super) fn lazy_populate_vector_index(&self, name: &str) {
        let def = match self.index_catalog_read().get(name).cloned() {
            Some(d) => d,
            None => return,
        };
        if def.state != StoredIndexState::Populating || def.kind != StoredIndexKind::Vector {
            return;
        }
        let label = match def.label.as_deref() {
            Some(l) => l,
            None => return,
        };
        let property = match def.properties.first() {
            Some(p) => p.as_str(),
            None => return,
        };
        self.backfill_vector_index(def.entity, label, property);
        self.index_catalog_write()
            .set_state(name, StoredIndexState::Online);
    }

    pub(super) fn deactivate_vector_index(&self, entity: StoredIndexEntity, name: &str) {
        self.vector_indexes_write(entity).deregister(name);
    }

    /// Snapshot of cardinality stats. Cheap: derived from already-tracked
    /// `nodes_by_label` / `relationships_by_type` lengths and the
    /// distinct-value sketches (a cached estimate per (scope, key), and
    /// the previous map reused while those don't change). The cost model
    /// uses this to populate `estimated_rows` on plan-tree nodes.
    pub fn graph_stats(&self) -> GraphStats {
        let mut stats = GraphStats {
            node_count: self.live_node_count,
            relationship_count: self.live_rel_count,
            ..Default::default()
        };
        for (label, ids) in &self.nodes_by_label {
            stats.nodes_by_label.insert(label.clone(), ids.len());
        }
        for (rel_type, ids) in &self.relationships_by_type {
            stats
                .relationships_by_type
                .insert(rel_type.clone(), ids.len());
        }
        // Distinct values per (label / type, property): estimated from
        // the distinct-value sketches, which exist for every key and are
        // a function of the live data alone. Deliberately *not* the
        // exact bucket counts of active hash indexes: which keys are
        // active depends on lookup history and on whether the process
        // restarted, and plans must not.
        stats.node_distinct_values = self.distinct_stats.nodes.distinct_values();
        stats.relationship_distinct_values = self.distinct_stats.relationships.distinct_values();

        for def in self.index_catalog_read().list() {
            if def.state != StoredIndexState::Online {
                continue;
            }
            let Some(label) = def.label else {
                continue;
            };
            for property in def.properties {
                let scope = (label.clone(), property);
                match (def.entity, def.kind) {
                    (StoredIndexEntity::Node, StoredIndexKind::Range) => {
                        stats.node_range_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Node, StoredIndexKind::Text) => {
                        stats.node_text_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Node, StoredIndexKind::Point) => {
                        stats.node_point_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Relationship, StoredIndexKind::Range) => {
                        stats.relationship_range_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Relationship, StoredIndexKind::Text) => {
                        stats.relationship_text_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Relationship, StoredIndexKind::Point) => {
                        stats.relationship_point_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Node, StoredIndexKind::Vector) => {
                        stats.node_vector_indexes.insert(scope);
                    }
                    (StoredIndexEntity::Relationship, StoredIndexKind::Vector) => {
                        stats.relationship_vector_indexes.insert(scope);
                    }
                    (_, StoredIndexKind::Lookup | StoredIndexKind::Fulltext) => {}
                }
            }
        }
        stats
    }

    /// Approximate retained-heap breakdown of this graph. See
    /// [`super::MemoryReport`] for the methodology and per-component
    /// fields. Intended for benches and the `mem_probe*` examples;
    /// not on a hot path.
    pub fn memory_estimate(&self) -> super::MemoryReport {
        super::mem_report::estimate(self)
    }

    /// Count (or uncount) every property of an entity under each of its
    /// scopes in the distinct-value sketches.
    pub(super) fn count_distinct_values<'a>(
        stats: &mut super::distinct_stats::DistinctStats,
        scopes: impl IntoIterator<Item = &'a str>,
        properties: &Properties,
        add: bool,
    ) {
        if properties.is_empty() {
            return;
        }
        for scope in scopes {
            if add {
                stats.add_all(scope, properties);
            } else {
                for (key, value) in properties {
                    stats.remove(scope, key, value);
                }
            }
        }
    }

    pub(super) fn on_node_created(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.insert_node_label_index(node.id, label);
        }
        Self::count_distinct_values(
            &mut self.distinct_stats.nodes,
            node.labels.strs(),
            &node.properties,
            true,
        );
        self.index_node_properties_if_active(node.id, node.labels.strs(), &node.properties);
        self.maintain_node_secondary_indexes(node, SecondaryIndexMutation::Insert);
    }

    pub(super) fn on_node_property_set(
        &mut self,
        node_id: NodeId,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        let Some(labels) = self.node_at(node_id).map(|node| node.labels().to_owned()) else {
            return;
        };
        for label in &labels {
            self.distinct_stats.nodes.replace(label, key, old, new);
        }

        if self.node_property_index_is_active(key) {
            if let Some(old) = old {
                self.unindex_node_property_if_active(node_id, labels.strs(), key, old);
            }
            self.index_node_property_if_active(node_id, labels.strs(), key, new);
        }

        self.update_secondary_property(
            StoredIndexEntity::Node,
            labels.strs(),
            node_id,
            key,
            old,
            Some(new),
        );
    }

    pub(super) fn on_node_property_removed(
        &mut self,
        node_id: NodeId,
        key: &str,
        old: &PropertyValue,
    ) {
        let Some(labels) = self.node_at(node_id).map(|node| node.labels().to_owned()) else {
            return;
        };
        for label in &labels {
            self.distinct_stats.nodes.remove(label, key, old);
        }
        if self.node_property_index_is_active(key) {
            self.unindex_node_property_if_active(node_id, labels.strs(), key, old);
        }
        self.update_secondary_property(
            StoredIndexEntity::Node,
            labels.strs(),
            node_id,
            key,
            Some(old),
            None,
        );
    }

    pub(super) fn on_node_label_added(&mut self, node_id: NodeId, label: &str) {
        self.insert_node_label_index(node_id, label);

        let Some(properties) = self
            .node_at(node_id)
            .map(|node| node.properties().to_owned())
        else {
            return;
        };
        Self::count_distinct_values(&mut self.distinct_stats.nodes, [label], &properties, true);
        if self.active_node_property_index_count() != 0 {
            self.index_node_scope_properties_if_active(node_id, label, &properties);
            // Its first label: it was indexed as a node without any.
            if self.node_at(node_id).is_some_and(|n| n.labels().len() == 1) {
                self.unindex_node_scope_properties_if_active(node_id, UNLABELLED, &properties);
            }
        }
        for (key, value) in &properties {
            self.update_secondary_property(
                StoredIndexEntity::Node,
                [label],
                node_id,
                key,
                None,
                Some(value),
            );
        }
    }

    pub(super) fn on_node_label_removed(&mut self, node_id: NodeId, label: &str) {
        self.remove_node_label_index(node_id, label);

        let Some(properties) = self
            .node_at(node_id)
            .map(|node| node.properties().to_owned())
        else {
            return;
        };
        Self::count_distinct_values(&mut self.distinct_stats.nodes, [label], &properties, false);
        if self.active_node_property_index_count() != 0 {
            self.unindex_node_scope_properties_if_active(node_id, label, &properties);
            // Its last label: index it as a node without any.
            if self.node_at(node_id).is_some_and(|n| n.labels().is_empty()) {
                self.index_node_scope_properties_if_active(node_id, UNLABELLED, &properties);
            }
        }
        for (key, value) in &properties {
            self.update_secondary_property(
                StoredIndexEntity::Node,
                [label],
                node_id,
                key,
                Some(value),
                None,
            );
        }
    }

    pub(super) fn on_node_deleted(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.remove_node_label_index(node.id, label);
        }
        Self::count_distinct_values(
            &mut self.distinct_stats.nodes,
            node.labels.strs(),
            &node.properties,
            false,
        );
        self.unindex_active_node_properties(node.id, node.labels.strs(), &node.properties);
        self.maintain_node_secondary_indexes(node, SecondaryIndexMutation::Remove);
    }

    pub(super) fn on_relationship_created(&mut self, rel: &RelationshipRecord) {
        self.attach_relationship(rel);
        Self::count_distinct_values(
            &mut self.distinct_stats.relationships,
            [rel.rel_type.as_str()],
            &rel.properties,
            true,
        );
        self.index_relationship_properties_if_active(
            rel.id,
            [rel.rel_type.as_str()],
            &rel.properties,
        );
        self.maintain_relationship_secondary_indexes(rel, SecondaryIndexMutation::Insert);
    }

    pub(super) fn on_relationship_property_set(
        &mut self,
        rel_id: RelationshipId,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        let Some(rel_type) = self.rel_at(rel_id).map(|rel| rel.type_name().clone()) else {
            return;
        };
        self.distinct_stats
            .relationships
            .replace(&rel_type, key, old, new);

        if self.relationship_property_index_is_active(key) {
            if let Some(old) = old {
                self.unindex_relationship_property_if_active(rel_id, [rel_type.as_str()], key, old);
            }
            self.index_relationship_property_if_active(rel_id, [rel_type.as_str()], key, new);
        }

        self.update_secondary_property(
            StoredIndexEntity::Relationship,
            [rel_type.as_str()],
            rel_id,
            key,
            old,
            Some(new),
        );
    }

    pub(super) fn on_relationship_property_removed(
        &mut self,
        rel_id: RelationshipId,
        key: &str,
        old: &PropertyValue,
    ) {
        let Some(rel_type) = self.rel_at(rel_id).map(|rel| rel.type_name().clone()) else {
            return;
        };
        self.distinct_stats
            .relationships
            .remove(&rel_type, key, old);
        if self.relationship_property_index_is_active(key) {
            self.unindex_relationship_property_if_active(rel_id, [rel_type.as_str()], key, old);
        }
        self.update_secondary_property(
            StoredIndexEntity::Relationship,
            [rel_type.as_str()],
            rel_id,
            key,
            Some(old),
            None,
        );
    }

    pub(super) fn on_relationship_deleted(&mut self, rel: &RelationshipRecord) {
        self.detach_relationship_indexes(rel);
        Self::count_distinct_values(
            &mut self.distinct_stats.relationships,
            [rel.rel_type.as_str()],
            &rel.properties,
            false,
        );
        self.unindex_active_relationship_properties(
            rel.id,
            [rel.rel_type.as_str()],
            &rel.properties,
        );
        self.maintain_relationship_secondary_indexes(rel, SecondaryIndexMutation::Remove);
    }

    fn index_node_property_if_active<'a>(
        &mut self,
        node_id: NodeId,
        labels: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        if self.active_node_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        if indexes.node_properties.is_active(key) {
            indexes
                .node_properties
                .insert_with_scopes(node_id, labels, key, value);
        }
    }

    fn index_node_properties_if_active<'a>(
        &mut self,
        node_id: NodeId,
        labels: impl IntoIterator<Item = &'a str> + Clone,
        properties: &Properties,
    ) {
        if self.active_node_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        for (key, value) in properties {
            if indexes.node_properties.is_active(key) {
                indexes
                    .node_properties
                    .insert_with_scopes(node_id, labels.clone(), key, value);
            }
        }
    }

    fn unindex_node_property_if_active<'a>(
        &mut self,
        node_id: NodeId,
        labels: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        if self.active_node_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        if indexes.node_properties.is_active(key) {
            indexes
                .node_properties
                .remove_with_scopes(node_id, labels, key, value);
        }
    }

    fn index_node_scope_properties_if_active(
        &mut self,
        node_id: NodeId,
        scope: &str,
        properties: &Properties,
    ) {
        if self.active_node_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        for (key, value) in properties {
            if indexes.node_properties.is_active(key) {
                indexes
                    .node_properties
                    .insert_scoped(node_id, scope, key, value);
            }
        }
    }

    fn unindex_node_scope_properties_if_active(
        &mut self,
        node_id: NodeId,
        scope: &str,
        properties: &Properties,
    ) {
        if self.active_node_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        for (key, value) in properties {
            if indexes.node_properties.is_active(key) {
                indexes
                    .node_properties
                    .remove_scoped(node_id, scope, key, value);
            }
        }
    }

    fn unindex_active_node_properties<'a>(
        &mut self,
        node_id: NodeId,
        labels: impl IntoIterator<Item = &'a str> + Clone,
        properties: &Properties,
    ) {
        if self.active_node_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        for (key, value) in properties {
            if indexes.node_properties.is_active(key) {
                indexes
                    .node_properties
                    .remove_with_scopes(node_id, labels.clone(), key, value);
            }
        }
    }

    fn index_relationship_property_if_active<'a>(
        &mut self,
        rel_id: RelationshipId,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        if self.active_relationship_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        if indexes.relationship_properties.is_active(key) {
            indexes
                .relationship_properties
                .insert_with_scopes(rel_id, scopes, key, value);
        }
    }

    fn index_relationship_properties_if_active<'a>(
        &mut self,
        rel_id: RelationshipId,
        scopes: impl IntoIterator<Item = &'a str> + Clone,
        properties: &Properties,
    ) {
        if self.active_relationship_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        for (key, value) in properties {
            if indexes.relationship_properties.is_active(key) {
                indexes.relationship_properties.insert_with_scopes(
                    rel_id,
                    scopes.clone(),
                    key,
                    value,
                );
            }
        }
    }

    fn unindex_relationship_property_if_active<'a>(
        &mut self,
        rel_id: RelationshipId,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        if self.active_relationship_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        if indexes.relationship_properties.is_active(key) {
            indexes
                .relationship_properties
                .remove_with_scopes(rel_id, scopes, key, value);
        }
    }

    fn unindex_active_relationship_properties<'a>(
        &mut self,
        rel_id: RelationshipId,
        scopes: impl IntoIterator<Item = &'a str> + Clone,
        properties: &Properties,
    ) {
        if self.active_relationship_property_index_count() == 0 {
            return;
        }
        let indexes = self.indexes_mut();
        for (key, value) in properties {
            if indexes.relationship_properties.is_active(key) {
                indexes.relationship_properties.remove_with_scopes(
                    rel_id,
                    scopes.clone(),
                    key,
                    value,
                );
            }
        }
    }

    pub(super) fn scan_nodes_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeRecord> {
        match label {
            Some(label) => self
                .nodes_by_label
                .get(label)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| self.node_at(id))
                .filter(|node| node.properties().get(key).is_some_and(|v| v == *value))
                .map(|node| node.to_record())
                .collect(),
            None => self
                .iter_node_refs()
                .filter(|node| node.properties().get(key).is_some_and(|v| v == *value))
                .map(|node| node.to_record())
                .collect(),
        }
    }

    pub(super) fn scan_node_ids_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeId> {
        match label {
            Some(label) => self
                .nodes_by_label
                .get(label)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| {
                    (self
                        .node_at(id)?
                        .properties()
                        .get(key)
                        .is_some_and(|v| v == *value))
                    .then_some(id)
                })
                .collect(),
            None => self
                .iter_nodes()
                .filter_map(|(id, node)| {
                    (node.properties().get(key).is_some_and(|v| v == *value)).then_some(id)
                })
                .collect(),
        }
    }

    pub(super) fn any_node_by_property(
        &self,
        label: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool {
        self.nodes_by_label
            .get(label)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|&id| self.node_at(id))
            .any(|node| node.properties().get(key).is_some_and(|v| v == *value))
    }

    pub(super) fn scan_relationships_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipRecord> {
        match rel_type {
            Some(rel_type) => self
                .relationships_by_type
                .get(rel_type)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| self.rel_at(id))
                .filter(|rel| rel.properties().get(key).is_some_and(|v| v == *value))
                .map(|rel| rel.to_record())
                .collect(),
            None => self
                .iter_rel_refs()
                .filter(|rel| rel.properties().get(key).is_some_and(|v| v == *value))
                .map(|rel| rel.to_record())
                .collect(),
        }
    }

    pub(super) fn scan_relationship_ids_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipId> {
        match rel_type {
            Some(rel_type) => self
                .relationships_by_type
                .get(rel_type)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| {
                    (self
                        .rel_at(id)?
                        .properties()
                        .get(key)
                        .is_some_and(|v| v == *value))
                    .then_some(id)
                })
                .collect(),
            None => self
                .iter_rels()
                .filter_map(|(id, rel)| {
                    (rel.properties().get(key).is_some_and(|v| v == *value)).then_some(id)
                })
                .collect(),
        }
    }

    pub(super) fn any_relationship_by_property(
        &self,
        rel_type: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool {
        self.relationships_by_type
            .get(rel_type)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|&id| self.rel_at(id))
            .any(|rel| rel.properties().get(key).is_some_and(|v| v == *value))
    }

    pub(super) fn attach_relationship(&mut self, rel: &RelationshipRecord) {
        let type_id = self.dicts.types.id_or_insert(&rel.rel_type);
        self.adjacency_push(
            rel.src,
            true,
            AdjEntry {
                type_id,
                neighbour: rel.dst,
                rel: rel.id,
            },
        );
        self.adjacency_push(
            rel.dst,
            false,
            AdjEntry {
                type_id,
                neighbour: rel.src,
                rel: rel.id,
            },
        );
        self.insert_relationship_type_index(rel.id, &rel.rel_type);
    }

    fn detach_relationship_indexes(&mut self, rel: &RelationshipRecord) {
        self.adjacency_remove(rel.src, true, rel.id);
        self.adjacency_remove(rel.dst, false, rel.id);

        self.remove_relationship_type_index(rel.id, &rel.rel_type);
    }

    pub(super) fn relationship_ids_for_direction(
        &self,
        node_id: NodeId,
        direction: Direction,
    ) -> Vec<RelationshipId> {
        let mut ids = Vec::new();
        let _ = self.try_for_each_adjacent_id_unchecked(node_id, direction, &[], |rel_id, _| {
            ids.push(rel_id);
            Ok::<(), ()>(())
        });
        ids
    }

    pub(super) fn has_incident_relationships(&self, node_id: NodeId) -> bool {
        self.outgoing_at(node_id).is_some_and(|adj| !adj.is_empty())
            || self.incoming_at(node_id).is_some_and(|adj| !adj.is_empty())
    }

    pub(super) fn incident_relationship_ids(&self, node_id: NodeId) -> Vec<RelationshipId> {
        self.relationship_ids_for_direction(node_id, Direction::Undirected)
    }

    /// Replay a node creation using the id captured in a durable mutation
    /// event. This intentionally does not emit a new mutation event: callers
    /// must invoke it before installing a recorder on the graph.
    #[doc(hidden)]
    pub fn replay_create_node(
        &mut self,
        id: NodeId,
        labels: Vec<String>,
        properties: Properties,
    ) -> Result<NodeRecord, String> {
        if self.recorder.is_some() {
            return Err(
                "cannot replay node creation while a mutation recorder is installed".into(),
            );
        }
        if self.node_at(id).is_some() {
            return Err(format!("node id {id} already exists"));
        }
        let idx = self.ensure_node_slot_checked(id)?;
        self.bump_next_node_id_past(id)?;

        let labels = Self::normalize_labels(labels);
        let node = NodeRecord {
            id,
            labels,
            properties,
        };

        self.put_node_at_slot(idx, &node);
        // Same index maintenance as a live create: only hash indexes that
        // are already active (declared by a replayed CREATE INDEX /
        // CREATE CONSTRAINT, which backfills from the data replayed so
        // far) are kept current. Lookup-activated (implicit) indexes are
        // rebuilt lazily on first use, exactly as in a fresh process.
        self.on_node_created(&node);

        Ok(node)
    }

    /// Replay a relationship creation using the id captured in a durable
    /// mutation event. This intentionally does not emit a new mutation event:
    /// callers must invoke it before installing a recorder on the graph.
    #[doc(hidden)]
    pub fn replay_create_relationship(
        &mut self,
        id: RelationshipId,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        properties: Properties,
    ) -> Result<RelationshipRecord, String> {
        if self.recorder.is_some() {
            return Err(
                "cannot replay relationship creation while a mutation recorder is installed".into(),
            );
        }
        if self.rel_at(id).is_some() {
            return Err(format!("relationship id {id} already exists"));
        }
        if self.node_at(src).is_none() {
            return Err(format!(
                "relationship {id} references missing source node {src}"
            ));
        }
        if self.node_at(dst).is_none() {
            return Err(format!(
                "relationship {id} references missing target node {dst}"
            ));
        }

        let trimmed = rel_type.trim();
        if trimmed.is_empty() {
            return Err(format!("relationship {id} has an empty type"));
        }
        let idx = self.ensure_rel_slot_checked(id)?;
        self.bump_next_rel_id_past(id)?;

        let rel = RelationshipRecord {
            id,
            src,
            dst,
            rel_type: trimmed.into(),
            properties,
        };

        self.put_rel_at_slot(idx, &rel);
        // See `replay_create_node`: active (declared) indexes only.
        self.on_relationship_created(&rel);

        Ok(rel)
    }

    #[cfg(test)]
    pub(super) fn assert_property_indexes_match_scan(&self) {
        let indexes = self.indexes_read();
        assert_eq!(
            indexes.node_properties.active_keys.len(),
            self.active_node_property_index_count(),
            "node property index counter diverged from active key set"
        );
        assert_eq!(
            indexes.relationship_properties.active_keys.len(),
            self.active_relationship_property_index_count(),
            "relationship property index counter diverged from active key set"
        );

        let mut expected_nodes = PropertyIndexState {
            active_keys: indexes.node_properties.active_keys.clone(),
            any_scope_keys: indexes.node_properties.any_scope_keys.clone(),
            ..PropertyIndexState::default()
        };
        for (id, node) in self.iter_nodes() {
            for (key, value) in node.properties() {
                if expected_nodes.is_active(key) {
                    expected_nodes.insert_with_scopes(
                        id,
                        node.labels().strs(),
                        key,
                        &value.to_owned(),
                    );
                }
            }
        }
        assert_eq!(
            indexes.node_properties.scoped_values, expected_nodes.scoped_values,
            "node property scoped index values diverged from scan"
        );
        assert_eq!(
            indexes.node_properties.any_scope, expected_nodes.any_scope,
            "node property across-scopes index values diverged from scan"
        );

        let mut expected_relationships = PropertyIndexState {
            active_keys: indexes.relationship_properties.active_keys.clone(),
            any_scope_keys: indexes.relationship_properties.any_scope_keys.clone(),
            ..PropertyIndexState::default()
        };
        for (id, rel) in self.iter_rels() {
            for (key, value) in rel.properties() {
                if expected_relationships.is_active(key) {
                    expected_relationships.insert_with_scopes(
                        id,
                        [rel.rel_type()],
                        key,
                        &value.to_owned(),
                    );
                }
            }
        }
        assert_eq!(
            indexes.relationship_properties.scoped_values, expected_relationships.scoped_values,
            "relationship property scoped index values diverged from scan"
        );
        assert_eq!(
            indexes.relationship_properties.any_scope, expected_relationships.any_scope,
            "relationship property across-scopes index values diverged from scan"
        );
    }
}
