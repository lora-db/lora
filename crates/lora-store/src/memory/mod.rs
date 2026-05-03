use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use lora_ast::Direction;

use crate::{
    MutationEvent, MutationRecorder, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId,
    RelationshipRecord,
};

mod property_index;
#[cfg(test)]
use property_index::PropertyIndexState;
use property_index::{PropertyIndexKey, PropertyIndexRegistry};

#[derive(Default)]
pub struct InMemoryGraph {
    next_node_id: NodeId,
    next_rel_id: RelationshipId,

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
    nodes: Vec<Option<Arc<NodeRecord>>>,
    relationships: Vec<Option<Arc<RelationshipRecord>>>,
    /// Live (non-tombstoned) counts kept in sync with `put_*`/`take_*` so
    /// `node_count` / `relationship_count` stay O(1) — without a counter
    /// they'd have to scan the slab.
    live_node_count: usize,
    live_rel_count: usize,

    /// Adjacency keyed by NodeId. `outgoing[id]` is the list of relationship
    /// ids that leave `id`; mirrored on `incoming[id]`. Inner `Vec` instead
    /// of `BTreeSet` because edges are inserted exactly once and traversal
    /// only needs sequential iteration; the cache-friendly contiguous layout
    /// shows up on every traversal hop.
    outgoing: Vec<Vec<RelationshipId>>,
    incoming: Vec<Vec<RelationshipId>>,

    // secondary indexes
    /// Label -> the (unique, monotonic) node ids that carry it. The inner
    /// `Vec` instead of `BTreeSet` because every node id is inserted at most
    /// once per label (no dedup needed) and every consumer iterates the
    /// whole list anyway — contiguous storage iterates faster than a
    /// tree-of-pointers, and removes via `swap_remove` stay O(degree-of-label).
    nodes_by_label: BTreeMap<String, Vec<NodeId>>,
    relationships_by_type: BTreeMap<String, Vec<RelationshipId>>,
    indexes: RwLock<PropertyIndexRegistry>,
    active_node_property_indexes: AtomicUsize,
    active_relationship_property_indexes: AtomicUsize,

    /// Optional mutation observer. When `Some`, every committed mutation
    /// fans out to this recorder *after* the in-memory state has been
    /// updated. The recorder is not part of the graph's identity, so Clone
    /// and snapshot restore both reset it to `None`.
    recorder: Option<Arc<dyn MutationRecorder>>,
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
            indexes: RwLock::new(if self.has_active_property_indexes() {
                self.indexes_read().clone()
            } else {
                PropertyIndexRegistry::default()
            }),
            active_node_property_indexes: AtomicUsize::new(self.active_node_property_index_count()),
            active_relationship_property_indexes: AtomicUsize::new(
                self.active_relationship_property_index_count(),
            ),
            recorder: None,
        }
    }
}

impl InMemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity_hint(_nodes: usize, _relationships: usize) -> Self {
        // BTreeMap/BTreeSet do not support capacity reservation.
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

    /// Handle to the currently-installed recorder, if any.
    pub fn mutation_recorder(&self) -> Option<&Arc<dyn MutationRecorder>> {
        self.recorder.as_ref()
    }

    /// Emit a mutation event only if a recorder is installed. The event is
    /// built lazily — callers pass a closure, so when no recorder is
    /// attached we pay only a `None` check and the cost of constructing the
    /// event (labels/properties clones) is avoided.
    #[inline]
    fn emit<F: FnOnce() -> MutationEvent>(&self, build: F) {
        if let Some(rec) = &self.recorder {
            rec.record(build());
        }
    }

    fn alloc_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    fn alloc_rel_id(&mut self) -> RelationshipId {
        let id = self.next_rel_id;
        self.next_rel_id += 1;
        id
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

    // ---------- Slab access helpers ----------
    //
    // Stand-in for the BTreeMap API the previous storage used. They keep the
    // call sites readable while the underlying layout is positional Vec.

    #[inline]
    fn node_at(&self, id: NodeId) -> Option<&NodeRecord> {
        self.nodes
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    /// Mutable handle to a node record, doing copy-on-write only when the
    /// `Arc` is shared with a concurrent reader. With no readers (the
    /// common case after a fresh write_store clone), `Arc::make_mut`
    /// upgrades in place — no record clone.
    #[inline]
    fn node_at_mut(&mut self, id: NodeId) -> Option<&mut NodeRecord> {
        self.nodes
            .get_mut(id as usize)
            .and_then(|s| s.as_mut())
            .map(Arc::make_mut)
    }

    #[inline]
    fn rel_at(&self, id: RelationshipId) -> Option<&RelationshipRecord> {
        self.relationships
            .get(id as usize)
            .and_then(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    #[inline]
    fn rel_at_mut(&mut self, id: RelationshipId) -> Option<&mut RelationshipRecord> {
        self.relationships
            .get_mut(id as usize)
            .and_then(|s| s.as_mut())
            .map(Arc::make_mut)
    }

    /// Resize the node-keyed Vecs so `id as usize` is in range. Adjacency
    /// lists are kept in lockstep with `nodes`, so a freshly-grown slot has
    /// empty outgoing/incoming Vecs ready to receive edges.
    fn ensure_node_slot(&mut self, id: NodeId) {
        let target = id as usize + 1;
        if self.nodes.len() < target {
            self.nodes.resize_with(target, || None);
            self.outgoing.resize_with(target, Vec::new);
            self.incoming.resize_with(target, Vec::new);
        }
    }

    fn ensure_rel_slot(&mut self, id: RelationshipId) {
        let target = id as usize + 1;
        if self.relationships.len() < target {
            self.relationships.resize_with(target, || None);
        }
    }

    fn put_node(&mut self, id: NodeId, node: NodeRecord) {
        self.ensure_node_slot(id);
        let was_present = self.nodes[id as usize].is_some();
        self.nodes[id as usize] = Some(Arc::new(node));
        if !was_present {
            self.live_node_count += 1;
        }
    }

    fn put_rel(&mut self, id: RelationshipId, rel: RelationshipRecord) {
        self.ensure_rel_slot(id);
        let was_present = self.relationships[id as usize].is_some();
        self.relationships[id as usize] = Some(Arc::new(rel));
        if !was_present {
            self.live_rel_count += 1;
        }
    }

    fn take_node(&mut self, id: NodeId) -> Option<NodeRecord> {
        let idx = id as usize;
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

    fn take_rel(&mut self, id: RelationshipId) -> Option<RelationshipRecord> {
        let idx = id as usize;
        let removed = self.relationships.get_mut(idx).and_then(|s| s.take());
        if removed.is_some() {
            self.live_rel_count -= 1;
        }
        removed.map(|arc| Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
    }

    #[inline]
    fn outgoing_at(&self, id: NodeId) -> Option<&Vec<RelationshipId>> {
        self.outgoing.get(id as usize)
    }

    #[inline]
    fn incoming_at(&self, id: NodeId) -> Option<&Vec<RelationshipId>> {
        self.incoming.get(id as usize)
    }

    fn iter_node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| i as NodeId))
    }

    fn iter_node_records(&self) -> impl Iterator<Item = &NodeRecord> + '_ {
        self.nodes
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    fn iter_rel_ids(&self) -> impl Iterator<Item = RelationshipId> + '_ {
        self.relationships
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| i as RelationshipId))
    }

    fn iter_rel_records(&self) -> impl Iterator<Item = &RelationshipRecord> + '_ {
        self.relationships
            .iter()
            .filter_map(|s| s.as_ref())
            .map(|arc| arc.as_ref())
    }

    fn iter_nodes(&self) -> impl Iterator<Item = (NodeId, &NodeRecord)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|n| (i as NodeId, n.as_ref())))
    }

    fn iter_rels(&self) -> impl Iterator<Item = (RelationshipId, &RelationshipRecord)> + '_ {
        self.relationships
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|r| (i as RelationshipId, r.as_ref())))
    }

    /// Add `rel_id` to `node_id`'s outgoing list. Idempotent: skips the push
    /// if the id is already present (it shouldn't be — relationship ids are
    /// monotonic — but the guard is cheap and keeps the invariant explicit).
    fn outgoing_push(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        self.ensure_node_slot(node_id);
        let v = &mut self.outgoing[node_id as usize];
        if !v.contains(&rel_id) {
            v.push(rel_id);
        }
    }

    fn incoming_push(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        self.ensure_node_slot(node_id);
        let v = &mut self.incoming[node_id as usize];
        if !v.contains(&rel_id) {
            v.push(rel_id);
        }
    }

    /// Remove `rel_id` from `node_id`'s outgoing list. `swap_remove` keeps
    /// the operation O(1) — adjacency order doesn't carry semantic meaning.
    fn outgoing_remove(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        if let Some(v) = self.outgoing.get_mut(node_id as usize) {
            if let Some(pos) = v.iter().position(|&id| id == rel_id) {
                v.swap_remove(pos);
            }
        }
    }

    fn incoming_remove(&mut self, node_id: NodeId, rel_id: RelationshipId) {
        if let Some(v) = self.incoming.get_mut(node_id as usize) {
            if let Some(pos) = v.iter().position(|&id| id == rel_id) {
                v.swap_remove(pos);
            }
        }
    }

    fn normalize_labels(labels: Vec<String>) -> Vec<String> {
        let mut seen = BTreeSet::new();

        labels
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .filter(|s| seen.insert(s.clone()))
            .collect()
    }

    fn insert_node_label_index(&mut self, node_id: NodeId, label: &str) {
        let bucket = self.nodes_by_label.entry(label.to_string()).or_default();
        // Same monotonic-id invariant as `outgoing_push`: ids never appear
        // twice, but the `contains` guard is cheap on small buckets and
        // makes the invariant explicit for replay paths.
        if !bucket.contains(&node_id) {
            bucket.push(node_id);
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
        let bucket = self
            .relationships_by_type
            .entry(rel_type.to_string())
            .or_default();
        if !bucket.contains(&rel_id) {
            bucket.push(rel_id);
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

    fn indexes_read(&self) -> std::sync::RwLockReadGuard<'_, PropertyIndexRegistry> {
        self.indexes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn indexes_write(&self) -> RwLockWriteGuard<'_, PropertyIndexRegistry> {
        self.indexes
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn indexes_mut(&mut self) -> &mut PropertyIndexRegistry {
        self.indexes
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[inline]
    fn active_node_property_index_count(&self) -> usize {
        self.active_node_property_indexes.load(Ordering::Relaxed)
    }

    #[inline]
    fn active_relationship_property_index_count(&self) -> usize {
        self.active_relationship_property_indexes
            .load(Ordering::Relaxed)
    }

    #[inline]
    fn has_active_property_indexes(&self) -> bool {
        self.active_node_property_index_count() != 0
            || self.active_relationship_property_index_count() != 0
    }

    fn node_property_index_is_active(&mut self, key: &str) -> bool {
        self.active_node_property_index_count() != 0
            && self.indexes_mut().node_properties.is_active(key)
    }

    fn relationship_property_index_is_active(&mut self, key: &str) -> bool {
        self.active_relationship_property_index_count() != 0
            && self.indexes_mut().relationship_properties.is_active(key)
    }

    fn ensure_node_property_index(&self, key: &str) {
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
            self.active_node_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn ensure_relationship_property_index(&self, key: &str) {
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
            self.active_relationship_property_indexes
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn rebuild_property_indexes(&mut self) {
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
        self.active_node_property_indexes
            .store(node_index_count, Ordering::Relaxed);
        self.active_relationship_property_indexes
            .store(relationship_index_count, Ordering::Relaxed);
    }

    fn on_node_created(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.insert_node_label_index(node.id, label);
        }
        self.index_node_properties_if_active(
            node.id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
    }

    fn on_node_replayed(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.insert_node_label_index(node.id, label);
        }
        self.index_node_properties_eager(
            node.id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
    }

    fn on_node_property_set(
        &mut self,
        node_id: NodeId,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        if !self.node_property_index_is_active(key) {
            return;
        }

        let Some(labels) = self.node_at(node_id).map(|node| node.labels.clone()) else {
            return;
        };

        if let Some(old) = old {
            self.unindex_node_property_if_active(
                node_id,
                labels.iter().map(String::as_str),
                key,
                old,
            );
        }
        self.index_node_property_if_active(node_id, labels.iter().map(String::as_str), key, new);
    }

    fn on_node_property_removed(&mut self, node_id: NodeId, key: &str, old: &PropertyValue) {
        if !self.node_property_index_is_active(key) {
            return;
        }

        let Some(labels) = self.node_at(node_id).map(|node| node.labels.clone()) else {
            return;
        };
        self.unindex_node_property_if_active(node_id, labels.iter().map(String::as_str), key, old);
    }

    fn on_node_label_added(&mut self, node_id: NodeId, label: &str) {
        self.insert_node_label_index(node_id, label);

        if self.active_node_property_index_count() == 0 {
            return;
        }

        let Some(properties) = self.node_at(node_id).map(|node| node.properties.clone()) else {
            return;
        };
        self.index_node_scope_properties_if_active(node_id, label, &properties);
    }

    fn on_node_label_removed(&mut self, node_id: NodeId, label: &str) {
        self.remove_node_label_index(node_id, label);

        if self.active_node_property_index_count() == 0 {
            return;
        }

        let Some(properties) = self.node_at(node_id).map(|node| node.properties.clone()) else {
            return;
        };
        self.unindex_node_scope_properties_if_active(node_id, label, &properties);
    }

    fn on_node_deleted(&mut self, node: &NodeRecord) {
        for label in &node.labels {
            self.remove_node_label_index(node.id, label);
        }
        self.unindex_active_node_properties(
            node.id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
    }

    fn on_relationship_created(&mut self, rel: &RelationshipRecord) {
        self.attach_relationship(rel);
        self.index_relationship_properties_if_active(
            rel.id,
            [rel.rel_type.as_str()],
            &rel.properties,
        );
    }

    fn on_relationship_replayed(&mut self, rel: &RelationshipRecord) {
        self.attach_relationship(rel);
        self.index_relationship_properties_eager(rel.id, [rel.rel_type.as_str()], &rel.properties);
    }

    fn on_relationship_property_set(
        &mut self,
        rel_id: RelationshipId,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        if !self.relationship_property_index_is_active(key) {
            return;
        }

        let Some(rel_type) = self.rel_at(rel_id).map(|rel| rel.rel_type.clone()) else {
            return;
        };

        if let Some(old) = old {
            self.unindex_relationship_property_if_active(rel_id, [rel_type.as_str()], key, old);
        }
        self.index_relationship_property_if_active(rel_id, [rel_type.as_str()], key, new);
    }

    fn on_relationship_property_removed(
        &mut self,
        rel_id: RelationshipId,
        key: &str,
        old: &PropertyValue,
    ) {
        if !self.relationship_property_index_is_active(key) {
            return;
        }

        let Some(rel_type) = self.rel_at(rel_id).map(|rel| rel.rel_type.clone()) else {
            return;
        };
        self.unindex_relationship_property_if_active(rel_id, [rel_type.as_str()], key, old);
    }

    fn on_relationship_deleted(&mut self, rel: &RelationshipRecord) {
        self.detach_relationship_indexes(rel);
        self.unindex_active_relationship_properties(
            rel.id,
            [rel.rel_type.as_str()],
            &rel.properties,
        );
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
            self.active_node_property_indexes
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
            self.active_relationship_property_indexes
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

    fn scan_nodes_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeRecord> {
        let candidates: Box<dyn Iterator<Item = &NodeRecord> + '_> = match label {
            Some(label) => Box::new(
                self.nodes_by_label
                    .get(label)
                    .into_iter()
                    .flat_map(|ids| ids.iter())
                    .filter_map(|&id| self.node_at(id)),
            ),
            None => Box::new(self.iter_node_records()),
        };

        candidates
            .filter(|node| node.properties.get(key) == Some(value))
            .cloned()
            .collect()
    }

    fn scan_relationships_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipRecord> {
        let candidates: Box<dyn Iterator<Item = &RelationshipRecord> + '_> = match rel_type {
            Some(rel_type) => Box::new(
                self.relationships_by_type
                    .get(rel_type)
                    .into_iter()
                    .flat_map(|ids| ids.iter())
                    .filter_map(|&id| self.rel_at(id)),
            ),
            None => Box::new(self.iter_rel_records()),
        };

        candidates
            .filter(|rel| rel.properties.get(key) == Some(value))
            .cloned()
            .collect()
    }

    fn attach_relationship(&mut self, rel: &RelationshipRecord) {
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

    fn relationship_ids_for_direction(
        &self,
        node_id: NodeId,
        direction: Direction,
    ) -> Vec<RelationshipId> {
        match direction {
            Direction::Left => self.incoming_at(node_id).cloned().unwrap_or_default(),

            Direction::Right => self.outgoing_at(node_id).cloned().unwrap_or_default(),

            Direction::Undirected => {
                let mut ids = BTreeSet::new();

                if let Some(out) = self.outgoing_at(node_id) {
                    ids.extend(out.iter().copied());
                }
                if let Some(inc) = self.incoming_at(node_id) {
                    ids.extend(inc.iter().copied());
                }

                ids.into_iter().collect()
            }
        }
    }

    fn other_endpoint(rel: &RelationshipRecord, node_id: NodeId) -> Option<NodeId> {
        if rel.src == node_id {
            Some(rel.dst)
        } else if rel.dst == node_id {
            Some(rel.src)
        } else {
            None
        }
    }

    fn has_incident_relationships(&self, node_id: NodeId) -> bool {
        self.outgoing_at(node_id)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
            || self
                .incoming_at(node_id)
                .map(|ids| !ids.is_empty())
                .unwrap_or(false)
    }

    fn incident_relationship_ids(&self, node_id: NodeId) -> BTreeSet<RelationshipId> {
        let mut rel_ids = BTreeSet::new();

        if let Some(ids) = self.outgoing_at(node_id) {
            rel_ids.extend(ids.iter().copied());
        }
        if let Some(ids) = self.incoming_at(node_id) {
            rel_ids.extend(ids.iter().copied());
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

        let labels = Self::normalize_labels(labels);
        let node = NodeRecord {
            id,
            labels: labels.clone(),
            properties,
        };

        self.on_node_replayed(&node);
        self.put_node(id, node.clone());
        // ensure_node_slot grew both adjacency Vecs to cover this id when
        // we put_node above.
        self.bump_next_node_id_past(id)?;

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

        let rel = RelationshipRecord {
            id,
            src,
            dst,
            rel_type: trimmed.to_string(),
            properties,
        };

        self.on_relationship_replayed(&rel);
        self.put_rel(id, rel.clone());
        self.bump_next_rel_id_past(id)?;

        Ok(rel)
    }

    #[cfg(test)]
    fn assert_property_indexes_match_scan(&self) {
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
mod impls;
mod snapshot;

#[cfg(test)]
mod tests;
