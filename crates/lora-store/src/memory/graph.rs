//! The [`InMemoryGraph`] data structure: slot-indexed node/relationship
//! storage, adjacency lists, label/type indexes, and the inherent
//! helpers that the trait impls in `super::impls` delegate to.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use lora_ast::Direction;

use crate::{
    LoraPoint, MutationEvent, MutationRecorder, NodeId, NodeRecord, Properties, PropertyValue,
    RelationshipId, RelationshipRecord,
};

use super::constraint_catalog::{
    ConstraintCatalog, ConstraintRequest, CreateConstraintError, CreateConstraintOutcome,
    DropConstraintError, DropConstraintOutcome,
};
use super::entity_index_store::IndexBundle;
use super::fulltext_index::FulltextRegistry;
use super::index_catalog::IndexConfigValue;
use super::index_catalog::{
    CreateIndexError, CreateIndexOutcome, DropIndexError, DropIndexOutcome, IndexCatalog,
    IndexDefinition, IndexRequest, StoredIndexEntity, StoredIndexKind, StoredIndexState,
};
use super::point_index::PointRegistry;
#[cfg(test)]
use super::property_index::PropertyIndexState;
use super::property_index::{PropertyIndexKey, PropertyIndexRegistry};
use super::secondary_index_maintenance::SecondaryIndexMutation;
use super::sorted_property_index::SortedPropertyIndex;
use super::stats::GraphStats;
use super::text_index::TrigramRegistry;

#[derive(Default)]
pub struct InMemoryGraph {
    pub(super) next_node_id: NodeId,
    pub(super) next_rel_id: RelationshipId,

    /// Slot-indexed node storage: `nodes[id as usize]` is the record at `id`.
    /// `None` slots are tombstones from deletes (we don't compact). Because
    /// `next_node_id` is monotonic the slot at `id` is initialized exactly
    /// when `id < next_node_id` — same identity guarantee the previous
    /// `BTreeMap<NodeId, NodeRecord>` had, just with O(1) lookup and
    /// cache-coherent layout.
    ///
    /// Records are wrapped in `Arc` so [`Self::clone`] (called on every
    /// auto-commit write to build a working copy) is `O(N)` atomic
    /// refcount bumps instead of `O(N)` deep record clones — for a
    /// 100k-node graph the difference is microseconds vs. tens of
    /// milliseconds. Mutating a record uses `Arc::make_mut`, which
    /// clones in place when the refcount is 1 (no concurrent reader)
    /// and falls back to a single-record clone-on-write when readers
    /// still hold a snapshot.
    pub(super) nodes: Vec<Option<Arc<NodeRecord>>>,
    pub(super) relationships: Vec<Option<Arc<RelationshipRecord>>>,
    /// Live (non-tombstoned) counts kept in sync with `put_*`/`take_*` so
    /// `node_count` / `relationship_count` stay O(1) — without a counter
    /// they'd have to scan the slab.
    pub(super) live_node_count: usize,
    pub(super) live_rel_count: usize,

    /// Adjacency keyed by NodeId. `outgoing[id]` is the list of relationship
    /// ids that leave `id`; mirrored on `incoming[id]`. Inner `Vec` instead
    /// of `BTreeSet` because edges are inserted exactly once and traversal
    /// only needs sequential iteration; the cache-friendly contiguous layout
    /// shows up on every traversal hop.
    pub(super) outgoing: Vec<Vec<RelationshipId>>,
    pub(super) incoming: Vec<Vec<RelationshipId>>,

    // secondary indexes
    /// Label -> the (unique, monotonic) node ids that carry it. The inner
    /// `Vec` instead of `BTreeSet` because every node id is inserted at most
    /// once per label (no dedup needed) and every consumer iterates the
    /// whole list anyway — contiguous storage iterates faster than a
    /// tree-of-pointers, and removes via `swap_remove` stay O(degree-of-label).
    pub(super) nodes_by_label: BTreeMap<String, Vec<NodeId>>,
    pub(super) relationships_by_type: BTreeMap<String, Vec<RelationshipId>>,

    /// All index machinery — the declared-index catalog, hash-bucket
    /// property registry, and the per-entity-kind secondary index
    /// registries (text, sorted, point, fulltext) plus their active
    /// counters — collapsed into one bundle. See [`IndexBundle`] for
    /// the rationale. The bundle is a packaging-only abstraction:
    /// every field accessed through `self.indexes.<x>` lives at the
    /// same address it would have as a top-level field.
    pub(super) indexes: IndexBundle,

    /// Catalog of explicitly-created constraints (CREATE CONSTRAINT).
    /// Deliberately not part of [`IndexBundle`] — constraints describe
    /// data invariants, not indexed access. The fact that uniqueness /
    /// key constraints back range indexes is handled in the
    /// constraint code path, not by the bundle's layout.
    pub(super) constraint_catalog: RwLock<ConstraintCatalog>,
    /// Fast-path counter for mutation-time constraint checks. Most
    /// workloads have no constraints installed; this lets the executor
    /// skip taking the catalog lock in that case.
    pub(super) active_constraints: AtomicUsize,

    /// Optional mutation observer. When `Some`, every committed mutation
    /// fans out to this recorder *after* the in-memory state has been
    /// updated. The recorder is not part of the graph's identity, so Clone
    /// and snapshot restore both reset it to `None`.
    pub(super) recorder: Option<Arc<dyn MutationRecorder>>,
}

impl std::fmt::Debug for InMemoryGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryGraph")
            .field("next_node_id", &self.next_node_id)
            .field("next_rel_id", &self.next_rel_id)
            .field("nodes", &self.nodes)
            .field("relationships", &self.relationships)
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
            nodes_by_label: self.nodes_by_label.clone(),
            relationships_by_type: self.relationships_by_type.clone(),
            // IndexBundle::clone deep-copies every owned registry under
            // its locks, mirroring what the old per-field clones did.
            // The hash-bucket registry skip-on-empty optimisation is
            // preserved: `PropertyIndexRegistry::clone` itself is cheap
            // when no entries exist.
            indexes: self.indexes.clone(),
            constraint_catalog: RwLock::new(self.constraint_catalog_read().clone()),
            active_constraints: AtomicUsize::new(self.active_constraint_count()),
            recorder: None,
        }
    }
}

impl InMemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity_hint(nodes: usize, relationships: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(nodes),
            relationships: Vec::with_capacity(relationships),
            outgoing: Vec::with_capacity(nodes),
            incoming: Vec::with_capacity(nodes),
            ..Self::default()
        }
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

    pub(super) fn reserve_next_node_slot(&mut self) -> (NodeId, usize) {
        let id = self.next_node_id;
        let idx = self
            .ensure_node_slot_checked(id)
            .expect("next node id should fit in memory-backed slab");
        self.bump_next_node_id_past(id)
            .expect("next node id should leave a valid successor");
        (id, idx)
    }

    pub(super) fn try_reserve_next_rel_slot(&mut self) -> Option<(RelationshipId, usize)> {
        let id = self.next_rel_id;
        let idx = self.ensure_rel_slot_checked(id).ok()?;
        self.bump_next_rel_id_past(id).ok()?;
        Some((id, idx))
    }

    // ---------- Slab access helpers ----------
    //
    // Stand-in for the BTreeMap API the previous storage used. They keep the
    // call sites readable while the underlying layout is positional Vec.

    #[inline]
    pub(super) fn node_at(&self, id: NodeId) -> Option<&NodeRecord> {
        self.nodes
            .get(Self::slot_index(id)?)
            .and_then(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    /// Mutable handle to a node record, doing copy-on-write only when the
    /// `Arc` is shared with a concurrent reader. With no readers (the
    /// common case after a fresh write_store clone), `Arc::make_mut`
    /// upgrades in place — no record clone.
    #[inline]
    pub(super) fn node_at_mut(&mut self, id: NodeId) -> Option<&mut NodeRecord> {
        self.nodes
            .get_mut(Self::slot_index(id)?)
            .and_then(|s| s.as_mut())
            .map(Arc::make_mut)
    }

    #[inline]
    pub(super) fn rel_at(&self, id: RelationshipId) -> Option<&RelationshipRecord> {
        self.relationships
            .get(Self::slot_index(id)?)
            .and_then(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    #[inline]
    pub(super) fn rel_at_mut(&mut self, id: RelationshipId) -> Option<&mut RelationshipRecord> {
        self.relationships
            .get_mut(Self::slot_index(id)?)
            .and_then(|s| s.as_mut())
            .map(Arc::make_mut)
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
            self.outgoing.resize_with(target, Vec::new);
            self.incoming.resize_with(target, Vec::new);
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

    fn ensure_node_slot(&mut self, id: NodeId) -> usize {
        self.ensure_node_slot_checked(id)
            .expect("node id should fit in memory-backed slab")
    }

    pub(super) fn put_node_checked(&mut self, id: NodeId, node: NodeRecord) -> Result<(), String> {
        let idx = self.ensure_node_slot_checked(id)?;
        self.put_node_at_slot(idx, node);
        Ok(())
    }

    pub(super) fn put_rel_checked(
        &mut self,
        id: RelationshipId,
        rel: RelationshipRecord,
    ) -> Result<(), String> {
        let idx = self.ensure_rel_slot_checked(id)?;
        self.put_rel_at_slot(idx, rel);
        Ok(())
    }

    pub(super) fn put_node_at_slot(&mut self, idx: usize, node: NodeRecord) {
        let was_present = self.nodes[idx].is_some();
        self.nodes[idx] = Some(Arc::new(node));
        if !was_present {
            self.live_node_count += 1;
        }
    }

    pub(super) fn put_rel_at_slot(&mut self, idx: usize, rel: RelationshipRecord) {
        let was_present = self.relationships[idx].is_some();
        self.relationships[idx] = Some(Arc::new(rel));
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
        // Unwrap the Arc — `try_unwrap` returns the inner `NodeRecord`
        // without cloning when our slab held the only reference, falling
        // back to a clone only when concurrent readers still hold a
        // snapshot Arc.
        removed.map(|arc| Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
    }

    pub(super) fn take_rel(&mut self, id: RelationshipId) -> Option<RelationshipRecord> {
        let idx = Self::slot_index(id)?;
        let removed = self.relationships.get_mut(idx).and_then(|s| s.take());
        if removed.is_some() {
            self.live_rel_count -= 1;
        }
        removed.map(|arc| Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
    }

    #[inline]
    pub(super) fn outgoing_at(&self, id: NodeId) -> Option<&[RelationshipId]> {
        self.outgoing.get(Self::slot_index(id)?).map(Vec::as_slice)
    }

    #[inline]
    pub(super) fn incoming_at(&self, id: NodeId) -> Option<&[RelationshipId]> {
        self.incoming.get(Self::slot_index(id)?).map(Vec::as_slice)
    }

    #[inline]
    fn try_for_each_adjacent_slice<F, E>(
        &self,
        node_id: NodeId,
        types: &[String],
        adj: &[RelationshipId],
        skip_self_loops: bool,
        visit: &mut F,
    ) -> Result<(), E>
    where
        F: FnMut(RelationshipId, NodeId) -> Result<(), E>,
    {
        let single_type = match types {
            [single] => Some(single.as_str()),
            _ => None,
        };
        let has_type_filter = !types.is_empty();

        for &rel_id in adj {
            let Some(rel) = self.rel_at(rel_id) else {
                continue;
            };
            if skip_self_loops && rel.src == node_id && rel.dst == node_id {
                continue;
            }
            if let Some(single) = single_type {
                if rel.rel_type != single {
                    continue;
                }
            } else if has_type_filter && !types.iter().any(|t| t == &rel.rel_type) {
                continue;
            }
            let Some(other_id) = Self::other_endpoint(rel, node_id) else {
                continue;
            };
            visit(rel_id, other_id)?;
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
        match direction {
            Direction::Right => {
                if let Some(adj) = self.outgoing_at(node_id) {
                    self.try_for_each_adjacent_slice(node_id, types, adj, false, &mut visit)?;
                }
            }
            Direction::Left => {
                if let Some(adj) = self.incoming_at(node_id) {
                    self.try_for_each_adjacent_slice(node_id, types, adj, false, &mut visit)?;
                }
            }
            Direction::Undirected => {
                if let Some(adj) = self.outgoing_at(node_id) {
                    self.try_for_each_adjacent_slice(node_id, types, adj, false, &mut visit)?;
                }
                if let Some(adj) = self.incoming_at(node_id) {
                    self.try_for_each_adjacent_slice(node_id, types, adj, true, &mut visit)?;
                }
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

    pub(super) fn iter_node_records(&self) -> impl Iterator<Item = &NodeRecord> + '_ {
        self.nodes
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    pub(super) fn iter_rel_ids(&self) -> impl Iterator<Item = RelationshipId> + '_ {
        self.relationships
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| i as RelationshipId))
    }

    pub(super) fn iter_rel_records(&self) -> impl Iterator<Item = &RelationshipRecord> + '_ {
        self.relationships
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    pub(super) fn iter_nodes(&self) -> impl Iterator<Item = (NodeId, &NodeRecord)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|n| (i as NodeId, n.as_ref())))
    }

    pub(super) fn iter_rels(
        &self,
    ) -> impl Iterator<Item = (RelationshipId, &RelationshipRecord)> + '_ {
        self.relationships
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|r| (i as RelationshipId, r.as_ref())))
    }

    /// Add `rel_id` to `node_id`'s outgoing list. Relies on the monotonic-id
    /// invariant: relationship ids are allocated once and never re-used, so
    /// the bucket can never see a duplicate.
    fn outgoing_push(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        let idx = self.ensure_node_slot(node_id);
        self.outgoing[idx].push(rel_id);
    }

    fn incoming_push(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        let idx = self.ensure_node_slot(node_id);
        self.incoming[idx].push(rel_id);
    }

    /// Remove `rel_id` from `node_id`'s outgoing list. `swap_remove` keeps
    /// the operation O(1) — adjacency order doesn't carry semantic meaning.
    fn outgoing_remove(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        if let Some(v) = Self::slot_index(node_id).and_then(|idx| self.outgoing.get_mut(idx)) {
            if let Some(pos) = v.iter().position(|&id| id == rel_id) {
                v.swap_remove(pos);
            }
        }
    }

    fn incoming_remove(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        if let Some(v) = Self::slot_index(node_id).and_then(|idx| self.incoming.get_mut(idx)) {
            if let Some(pos) = v.iter().position(|&id| id == rel_id) {
                v.swap_remove(pos);
            }
        }
    }

    pub(super) fn normalize_labels(labels: Vec<String>) -> Vec<String> {
        let mut seen = BTreeSet::new();

        labels
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .filter(|s| seen.insert(s.clone()))
            .collect()
    }

    pub(super) fn insert_node_label_index(&mut self, node_id: NodeId, label: &str) {
        // Hot path: skip the `String` alloc when the label bucket already
        // exists. The monotonic-id invariant on the create path guarantees
        // `node_id` is unique, so we push unconditionally; the previous
        // `contains` guard turned bulk CREATE into O(n²).
        if let Some(bucket) = self.nodes_by_label.get_mut(label) {
            bucket.push(node_id);
        } else {
            self.nodes_by_label.insert(label.to_string(), vec![node_id]);
        }
    }

    fn remove_node_label_index(&mut self, node_id: NodeId, label: &str) {
        if let Some(ids) = self.nodes_by_label.get_mut(label) {
            if let Some(pos) = ids.iter().position(|&id| id == node_id) {
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
                .insert(rel_type.to_string(), vec![rel_id]);
        }
    }

    fn remove_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        if let Some(ids) = self.relationships_by_type.get_mut(rel_type) {
            if let Some(pos) = ids.iter().position(|&id| id == rel_id) {
                ids.swap_remove(pos);
            }
            if ids.is_empty() {
                self.relationships_by_type.remove(rel_type);
            }
        }
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

        for (id, node) in self.iter_nodes() {
            if let Some(value) = node.properties.get(key) {
                indexes.node_properties.insert_with_scopes(
                    id,
                    node.labels.iter().map(String::as_str),
                    key,
                    value,
                );
            }
        }
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

        for (id, rel) in self.iter_rels() {
            if let Some(value) = rel.properties.get(key) {
                indexes.relationship_properties.insert_with_scopes(
                    id,
                    [rel.rel_type.as_str()],
                    key,
                    value,
                );
            }
        }
        if indexes.relationship_properties.activate(key) {
            self.indexes
                .active_relationship_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn index_catalog_read(&self) -> std::sync::RwLockReadGuard<'_, IndexCatalog> {
        self.indexes
            .catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn index_catalog_write(&self) -> RwLockWriteGuard<'_, IndexCatalog> {
        self.indexes
            .catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn constraint_catalog_read(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, ConstraintCatalog> {
        self.constraint_catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn constraint_catalog_write(&self) -> RwLockWriteGuard<'_, ConstraintCatalog> {
        self.constraint_catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
            // Matches Neo4j's "creating constraints when there exists
            // conflicting data will fail" behaviour (22N77/79/80).
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
                StoredIndexKind::Lookup | StoredIndexKind::Vector => {
                    // VECTOR uses a flat per-query scan today; no
                    // backing structure to release.
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
                let cell_size = point_cell_size_from_options(&def.options);
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
            // LOOKUP rides on the label/type indexes maintained eagerly.
            // VECTOR runs flat scans per query — no precomputed structure
            // to populate here.
            StoredIndexKind::Lookup | StoredIndexKind::Vector => {}
        }
    }

    pub(super) fn text_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> std::sync::RwLockReadGuard<'_, TrigramRegistry> {
        self.indexes.text.read(entity)
    }

    pub(super) fn text_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> RwLockWriteGuard<'_, TrigramRegistry> {
        self.indexes.text.write(entity)
    }

    pub(super) fn fulltext_indexes_read(
        &self,
        entity: StoredIndexEntity,
    ) -> std::sync::RwLockReadGuard<'_, FulltextRegistry> {
        self.indexes.fulltext.read(entity)
    }

    #[allow(dead_code)]
    pub(super) fn fulltext_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> RwLockWriteGuard<'_, FulltextRegistry> {
        self.indexes.fulltext.write(entity)
    }

    fn activate_text_scope(&self, entity: StoredIndexEntity, label: &str, property: &str) {
        if !self.text_indexes_write(entity).add_scope(label, property) {
            return;
        }

        let backfill: Vec<(u64, String)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| node.labels.iter().any(|l| l == label))
                .filter_map(|(id, node)| match node.properties.get(property) {
                    Some(PropertyValue::String(value)) => Some((id, value.clone())),
                    _ => None,
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type == label)
                .filter_map(|(id, rel)| match rel.properties.get(property) {
                    Some(PropertyValue::String(value)) => Some((id, value.clone())),
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
                        .any(|wanted| node.labels.iter().any(|l| l == wanted))
                })
                .map(|(id, node)| {
                    let counts = term_counts_for_properties(&node.properties, properties);
                    (id, counts)
                })
                .filter(|(_, c)| !c.is_empty())
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| labels.iter().any(|wanted| wanted == &rel.rel_type))
                .map(|(id, rel)| {
                    let counts = term_counts_for_properties(&rel.properties, properties);
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
    ) -> std::sync::RwLockReadGuard<'_, SortedPropertyIndex> {
        self.indexes.sorted.read(entity)
    }

    pub(super) fn sorted_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> RwLockWriteGuard<'_, SortedPropertyIndex> {
        self.indexes.sorted.write(entity)
    }

    fn activate_sorted_scope(&self, entity: StoredIndexEntity, label: &str, property: &str) {
        if !self.sorted_indexes_write(entity).add_scope(label, property) {
            return;
        }

        let backfill: Vec<(u64, PropertyValue)> = match entity {
            StoredIndexEntity::Node => self
                .iter_nodes()
                .filter(|(_, node)| node.labels.iter().any(|l| l == label))
                .filter_map(|(id, node)| {
                    node.properties
                        .get(property)
                        .map(|value| (id, value.clone()))
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type == label)
                .filter_map(|(id, rel)| {
                    rel.properties
                        .get(property)
                        .map(|value| (id, value.clone()))
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
    ) -> std::sync::RwLockReadGuard<'_, PointRegistry> {
        self.indexes.point.read(entity)
    }

    pub(super) fn point_indexes_write(
        &self,
        entity: StoredIndexEntity,
    ) -> RwLockWriteGuard<'_, PointRegistry> {
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
                .filter(|(_, node)| node.labels.iter().any(|l| l == label))
                .filter_map(|(id, node)| match node.properties.get(property) {
                    Some(PropertyValue::Point(point)) => Some((id, point.clone())),
                    _ => None,
                })
                .collect(),
            StoredIndexEntity::Relationship => self
                .iter_rels()
                .filter(|(_, rel)| rel.rel_type == label)
                .filter_map(|(id, rel)| match rel.properties.get(property) {
                    Some(PropertyValue::Point(point)) => Some((id, point.clone())),
                    _ => None,
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

    /// Snapshot of cardinality stats. Cheap: derived from already-tracked
    /// `nodes_by_label` / `relationships_by_type` lengths and the active
    /// property-index buckets. The cost model uses this to populate
    /// `estimated_rows` on plan-tree nodes.
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
        // Distinct values per (label, property): pulled from the
        // property-index scoped buckets, where we already track the
        // per-scope value distribution. Empty for properties without
        // an active hash-index — the cost model falls back to a
        // conservative estimate in that case.
        let prop_indexes = self.indexes_read();
        for (scope, props) in &prop_indexes.node_properties.scoped_values {
            for (key, values) in props {
                stats
                    .node_distinct_values
                    .insert((scope.clone(), key.clone()), values.len());
            }
        }
        for (scope, props) in &prop_indexes.relationship_properties.scoped_values {
            for (key, values) in props {
                stats
                    .relationship_distinct_values
                    .insert((scope.clone(), key.clone()), values.len());
            }
        }

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

    pub(super) fn rebuild_property_indexes(&mut self) {
        let mut indexes = PropertyIndexRegistry::default();

        for (id, node) in self.iter_nodes() {
            for (key, value) in &node.properties {
                if PropertyIndexKey::from_value(value).is_some() {
                    indexes.node_properties.activate(key);
                    indexes.node_properties.insert_with_scopes(
                        id,
                        node.labels.iter().map(String::as_str),
                        key,
                        value,
                    );
                }
            }
        }

        for (id, rel) in self.iter_rels() {
            for (key, value) in &rel.properties {
                if PropertyIndexKey::from_value(value).is_some() {
                    indexes.relationship_properties.activate(key);
                    indexes.relationship_properties.insert_with_scopes(
                        id,
                        [rel.rel_type.as_str()],
                        key,
                        value,
                    );
                }
            }
        }

        let node_index_count = indexes.node_properties.active_keys.len();
        let relationship_index_count = indexes.relationship_properties.active_keys.len();
        *self.indexes_mut() = indexes;
        self.indexes
            .active_node_property_indexes
            .store(node_index_count, Ordering::Relaxed);
        self.indexes
            .active_relationship_property_indexes
            .store(relationship_index_count, Ordering::Relaxed);
    }

    pub(super) fn on_node_created(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.insert_node_label_index(node.id, label);
        }
        self.index_node_properties_if_active(
            node.id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
        self.maintain_node_secondary_indexes(node, SecondaryIndexMutation::Insert);
    }

    pub(super) fn on_node_replayed(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.insert_node_label_index(node.id, label);
        }
        self.index_node_properties_eager(
            node.id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
        self.maintain_node_secondary_indexes(node, SecondaryIndexMutation::Insert);
    }

    pub(super) fn on_node_property_set(
        &mut self,
        node_id: NodeId,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        let Some(labels) = self.node_at(node_id).map(|node| node.labels.clone()) else {
            return;
        };

        if self.node_property_index_is_active(key) {
            if let Some(old) = old {
                self.unindex_node_property_if_active(
                    node_id,
                    labels.iter().map(String::as_str),
                    key,
                    old,
                );
            }
            self.index_node_property_if_active(
                node_id,
                labels.iter().map(String::as_str),
                key,
                new,
            );
        }

        self.update_secondary_property(
            StoredIndexEntity::Node,
            labels.iter().map(String::as_str),
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
        let Some(labels) = self.node_at(node_id).map(|node| node.labels.clone()) else {
            return;
        };
        if self.node_property_index_is_active(key) {
            self.unindex_node_property_if_active(
                node_id,
                labels.iter().map(String::as_str),
                key,
                old,
            );
        }
        self.update_secondary_property(
            StoredIndexEntity::Node,
            labels.iter().map(String::as_str),
            node_id,
            key,
            Some(old),
            None,
        );
    }

    pub(super) fn on_node_label_added(&mut self, node_id: NodeId, label: &str) {
        self.insert_node_label_index(node_id, label);

        let Some(properties) = self.node_at(node_id).map(|node| node.properties.clone()) else {
            return;
        };
        if self.active_node_property_index_count() != 0 {
            self.index_node_scope_properties_if_active(node_id, label, &properties);
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

        let Some(properties) = self.node_at(node_id).map(|node| node.properties.clone()) else {
            return;
        };
        if self.active_node_property_index_count() != 0 {
            self.unindex_node_scope_properties_if_active(node_id, label, &properties);
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
        self.unindex_active_node_properties(
            node.id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
        self.maintain_node_secondary_indexes(node, SecondaryIndexMutation::Remove);
    }

    pub(super) fn on_relationship_created(&mut self, rel: &RelationshipRecord) {
        self.attach_relationship(rel);
        self.index_relationship_properties_if_active(
            rel.id,
            [rel.rel_type.as_str()],
            &rel.properties,
        );
        self.maintain_relationship_secondary_indexes(rel, SecondaryIndexMutation::Insert);
    }

    pub(super) fn on_relationship_replayed(&mut self, rel: &RelationshipRecord) {
        self.attach_relationship(rel);
        self.index_relationship_properties_eager(rel.id, [rel.rel_type.as_str()], &rel.properties);
        self.maintain_relationship_secondary_indexes(rel, SecondaryIndexMutation::Insert);
    }

    pub(super) fn on_relationship_property_set(
        &mut self,
        rel_id: RelationshipId,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        let Some(rel_type) = self.rel_at(rel_id).map(|rel| rel.rel_type.clone()) else {
            return;
        };

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
        let Some(rel_type) = self.rel_at(rel_id).map(|rel| rel.rel_type.clone()) else {
            return;
        };
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
        self.unindex_active_relationship_properties(
            rel.id,
            [rel.rel_type.as_str()],
            &rel.properties,
        );
        self.maintain_relationship_secondary_indexes(rel, SecondaryIndexMutation::Remove);
    }

    fn index_node_property_eager<'a>(
        &mut self,
        node_id: NodeId,
        labels: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        if PropertyIndexKey::from_value(value).is_none() {
            return;
        }

        let activated = {
            let indexes = self.indexes_mut();
            let activated = indexes.node_properties.activate(key);
            indexes
                .node_properties
                .insert_with_scopes(node_id, labels, key, value);
            activated
        };
        if activated {
            self.indexes
                .active_node_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn index_relationship_property_eager<'a>(
        &mut self,
        rel_id: RelationshipId,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        if PropertyIndexKey::from_value(value).is_none() {
            return;
        }

        let activated = {
            let indexes = self.indexes_mut();
            let activated = indexes.relationship_properties.activate(key);
            indexes
                .relationship_properties
                .insert_with_scopes(rel_id, scopes, key, value);
            activated
        };
        if activated {
            self.indexes
                .active_relationship_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn index_node_properties_eager<'a>(
        &mut self,
        node_id: NodeId,
        labels: impl IntoIterator<Item = &'a str> + Clone,
        properties: &Properties,
    ) {
        for (key, value) in properties {
            self.index_node_property_eager(node_id, labels.clone(), key, value);
        }
    }

    fn index_relationship_properties_eager<'a>(
        &mut self,
        rel_id: RelationshipId,
        scopes: impl IntoIterator<Item = &'a str> + Clone,
        properties: &Properties,
    ) {
        for (key, value) in properties {
            self.index_relationship_property_eager(rel_id, scopes.clone(), key, value);
        }
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
                .filter(|node| node.properties.get(key) == Some(value))
                .cloned()
                .collect(),
            None => self
                .iter_node_records()
                .filter(|node| node.properties.get(key) == Some(value))
                .cloned()
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
                    (self.node_at(id)?.properties.get(key) == Some(value)).then_some(id)
                })
                .collect(),
            None => self
                .iter_nodes()
                .filter_map(|(id, node)| (node.properties.get(key) == Some(value)).then_some(id))
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
            .any(|node| node.properties.get(key) == Some(value))
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
                .filter(|rel| rel.properties.get(key) == Some(value))
                .cloned()
                .collect(),
            None => self
                .iter_rel_records()
                .filter(|rel| rel.properties.get(key) == Some(value))
                .cloned()
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
                    (self.rel_at(id)?.properties.get(key) == Some(value)).then_some(id)
                })
                .collect(),
            None => self
                .iter_rels()
                .filter_map(|(id, rel)| (rel.properties.get(key) == Some(value)).then_some(id))
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
            .any(|rel| rel.properties.get(key) == Some(value))
    }

    pub(super) fn attach_relationship(&mut self, rel: &RelationshipRecord) {
        self.outgoing_push(rel.src, rel.id);
        self.incoming_push(rel.dst, rel.id);
        self.insert_relationship_type_index(rel.id, &rel.rel_type);
    }

    fn detach_relationship_indexes(&mut self, rel: &RelationshipRecord) {
        // Adjacency is now positional `Vec<Vec<RelationshipId>>` — clearing
        // the inner Vec leaves the slot in place (the slot is sized for the
        // node's lifetime, not the edge's).
        self.outgoing_remove(rel.src, rel.id);
        self.incoming_remove(rel.dst, rel.id);

        self.remove_relationship_type_index(rel.id, &rel.rel_type);
    }

    pub(super) fn relationship_ids_for_direction(
        &self,
        node_id: NodeId,
        direction: Direction,
    ) -> Vec<RelationshipId> {
        match direction {
            Direction::Left => self
                .incoming_at(node_id)
                .map(<[_]>::to_vec)
                .unwrap_or_default(),

            Direction::Right => self
                .outgoing_at(node_id)
                .map(<[_]>::to_vec)
                .unwrap_or_default(),

            Direction::Undirected => {
                let out = self.outgoing_at(node_id);
                let inc = self.incoming_at(node_id);
                let mut ids = Vec::with_capacity(
                    out.map(<[_]>::len).unwrap_or(0) + inc.map(<[_]>::len).unwrap_or(0),
                );

                if let Some(out) = out {
                    ids.extend(out.iter().copied());
                }
                if let Some(inc) = inc {
                    for &rel_id in inc {
                        let Some(rel) = self.rel_at(rel_id) else {
                            continue;
                        };
                        if rel.src == node_id && rel.dst == node_id {
                            continue;
                        }
                        ids.push(rel_id);
                    }
                }

                ids
            }
        }
    }

    pub(super) fn other_endpoint(rel: &RelationshipRecord, node_id: NodeId) -> Option<NodeId> {
        if rel.src == node_id {
            Some(rel.dst)
        } else if rel.dst == node_id {
            Some(rel.src)
        } else {
            None
        }
    }

    pub(super) fn has_incident_relationships(&self, node_id: NodeId) -> bool {
        self.outgoing_at(node_id)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
            || self
                .incoming_at(node_id)
                .map(|ids| !ids.is_empty())
                .unwrap_or(false)
    }

    pub(super) fn incident_relationship_ids(&self, node_id: NodeId) -> Vec<RelationshipId> {
        let out = self.outgoing_at(node_id);
        let inc = self.incoming_at(node_id);
        let mut rel_ids =
            Vec::with_capacity(out.map(<[_]>::len).unwrap_or(0) + inc.map(<[_]>::len).unwrap_or(0));

        if let Some(ids) = out {
            rel_ids.extend(ids.iter().copied());
        }
        if let Some(ids) = inc {
            for &rel_id in ids {
                let Some(rel) = self.rel_at(rel_id) else {
                    continue;
                };
                if rel.src == node_id && rel.dst == node_id {
                    continue;
                }
                rel_ids.push(rel_id);
            }
        }

        rel_ids
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
            labels: labels.clone(),
            properties,
        };

        self.put_node_at_slot(idx, node.clone());
        self.on_node_replayed(&node);

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
            rel_type: trimmed.to_string(),
            properties,
        };

        self.put_rel_at_slot(idx, rel.clone());
        self.on_relationship_replayed(&rel);

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
            ..PropertyIndexState::default()
        };
        for (id, node) in self.iter_nodes() {
            for (key, value) in &node.properties {
                if expected_nodes.is_active(key) {
                    expected_nodes.insert_with_scopes(
                        id,
                        node.labels.iter().map(String::as_str),
                        key,
                        value,
                    );
                }
            }
        }
        assert_eq!(
            indexes.node_properties.values, expected_nodes.values,
            "node property index values diverged from scan"
        );
        assert_eq!(
            indexes.node_properties.scoped_values, expected_nodes.scoped_values,
            "node property scoped index values diverged from scan"
        );

        let mut expected_relationships = PropertyIndexState {
            active_keys: indexes.relationship_properties.active_keys.clone(),
            ..PropertyIndexState::default()
        };
        for (id, rel) in self.iter_rels() {
            for (key, value) in &rel.properties {
                if expected_relationships.is_active(key) {
                    expected_relationships.insert_with_scopes(
                        id,
                        [rel.rel_type.as_str()],
                        key,
                        value,
                    );
                }
            }
        }
        assert_eq!(
            indexes.relationship_properties.values, expected_relationships.values,
            "relationship property index values diverged from scan"
        );
        assert_eq!(
            indexes.relationship_properties.scoped_values, expected_relationships.scoped_values,
            "relationship property scoped index values diverged from scan"
        );
    }
}

/// Read the optional `cell_size` from a POINT index `OPTIONS` map.
/// Falls back to the registry's default when the key is missing,
/// not numeric, or non-positive.
fn point_cell_size_from_options(
    options: &std::collections::BTreeMap<String, IndexConfigValue>,
) -> Option<f64> {
    let raw = options.get("cellSize")?;
    match raw {
        IndexConfigValue::Number(v) if *v > 0.0 && v.is_finite() => Some(*v),
        IndexConfigValue::Integer(v) if *v > 0 => Some(*v as f64),
        _ => None,
    }
}
