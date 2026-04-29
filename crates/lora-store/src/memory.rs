use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use lora_ast::Direction;

use crate::snapshot::{read_snapshot, write_snapshot, SNAPSHOT_FORMAT_VERSION};
use crate::{
    BorrowedGraphStorage, GraphStorage, GraphStorageMut, LoraBinary, MutationEvent,
    MutationRecorder, NodeId, NodeRecord, Properties, PropertyValue, RelationshipId,
    RelationshipRecord, SnapshotError, SnapshotMeta, SnapshotPayload, Snapshotable,
};

type PropertyValueBuckets = HashMap<PropertyIndexKey, BTreeSet<u64>>;
type PropertyIndex = HashMap<String, PropertyValueBuckets>;
type ScopedPropertyIndex = HashMap<String, PropertyIndex>;

#[derive(Default)]
struct PropertyIndexRegistry {
    node_properties: PropertyIndexState,
    relationship_properties: PropertyIndexState,
}

impl std::fmt::Debug for PropertyIndexRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PropertyIndexRegistry")
            .field("node_properties", &self.node_properties)
            .field("relationship_properties", &self.relationship_properties)
            .finish()
    }
}

impl Clone for PropertyIndexRegistry {
    fn clone(&self) -> Self {
        Self {
            node_properties: self.node_properties.clone(),
            relationship_properties: self.relationship_properties.clone(),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct PropertyIndexState {
    active_keys: BTreeSet<String>,
    values: PropertyIndex,
    scoped_values: ScopedPropertyIndex,
}

impl PropertyIndexState {
    fn is_active(&self, key: &str) -> bool {
        self.active_keys.contains(key)
    }

    fn activate(&mut self, key: &str) -> bool {
        self.active_keys.insert(key.to_string())
    }

    fn insert_value(
        values: &mut PropertyIndex,
        entity_id: u64,
        key: &str,
        value: PropertyIndexKey,
    ) {
        values
            .entry(key.to_string())
            .or_default()
            .entry(value)
            .or_default()
            .insert(entity_id);
    }

    fn remove_value(
        values: &mut PropertyIndex,
        entity_id: u64,
        key: &str,
        value: &PropertyIndexKey,
    ) {
        let mut remove_key = false;
        if let Some(buckets) = values.get_mut(key) {
            if let Some(ids) = buckets.get_mut(value) {
                ids.remove(&entity_id);
                if ids.is_empty() {
                    buckets.remove(value);
                }
            }
            remove_key = buckets.is_empty();
        }
        if remove_key {
            values.remove(key);
        }
    }

    fn insert_scoped(&mut self, entity_id: u64, scope: &str, key: &str, value: &PropertyValue) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        Self::insert_value(
            self.scoped_values.entry(scope.to_string()).or_default(),
            entity_id,
            key,
            indexed_value,
        );
    }

    fn insert_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        Self::insert_value(&mut self.values, entity_id, key, indexed_value.clone());
        for scope in scopes {
            Self::insert_value(
                self.scoped_values.entry(scope.to_string()).or_default(),
                entity_id,
                key,
                indexed_value.clone(),
            );
        }
    }

    fn remove_scoped(&mut self, entity_id: u64, scope: &str, key: &str, value: &PropertyValue) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        let mut remove_scope = false;
        if let Some(values) = self.scoped_values.get_mut(scope) {
            Self::remove_value(values, entity_id, key, &indexed_value);
            remove_scope = values.is_empty();
        }
        if remove_scope {
            self.scoped_values.remove(scope);
        }
    }

    fn remove_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        Self::remove_value(&mut self.values, entity_id, key, &indexed_value);
        for scope in scopes {
            let mut remove_scope = false;
            if let Some(values) = self.scoped_values.get_mut(scope) {
                Self::remove_value(values, entity_id, key, &indexed_value);
                remove_scope = values.is_empty();
            }
            if remove_scope {
                self.scoped_values.remove(scope);
            }
        }
    }

    fn ids_for(&self, key: &str, value: &PropertyValue) -> Option<&BTreeSet<u64>> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.values
            .get(key)
            .and_then(|values| values.get(&indexed_value))
    }

    fn scoped_ids_for(
        &self,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) -> Option<&BTreeSet<u64>> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.scoped_values
            .get(scope)
            .and_then(|values| values.get(key))
            .and_then(|values| values.get(&indexed_value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PropertyIndexKey {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64),
    String(String),
    Binary(LoraBinary),
    List(Vec<PropertyIndexKey>),
    Map(BTreeMap<String, PropertyIndexKey>),
}

impl PropertyIndexKey {
    fn from_value(value: &PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Null => Some(Self::Null),
            PropertyValue::Bool(v) => Some(Self::Bool(*v)),
            PropertyValue::Int(v) => Some(Self::Int(*v)),
            PropertyValue::Float(v) => {
                if v.is_nan() {
                    None
                } else if *v == 0.0 {
                    Some(Self::Float(0.0f64.to_bits()))
                } else {
                    Some(Self::Float(v.to_bits()))
                }
            }
            PropertyValue::String(v) => Some(Self::String(v.clone())),
            PropertyValue::Binary(v) => Some(Self::Binary(v.clone())),
            PropertyValue::List(values) => values
                .iter()
                .map(Self::from_value)
                .collect::<Option<Vec<_>>>()
                .map(Self::List),
            PropertyValue::Map(values) => values
                .iter()
                .map(|(k, v)| Self::from_value(v).map(|indexed| (k.clone(), indexed)))
                .collect::<Option<BTreeMap<_, _>>>()
                .map(Self::Map),
            // Temporal, spatial, and vector values have richer equality
            // semantics and/or no stable hash representation in the storage
            // crate today. Those continue to use the scan fallback.
            PropertyValue::Date(_)
            | PropertyValue::Time(_)
            | PropertyValue::LocalTime(_)
            | PropertyValue::DateTime(_)
            | PropertyValue::LocalDateTime(_)
            | PropertyValue::Duration(_)
            | PropertyValue::Point(_)
            | PropertyValue::Vector(_) => None,
        }
    }
}

#[derive(Default)]
pub struct InMemoryGraph {
    next_node_id: NodeId,
    next_rel_id: RelationshipId,

    nodes: BTreeMap<NodeId, NodeRecord>,
    relationships: BTreeMap<RelationshipId, RelationshipRecord>,

    // adjacency
    outgoing: BTreeMap<NodeId, BTreeSet<RelationshipId>>,
    incoming: BTreeMap<NodeId, BTreeSet<RelationshipId>>,

    // secondary indexes
    nodes_by_label: BTreeMap<String, BTreeSet<NodeId>>,
    relationships_by_type: BTreeMap<String, BTreeSet<RelationshipId>>,
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
        self.nodes.contains_key(&node_id)
    }

    pub fn contains_relationship(&self, rel_id: RelationshipId) -> bool {
        self.relationships.contains_key(&rel_id)
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
        self.nodes_by_label
            .entry(label.to_string())
            .or_default()
            .insert(node_id);
    }

    fn remove_node_label_index(&mut self, node_id: NodeId, label: &str) {
        if let Some(ids) = self.nodes_by_label.get_mut(label) {
            ids.remove(&node_id);
            if ids.is_empty() {
                self.nodes_by_label.remove(label);
            }
        }
    }

    fn insert_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        self.relationships_by_type
            .entry(rel_type.to_string())
            .or_default()
            .insert(rel_id);
    }

    fn remove_relationship_type_index(&mut self, rel_id: RelationshipId, rel_type: &str) {
        if let Some(ids) = self.relationships_by_type.get_mut(rel_type) {
            ids.remove(&rel_id);
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

        for (id, node) in &self.nodes {
            if let Some(value) = node.properties.get(key) {
                indexes.node_properties.insert_with_scopes(
                    *id,
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

        for (id, rel) in &self.relationships {
            if let Some(value) = rel.properties.get(key) {
                indexes.relationship_properties.insert_with_scopes(
                    *id,
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

        for (id, node) in &self.nodes {
            for (key, value) in &node.properties {
                if PropertyIndexKey::from_value(value).is_some() {
                    indexes.node_properties.activate(key);
                    indexes.node_properties.insert_with_scopes(
                        *id,
                        node.labels.iter().map(String::as_str),
                        key,
                        value,
                    );
                }
            }
        }

        for (id, rel) in &self.relationships {
            for (key, value) in &rel.properties {
                if PropertyIndexKey::from_value(value).is_some() {
                    indexes.relationship_properties.activate(key);
                    indexes.relationship_properties.insert_with_scopes(
                        *id,
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
                    .filter_map(|id| self.nodes.get(id)),
            ),
            None => Box::new(self.nodes.values()),
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
                    .filter_map(|id| self.relationships.get(id)),
            ),
            None => Box::new(self.relationships.values()),
        };

        candidates
            .filter(|rel| rel.properties.get(key) == Some(value))
            .cloned()
            .collect()
    }

    fn attach_relationship(&mut self, rel: &RelationshipRecord) {
        self.outgoing.entry(rel.src).or_default().insert(rel.id);
        self.incoming.entry(rel.dst).or_default().insert(rel.id);
        self.insert_relationship_type_index(rel.id, &rel.rel_type);
    }

    fn detach_relationship_indexes(&mut self, rel: &RelationshipRecord) {
        if let Some(ids) = self.outgoing.get_mut(&rel.src) {
            ids.remove(&rel.id);
            if ids.is_empty() {
                self.outgoing.remove(&rel.src);
            }
        }

        if let Some(ids) = self.incoming.get_mut(&rel.dst) {
            ids.remove(&rel.id);
            if ids.is_empty() {
                self.incoming.remove(&rel.dst);
            }
        }

        self.remove_relationship_type_index(rel.id, &rel.rel_type);
    }

    fn relationship_ids_for_direction(
        &self,
        node_id: NodeId,
        direction: Direction,
    ) -> Vec<RelationshipId> {
        match direction {
            Direction::Left => self
                .incoming
                .get(&node_id)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),

            Direction::Right => self
                .outgoing
                .get(&node_id)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),

            Direction::Undirected => {
                let mut ids = BTreeSet::new();

                if let Some(out) = self.outgoing.get(&node_id) {
                    ids.extend(out.iter().copied());
                }
                if let Some(inc) = self.incoming.get(&node_id) {
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
        self.outgoing
            .get(&node_id)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
            || self
                .incoming
                .get(&node_id)
                .map(|ids| !ids.is_empty())
                .unwrap_or(false)
    }

    fn incident_relationship_ids(&self, node_id: NodeId) -> BTreeSet<RelationshipId> {
        let mut rel_ids = BTreeSet::new();

        if let Some(ids) = self.outgoing.get(&node_id) {
            rel_ids.extend(ids.iter().copied());
        }
        if let Some(ids) = self.incoming.get(&node_id) {
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
        if self.nodes.contains_key(&id) {
            return Err(format!("node id {id} already exists"));
        }

        let labels = Self::normalize_labels(labels);
        let node = NodeRecord {
            id,
            labels: labels.clone(),
            properties,
        };

        for label in &labels {
            self.insert_node_label_index(id, label);
        }
        self.index_node_properties_eager(
            id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );
        self.nodes.insert(id, node.clone());
        self.outgoing.entry(id).or_default();
        self.incoming.entry(id).or_default();
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
        if self.relationships.contains_key(&id) {
            return Err(format!("relationship id {id} already exists"));
        }
        if !self.nodes.contains_key(&src) {
            return Err(format!(
                "relationship {id} references missing source node {src}"
            ));
        }
        if !self.nodes.contains_key(&dst) {
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

        self.attach_relationship(&rel);
        self.index_relationship_properties_eager(id, [rel.rel_type.as_str()], &rel.properties);
        self.relationships.insert(id, rel.clone());
        self.bump_next_rel_id_past(id)?;

        Ok(rel)
    }
}

impl GraphStorage for InMemoryGraph {
    // ---------- Required primitives ----------

    fn contains_node(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    fn node(&self, id: NodeId) -> Option<NodeRecord> {
        self.nodes.get(&id).cloned()
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    fn node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        match self.nodes_by_label.get(label) {
            Some(ids) => ids.iter().copied().collect(),
            None => Vec::new(),
        }
    }

    fn contains_relationship(&self, id: RelationshipId) -> bool {
        self.relationships.contains_key(&id)
    }

    fn relationship(&self, id: RelationshipId) -> Option<RelationshipRecord> {
        self.relationships.get(&id).cloned()
    }

    fn all_rel_ids(&self) -> Vec<RelationshipId> {
        self.relationships.keys().copied().collect()
    }

    fn rel_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        match self.relationships_by_type.get(rel_type) {
            Some(ids) => ids.iter().copied().collect(),
            None => Vec::new(),
        }
    }

    fn relationship_endpoints(&self, id: RelationshipId) -> Option<(NodeId, NodeId)> {
        self.relationships.get(&id).map(|r| (r.src, r.dst))
    }

    fn expand_ids(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        if !self.nodes.contains_key(&node_id) {
            return Vec::new();
        }

        // Fast path: no type filter — just join adjacency + rel endpoints.
        if types.is_empty() {
            return self
                .relationship_ids_for_direction(node_id, direction)
                .into_iter()
                .filter_map(|rel_id| {
                    let rel = self.relationships.get(&rel_id)?;
                    let other_id = Self::other_endpoint(rel, node_id)?;
                    Some((rel_id, other_id))
                })
                .collect();
        }

        // Type-filtered: borrow rel_type straight from the stored record.
        // For small type lists (the common case) the linear scan beats a
        // BTreeSet; we keep it allocation-free.
        self.relationship_ids_for_direction(node_id, direction)
            .into_iter()
            .filter_map(|rel_id| {
                let rel = self.relationships.get(&rel_id)?;
                if !types.iter().any(|t| t == &rel.rel_type) {
                    return None;
                }
                let other_id = Self::other_endpoint(rel, node_id)?;
                Some((rel_id, other_id))
            })
            .collect()
    }

    fn all_labels(&self) -> Vec<String> {
        self.nodes_by_label.keys().cloned().collect()
    }

    fn all_relationship_types(&self) -> Vec<String> {
        self.relationships_by_type.keys().cloned().collect()
    }

    // ---------- Optimization hooks: zero-clone borrow access ----------

    fn with_node<F, R>(&self, id: NodeId, f: F) -> Option<R>
    where
        F: FnOnce(&NodeRecord) -> R,
        Self: Sized,
    {
        self.nodes.get(&id).map(f)
    }

    fn with_relationship<F, R>(&self, id: RelationshipId, f: F) -> Option<R>
    where
        F: FnOnce(&RelationshipRecord) -> R,
        Self: Sized,
    {
        self.relationships.get(&id).map(f)
    }

    // ---------- Overrides: counts + existence ----------

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn relationship_count(&self) -> usize {
        self.relationships.len()
    }

    fn has_node(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    fn has_relationship(&self, id: RelationshipId) -> bool {
        self.relationships.contains_key(&id)
    }

    // ---------- Overrides: record-returning scans (direct iteration) ----------

    fn all_nodes(&self) -> Vec<NodeRecord> {
        self.nodes.values().cloned().collect()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord> {
        self.nodes_by_label
            .get(label)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.nodes.get(id).cloned())
            .collect()
    }

    fn all_relationships(&self) -> Vec<RelationshipRecord> {
        self.relationships.values().cloned().collect()
    }

    fn relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord> {
        self.relationships_by_type
            .get(rel_type)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.relationships.get(id).cloned())
            .collect()
    }

    fn find_nodes_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeRecord>
    where
        Self: Sized,
    {
        if PropertyIndexKey::from_value(value).is_none() {
            return self.scan_nodes_by_property(label, key, value);
        }

        self.ensure_node_property_index(key);
        let indexes = self.indexes_read();

        match label {
            Some(label) => {
                let Some(ids) = indexes.node_properties.scoped_ids_for(label, key, value) else {
                    return Vec::new();
                };
                ids.iter()
                    .filter_map(|id| self.nodes.get(id).cloned())
                    .collect()
            }
            None => indexes
                .node_properties
                .ids_for(key, value)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|id| self.nodes.get(id).cloned())
                .collect(),
        }
    }

    fn find_node_ids_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<NodeId>
    where
        Self: Sized,
    {
        if PropertyIndexKey::from_value(value).is_none() {
            return self
                .scan_nodes_by_property(label, key, value)
                .into_iter()
                .map(|n| n.id)
                .collect();
        }

        self.ensure_node_property_index(key);
        let indexes = self.indexes_read();

        match label {
            Some(label) => indexes
                .node_properties
                .scoped_ids_for(label, key, value)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),
            None => indexes
                .node_properties
                .ids_for(key, value)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),
        }
    }

    fn find_relationships_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipRecord>
    where
        Self: Sized,
    {
        if PropertyIndexKey::from_value(value).is_none() {
            return self.scan_relationships_by_property(rel_type, key, value);
        }

        self.ensure_relationship_property_index(key);
        let indexes = self.indexes_read();

        match rel_type {
            Some(rel_type) => {
                let Some(ids) = indexes
                    .relationship_properties
                    .scoped_ids_for(rel_type, key, value)
                else {
                    return Vec::new();
                };
                ids.iter()
                    .filter_map(|id| self.relationships.get(id).cloned())
                    .collect()
            }
            None => indexes
                .relationship_properties
                .ids_for(key, value)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|id| self.relationships.get(id).cloned())
                .collect(),
        }
    }

    fn find_relationship_ids_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: &PropertyValue,
    ) -> Vec<RelationshipId>
    where
        Self: Sized,
    {
        if PropertyIndexKey::from_value(value).is_none() {
            return self
                .scan_relationships_by_property(rel_type, key, value)
                .into_iter()
                .map(|r| r.id)
                .collect();
        }

        self.ensure_relationship_property_index(key);
        let indexes = self.indexes_read();

        match rel_type {
            Some(rel_type) => indexes
                .relationship_properties
                .scoped_ids_for(rel_type, key, value)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),
            None => indexes
                .relationship_properties
                .ids_for(key, value)
                .map(|ids| ids.iter().copied().collect())
                .unwrap_or_default(),
        }
    }

    fn node_exists_with_label_and_property(
        &self,
        label: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool
    where
        Self: Sized,
    {
        if PropertyIndexKey::from_value(value).is_none() {
            return !self
                .scan_nodes_by_property(Some(label), key, value)
                .is_empty();
        }

        self.ensure_node_property_index(key);
        let indexes = self.indexes_read();
        indexes
            .node_properties
            .scoped_ids_for(label, key, value)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
    }

    fn relationship_exists_with_type_and_property(
        &self,
        rel_type: &str,
        key: &str,
        value: &PropertyValue,
    ) -> bool
    where
        Self: Sized,
    {
        if PropertyIndexKey::from_value(value).is_none() {
            return !self
                .scan_relationships_by_property(Some(rel_type), key, value)
                .is_empty();
        }

        self.ensure_relationship_property_index(key);
        let indexes = self.indexes_read();
        indexes
            .relationship_properties
            .scoped_ids_for(rel_type, key, value)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
    }

    // ---------- Overrides: traversal (direct adjacency) ----------

    fn relationship_ids_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipId> {
        self.relationship_ids_for_direction(node_id, direction)
    }

    fn outgoing_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.outgoing
            .get(&node_id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.relationships.get(id).cloned())
            .collect()
    }

    fn incoming_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.incoming
            .get(&node_id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.relationships.get(id).cloned())
            .collect()
    }

    fn degree(&self, node_id: NodeId, direction: Direction) -> usize {
        match direction {
            Direction::Right => self.outgoing.get(&node_id).map(|s| s.len()).unwrap_or(0),
            Direction::Left => self.incoming.get(&node_id).map(|s| s.len()).unwrap_or(0),
            Direction::Undirected => {
                self.outgoing.get(&node_id).map(|s| s.len()).unwrap_or(0)
                    + self.incoming.get(&node_id).map(|s| s.len()).unwrap_or(0)
            }
        }
    }

    fn expand(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipRecord, NodeRecord)> {
        if !self.nodes.contains_key(&node_id) {
            return Vec::new();
        }

        let type_filter: Option<BTreeSet<&str>> = if types.is_empty() {
            None
        } else {
            Some(types.iter().map(String::as_str).collect())
        };

        self.relationship_ids_for_direction(node_id, direction)
            .into_iter()
            .filter_map(|rel_id| self.relationships.get(&rel_id))
            .filter(|rel| {
                type_filter
                    .as_ref()
                    .map(|allowed| allowed.contains(rel.rel_type.as_str()))
                    .unwrap_or(true)
            })
            .filter_map(|rel| {
                let other_id = Self::other_endpoint(rel, node_id)?;
                let other = self.nodes.get(&other_id)?;
                Some((rel.clone(), other.clone()))
            })
            .collect()
    }
    // ---------- Overrides: schema introspection ----------

    fn all_node_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for node in self.nodes.values() {
            for key in node.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn all_relationship_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for rel in self.relationships.values() {
            for key in rel.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn label_property_keys(&self, label: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();

        if let Some(ids) = self.nodes_by_label.get(label) {
            for id in ids {
                if let Some(node) = self.nodes.get(id) {
                    for key in node.properties.keys() {
                        keys.insert(key.clone());
                    }
                }
            }
        }

        keys.into_iter().collect()
    }

    fn rel_type_property_keys(&self, rel_type: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();

        if let Some(ids) = self.relationships_by_type.get(rel_type) {
            for id in ids {
                if let Some(rel) = self.relationships.get(id) {
                    for key in rel.properties.keys() {
                        keys.insert(key.clone());
                    }
                }
            }
        }

        keys.into_iter().collect()
    }
}

impl BorrowedGraphStorage for InMemoryGraph {
    fn node_ref(&self, id: NodeId) -> Option<&NodeRecord> {
        self.nodes.get(&id)
    }

    fn relationship_ref(&self, id: RelationshipId) -> Option<&RelationshipRecord> {
        self.relationships.get(&id)
    }

    fn node_refs(&self) -> Box<dyn Iterator<Item = &NodeRecord> + '_> {
        Box::new(self.nodes.values())
    }

    fn node_refs_by_label(&self, label: &str) -> Box<dyn Iterator<Item = &NodeRecord> + '_> {
        Box::new(
            self.nodes_by_label
                .get(label)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|id| self.nodes.get(id)),
        )
    }

    fn relationship_refs(&self) -> Box<dyn Iterator<Item = &RelationshipRecord> + '_> {
        Box::new(self.relationships.values())
    }

    fn relationship_refs_by_type(
        &self,
        rel_type: &str,
    ) -> Box<dyn Iterator<Item = &RelationshipRecord> + '_> {
        Box::new(
            self.relationships_by_type
                .get(rel_type)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|id| self.relationships.get(id)),
        )
    }
}

impl GraphStorageMut for InMemoryGraph {
    fn create_node(&mut self, labels: Vec<String>, properties: Properties) -> NodeRecord {
        let id = self.alloc_node_id();
        let labels = Self::normalize_labels(labels);

        let node = NodeRecord {
            id,
            labels: labels.clone(),
            properties,
        };

        for label in &labels {
            self.insert_node_label_index(id, label);
        }
        if self.active_node_property_index_count() != 0 {
            self.index_node_properties_if_active(
                id,
                node.labels.iter().map(String::as_str),
                &node.properties,
            );
        }

        self.nodes.insert(id, node.clone());

        self.outgoing.entry(id).or_default();
        self.incoming.entry(id).or_default();

        self.emit(|| MutationEvent::CreateNode {
            id,
            labels: node.labels.clone(),
            properties: node.properties.clone(),
        });

        node
    }

    fn create_relationship(
        &mut self,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        properties: Properties,
    ) -> Option<RelationshipRecord> {
        if !self.nodes.contains_key(&src) || !self.nodes.contains_key(&dst) {
            return None;
        }

        let trimmed = rel_type.trim();
        if trimmed.is_empty() {
            return None;
        }

        let id = self.alloc_rel_id();
        let rel = RelationshipRecord {
            id,
            src,
            dst,
            rel_type: trimmed.to_string(),
            properties,
        };

        self.attach_relationship(&rel);
        if self.active_relationship_property_index_count() != 0 {
            self.index_relationship_properties_if_active(
                id,
                [rel.rel_type.as_str()],
                &rel.properties,
            );
        }
        self.relationships.insert(id, rel.clone());

        self.emit(|| MutationEvent::CreateRelationship {
            id,
            src,
            dst,
            rel_type: rel.rel_type.clone(),
            properties: rel.properties.clone(),
        });

        Some(rel)
    }

    fn set_node_property(&mut self, node_id: NodeId, key: String, value: PropertyValue) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }

        let recorder_active = self.recorder.is_some();
        let (stored_key, stored_value) = if recorder_active {
            (Some(key.clone()), Some(value.clone()))
        } else {
            (None, None)
        };

        let index_active = self.active_node_property_index_count() != 0
            && self.indexes_mut().node_properties.is_active(&key);
        let (old, labels) = match self.nodes.get_mut(&node_id) {
            Some(node) => {
                let labels = if index_active {
                    Some(node.labels.clone())
                } else {
                    None
                };
                (node.properties.insert(key.clone(), value.clone()), labels)
            }
            None => return false,
        };
        if let Some(labels) = labels.as_ref() {
            if let Some(old) = old.as_ref() {
                self.unindex_node_property_if_active(
                    node_id,
                    labels.iter().map(String::as_str),
                    &key,
                    old,
                );
            }
            self.index_node_property_if_active(
                node_id,
                labels.iter().map(String::as_str),
                &key,
                &value,
            );
        }

        self.emit(|| MutationEvent::SetNodeProperty {
            node_id,
            key: stored_key.unwrap(),
            value: stored_value.unwrap(),
        });

        true
    }

    fn remove_node_property(&mut self, node_id: NodeId, key: &str) -> bool {
        let removed = match self.nodes.get_mut(&node_id) {
            Some(node) => node.properties.remove(key),
            None => return false,
        };
        let Some(removed) = removed else {
            return false;
        };

        let labels = if self.active_node_property_index_count() != 0
            && self.indexes_mut().node_properties.is_active(key)
        {
            self.nodes.get(&node_id).map(|node| node.labels.clone())
        } else {
            None
        };

        if let Some(labels) = labels.as_ref() {
            self.unindex_node_property_if_active(
                node_id,
                labels.iter().map(String::as_str),
                key,
                &removed,
            );
        }

        self.emit(|| MutationEvent::RemoveNodeProperty {
            node_id,
            key: key.to_string(),
        });

        true
    }

    fn add_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        let label = label.trim();
        if label.is_empty() {
            return false;
        }

        let index_has_active_keys = self.active_node_property_index_count() != 0;
        let mut scoped_properties = None;
        let applied = match self.nodes.get_mut(&node_id) {
            Some(node) => {
                if node.labels.iter().any(|l| l == label) {
                    return false;
                }

                node.labels.push(label.to_string());
                if index_has_active_keys {
                    scoped_properties = Some(node.properties.clone());
                }
                self.insert_node_label_index(node_id, label);
                true
            }
            None => false,
        };
        if applied {
            if let Some(properties) = scoped_properties.as_ref() {
                self.index_node_scope_properties_if_active(node_id, label, properties);
            }
            self.emit(|| MutationEvent::AddNodeLabel {
                node_id,
                label: label.to_string(),
            });
        }
        applied
    }

    fn remove_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        let index_has_active_keys = self.active_node_property_index_count() != 0;
        let mut scoped_properties = None;
        let applied = match self.nodes.get_mut(&node_id) {
            Some(node) => {
                let original_len = node.labels.len();
                node.labels.retain(|l| l != label);

                if node.labels.len() != original_len {
                    if index_has_active_keys {
                        scoped_properties = Some(node.properties.clone());
                    }
                    self.remove_node_label_index(node_id, label);
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if applied {
            if let Some(properties) = scoped_properties.as_ref() {
                self.unindex_node_scope_properties_if_active(node_id, label, properties);
            }
            self.emit(|| MutationEvent::RemoveNodeLabel {
                node_id,
                label: label.to_string(),
            });
        }
        applied
    }

    fn set_relationship_property(
        &mut self,
        rel_id: RelationshipId,
        key: String,
        value: PropertyValue,
    ) -> bool {
        if !self.relationships.contains_key(&rel_id) {
            return false;
        }

        let recorder_active = self.recorder.is_some();
        let (stored_key, stored_value) = if recorder_active {
            (Some(key.clone()), Some(value.clone()))
        } else {
            (None, None)
        };

        let index_active = self.active_relationship_property_index_count() != 0
            && self.indexes_mut().relationship_properties.is_active(&key);
        let (old, rel_type) = match self.relationships.get_mut(&rel_id) {
            Some(rel) => {
                let rel_type = if index_active {
                    Some(rel.rel_type.clone())
                } else {
                    None
                };
                (rel.properties.insert(key.clone(), value.clone()), rel_type)
            }
            None => return false,
        };
        if let Some(rel_type) = rel_type.as_deref() {
            if let Some(old) = old.as_ref() {
                self.unindex_relationship_property_if_active(rel_id, [rel_type], &key, old);
            }
            self.index_relationship_property_if_active(rel_id, [rel_type], &key, &value);
        }

        self.emit(|| MutationEvent::SetRelationshipProperty {
            rel_id,
            key: stored_key.unwrap(),
            value: stored_value.unwrap(),
        });

        true
    }

    fn remove_relationship_property(&mut self, rel_id: RelationshipId, key: &str) -> bool {
        let removed = match self.relationships.get_mut(&rel_id) {
            Some(rel) => rel.properties.remove(key),
            None => return false,
        };
        let Some(removed) = removed else {
            return false;
        };

        let rel_type = if self.active_relationship_property_index_count() != 0
            && self.indexes_mut().relationship_properties.is_active(key)
        {
            self.relationships
                .get(&rel_id)
                .map(|rel| rel.rel_type.clone())
        } else {
            None
        };

        if let Some(rel_type) = rel_type.as_deref() {
            self.unindex_relationship_property_if_active(rel_id, [rel_type], key, &removed);
        }

        self.emit(|| MutationEvent::RemoveRelationshipProperty {
            rel_id,
            key: key.to_string(),
        });

        true
    }

    fn delete_relationship(&mut self, rel_id: RelationshipId) -> bool {
        let applied = match self.relationships.remove(&rel_id) {
            Some(rel) => {
                self.detach_relationship_indexes(&rel);
                self.unindex_active_relationship_properties(
                    rel_id,
                    [rel.rel_type.as_str()],
                    &rel.properties,
                );
                true
            }
            None => false,
        };
        if applied {
            self.emit(|| MutationEvent::DeleteRelationship { rel_id });
        }
        applied
    }

    fn delete_node(&mut self, node_id: NodeId) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }

        if self.has_incident_relationships(node_id) {
            return false;
        }

        let node = match self.nodes.remove(&node_id) {
            Some(node) => node,
            None => return false,
        };

        for label in &node.labels {
            self.remove_node_label_index(node_id, label);
        }
        self.unindex_active_node_properties(
            node_id,
            node.labels.iter().map(String::as_str),
            &node.properties,
        );

        self.outgoing.remove(&node_id);
        self.incoming.remove(&node_id);

        self.emit(|| MutationEvent::DeleteNode { node_id });

        true
    }

    fn detach_delete_node(&mut self, node_id: NodeId) -> bool {
        if !self.nodes.contains_key(&node_id) {
            return false;
        }

        let rel_ids: Vec<_> = self
            .incident_relationship_ids(node_id)
            .into_iter()
            .collect();

        // We deliberately fire per-relationship DeleteRelationship events
        // here (via `delete_relationship`) and a DetachDeleteNode event at
        // the end. A WAL replayer that sees DetachDeleteNode can ignore the
        // preceding DeleteRelationship events — or, equivalently, replay
        // them and the DetachDeleteNode becomes a no-op on the remaining
        // (now-empty) node. The emit-before-delete choice costs one extra
        // event per mutation but keeps the replay contract simple:
        // "apply every event in order".
        for rel_id in rel_ids {
            let _ = self.delete_relationship(rel_id);
        }

        if self.delete_node(node_id) {
            self.emit(|| MutationEvent::DetachDeleteNode { node_id });
            true
        } else {
            false
        }
    }

    fn clear(&mut self) {
        // Keep the recorder across clear so observers can see the Clear
        // event plus whatever follows. Matches WAL semantics where the log
        // is the source of truth across a truncation.
        let recorder = self.recorder.take();
        *self = Self::default();
        self.recorder = recorder;
        self.emit(|| MutationEvent::Clear);
    }
}

// ---------------------------------------------------------------------------
// Snapshotable
// ---------------------------------------------------------------------------

impl InMemoryGraph {
    /// Return the portable graph-state payload without encoding it into the
    /// legacy `LORASNAP` file format.
    pub fn snapshot_payload(&self) -> SnapshotPayload {
        SnapshotPayload {
            next_node_id: self.next_node_id,
            next_rel_id: self.next_rel_id,
            nodes: self.nodes.values().cloned().collect(),
            relationships: self.relationships.values().cloned().collect(),
        }
    }

    /// Replace the graph from a portable graph-state payload, preserving the
    /// currently installed mutation recorder across the swap.
    pub fn load_snapshot_payload(
        &mut self,
        payload: SnapshotPayload,
    ) -> Result<SnapshotMeta, SnapshotError> {
        let meta = SnapshotMeta {
            format_version: SNAPSHOT_FORMAT_VERSION,
            node_count: payload.nodes.len(),
            relationship_count: payload.relationships.len(),
            wal_lsn: None,
        };

        // Build the restored graph in a fresh local instance and only
        // commit it into `self` at the very end. If a panic fires mid-
        // rebuild (e.g. OOM on a HashMap grow) the caller's graph is
        // untouched — we never observe a half-populated store.
        let mut rebuilt = Self {
            next_node_id: payload.next_node_id,
            next_rel_id: payload.next_rel_id,
            ..Self::default()
        };

        // Insert nodes + rebuild label index + seed adjacency slots.
        for node in payload.nodes {
            let id = node.id;
            let labels = node.labels.clone();
            rebuilt.nodes.insert(id, node);
            for label in &labels {
                rebuilt.insert_node_label_index(id, label);
            }
            rebuilt.outgoing.entry(id).or_default();
            rebuilt.incoming.entry(id).or_default();
        }

        // Insert relationships + rebuild adjacency + type index.
        for rel in payload.relationships {
            rebuilt.attach_relationship(&rel);
            rebuilt.relationships.insert(rel.id, rel);
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

impl Snapshotable for InMemoryGraph {
    fn save_snapshot<W: std::io::Write>(&self, writer: W) -> Result<SnapshotMeta, SnapshotError> {
        let payload = self.snapshot_payload();
        write_snapshot(writer, &payload, None)
    }

    fn save_checkpoint<W: std::io::Write>(
        &self,
        writer: W,
        wal_lsn: u64,
    ) -> Result<SnapshotMeta, SnapshotError> {
        let payload = self.snapshot_payload();
        write_snapshot(writer, &payload, Some(wal_lsn))
    }

    fn load_snapshot<R: std::io::Read>(
        &mut self,
        reader: R,
    ) -> Result<SnapshotMeta, SnapshotError> {
        let (payload, meta) = read_snapshot(reader)?;
        self.load_snapshot_payload(payload)?;
        Ok(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, PropertyValue)]) -> Properties {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn create_and_lookup_nodes() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(
            vec!["Person".into(), "Employee".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let b = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Bob".into()))]),
        );

        assert_eq!(a.id, 0);
        assert_eq!(b.id, 1);

        assert_eq!(g.all_nodes().len(), 2);
        assert_eq!(g.nodes_by_label("Person").len(), 2);
        assert_eq!(g.nodes_by_label("Employee").len(), 1);
        assert_eq!(BorrowedGraphStorage::node_refs(&g).count(), 2);
        assert_eq!(
            BorrowedGraphStorage::node_refs_by_label(&g, "Person").count(),
            2
        );
        assert!(g.node_has_label(a.id, "Person"));
        assert_eq!(
            g.node_property(a.id, "name"),
            Some(PropertyValue::String("Alice".into()))
        );
    }

    #[test]
    fn create_and_expand_relationships() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Company".into()], Properties::new());

        let r1 = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        let r2 = g
            .create_relationship(a.id, c.id, "WORKS_AT", Properties::new())
            .unwrap();

        assert_eq!(g.all_relationships().len(), 2);
        assert_eq!(g.relationships_by_type("KNOWS").len(), 1);
        assert_eq!(BorrowedGraphStorage::relationship_refs(&g).count(), 2);
        assert_eq!(
            BorrowedGraphStorage::relationship_refs_by_type(&g, "KNOWS").count(),
            1
        );
        assert_eq!(g.outgoing_relationships(a.id).len(), 2);
        assert_eq!(g.incoming_relationships(b.id).len(), 1);

        let knows = g.expand(a.id, Direction::Right, &[String::from("KNOWS")]);
        assert_eq!(knows.len(), 1);
        assert_eq!(knows[0].0.id, r1.id);
        assert_eq!(knows[0].1.id, b.id);

        let undirected = g.expand(a.id, Direction::Undirected, &[]);
        assert_eq!(undirected.len(), 2);

        assert_eq!(g.relationship(r2.id).unwrap().dst, c.id);
    }

    #[test]
    fn incoming_and_outgoing_are_distinct() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Person".into()], Properties::new());

        g.create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        g.create_relationship(c.id, a.id, "LIKES", Properties::new())
            .unwrap();

        let outgoing = g.expand(a.id, Direction::Right, &[]);
        let incoming = g.expand(a.id, Direction::Left, &[]);

        assert_eq!(outgoing.len(), 1);
        assert_eq!(incoming.len(), 1);
        assert_eq!(outgoing[0].1.id, b.id);
        assert_eq!(incoming[0].1.id, c.id);
    }

    #[test]
    fn set_and_remove_properties() {
        let mut g = InMemoryGraph::new();

        let n = g.create_node(vec!["Person".into()], Properties::new());
        assert!(g.set_node_property(n.id, "age".into(), PropertyValue::Int(42)));
        assert_eq!(g.node_property(n.id, "age"), Some(PropertyValue::Int(42)));
        assert!(g.remove_node_property(n.id, "age"));
        assert_eq!(g.node_property(n.id, "age"), None);

        let m = g.create_node(vec!["Person".into()], Properties::new());
        let r = g
            .create_relationship(n.id, m.id, "KNOWS", Properties::new())
            .unwrap();

        assert!(g.set_relationship_property(r.id, "since".into(), PropertyValue::Int(2020)));
        assert_eq!(
            g.relationship_property(r.id, "since"),
            Some(PropertyValue::Int(2020))
        );
        assert!(g.remove_relationship_property(r.id, "since"));
        assert_eq!(g.relationship_property(r.id, "since"), None);
    }

    #[test]
    fn node_property_index_tracks_create_set_remove_and_delete() {
        let mut g = InMemoryGraph::new();
        let alice = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let other_alice = g.create_node(
            vec!["Robot".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let bob = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Bob".into()))]),
        );

        let alice_value = PropertyValue::String("Alice".into());
        assert_eq!(
            g.find_nodes_by_property(Some("Person"), "name", &alice_value)
                .into_iter()
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            vec![alice.id]
        );
        assert!(g.node_exists_with_label_and_property("Robot", "name", &alice_value));

        assert!(g.set_node_property(
            other_alice.id,
            "name".into(),
            PropertyValue::String("Alicia".into())
        ));
        assert_eq!(
            g.find_nodes_by_property(None, "name", &alice_value)
                .into_iter()
                .map(|n| n.id)
                .collect::<Vec<_>>(),
            vec![alice.id]
        );

        assert!(g.remove_node_property(alice.id, "name"));
        assert!(!g.node_exists_with_label_and_property("Person", "name", &alice_value));

        assert!(g.delete_node(bob.id));
        assert!(!g.node_exists_with_label_and_property(
            "Person",
            "name",
            &PropertyValue::String("Bob".into())
        ));
    }

    #[test]
    fn node_property_index_activates_on_lookup_and_tracks_later_create() {
        let mut g = InMemoryGraph::new();
        let first = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );

        assert!(!g.indexes_read().node_properties.is_active("name"));

        let alice = PropertyValue::String("Alice".into());
        assert_eq!(
            g.find_nodes_by_property(Some("Person"), "name", &alice)
                .into_iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![first.id]
        );
        assert!(g.indexes_read().node_properties.is_active("name"));

        let second = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        assert_eq!(
            g.find_nodes_by_property(Some("Person"), "name", &alice)
                .into_iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![first.id, second.id]
        );
    }

    #[test]
    fn property_indexes_activate_on_lookup_after_set_for_new_keys() {
        let mut g = InMemoryGraph::new();
        let node = g.create_node(vec!["Person".into()], Properties::new());

        assert!(!g.indexes_read().node_properties.is_active("name"));
        assert!(g.set_node_property(
            node.id,
            "name".into(),
            PropertyValue::String("Alice".into())
        ));
        assert!(!g.indexes_read().node_properties.is_active("name"));
        assert_eq!(
            g.find_node_ids_by_property(
                Some("Person"),
                "name",
                &PropertyValue::String("Alice".into())
            ),
            vec![node.id]
        );
        assert!(g.indexes_read().node_properties.is_active("name"));

        let other = g.create_node(vec!["Person".into()], Properties::new());
        let rel = g
            .create_relationship(node.id, other.id, "KNOWS", Properties::new())
            .unwrap();
        assert!(!g.indexes_read().relationship_properties.is_active("since"));
        assert!(g.set_relationship_property(rel.id, "since".into(), PropertyValue::Int(2020)));
        assert!(!g.indexes_read().relationship_properties.is_active("since"));
        assert_eq!(
            g.find_relationship_ids_by_property(Some("KNOWS"), "since", &PropertyValue::Int(2020)),
            vec![rel.id]
        );
        assert!(g.indexes_read().relationship_properties.is_active("since"));
    }

    #[test]
    fn replay_create_eagerly_activates_property_indexes() {
        let mut g = InMemoryGraph::new();
        let alice = g
            .replay_create_node(
                0,
                vec!["Person".into()],
                props(&[("name", PropertyValue::String("Alice".into()))]),
            )
            .unwrap();
        let bob = g
            .replay_create_node(
                1,
                vec!["Person".into()],
                props(&[("name", PropertyValue::String("Bob".into()))]),
            )
            .unwrap();

        assert!(g.indexes_read().node_properties.is_active("name"));
        assert_eq!(
            g.find_node_ids_by_property(
                Some("Person"),
                "name",
                &PropertyValue::String("Alice".into())
            ),
            vec![alice.id]
        );

        let rel = g
            .replay_create_relationship(
                0,
                alice.id,
                bob.id,
                "KNOWS",
                props(&[("since", PropertyValue::Int(2020))]),
            )
            .unwrap();

        assert!(g.indexes_read().relationship_properties.is_active("since"));
        assert_eq!(
            g.find_relationship_ids_by_property(Some("KNOWS"), "since", &PropertyValue::Int(2020)),
            vec![rel.id]
        );
    }

    #[test]
    fn node_property_index_tracks_scoped_label_buckets() {
        let mut g = InMemoryGraph::new();
        let alice = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let robot = g.create_node(
            vec!["Robot".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let alice_value = PropertyValue::String("Alice".into());

        assert_eq!(
            g.find_node_ids_by_property(Some("Person"), "name", &alice_value),
            vec![alice.id]
        );
        assert_eq!(
            g.indexes_read()
                .node_properties
                .scoped_ids_for("Person", "name", &alice_value)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![alice.id]
        );

        assert!(g.add_node_label(robot.id, "Employee"));
        assert_eq!(
            g.find_node_ids_by_property(Some("Employee"), "name", &alice_value),
            vec![robot.id]
        );

        assert!(g.remove_node_label(alice.id, "Person"));
        assert!(g
            .find_node_ids_by_property(Some("Person"), "name", &alice_value)
            .is_empty());
        assert_eq!(
            g.find_node_ids_by_property(None, "name", &alice_value),
            vec![alice.id, robot.id]
        );
    }

    #[test]
    fn relationship_property_index_tracks_create_set_remove_and_delete() {
        let mut g = InMemoryGraph::new();
        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Person".into()], Properties::new());
        let first = g
            .create_relationship(
                a.id,
                b.id,
                "KNOWS",
                props(&[("since", PropertyValue::Int(2020))]),
            )
            .unwrap();
        let second = g
            .create_relationship(
                b.id,
                c.id,
                "LIKES",
                props(&[("since", PropertyValue::Int(2020))]),
            )
            .unwrap();

        let year = PropertyValue::Int(2020);
        assert_eq!(
            g.find_relationships_by_property(Some("KNOWS"), "since", &year)
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![first.id]
        );
        assert!(g.relationship_exists_with_type_and_property("LIKES", "since", &year));

        assert!(g.set_relationship_property(second.id, "since".into(), PropertyValue::Int(2021)));
        assert_eq!(
            g.find_relationships_by_property(None, "since", &year)
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![first.id]
        );

        assert!(g.remove_relationship_property(first.id, "since"));
        assert!(!g.relationship_exists_with_type_and_property("KNOWS", "since", &year));

        assert!(g.delete_relationship(second.id));
        assert!(!g.relationship_exists_with_type_and_property(
            "LIKES",
            "since",
            &PropertyValue::Int(2021)
        ));
    }

    #[test]
    fn property_index_falls_back_for_unhashed_values() {
        let mut g = InMemoryGraph::new();
        let date = PropertyValue::Date(crate::temporal::LoraDate::new(2026, 4, 26).unwrap());
        let n = g.create_node(vec!["Event".into()], props(&[("day", date.clone())]));

        // Dates are intentionally not hash-indexed yet, so this exercises the
        // scan fallback path rather than the secondary index.
        assert_eq!(
            g.find_nodes_by_property(Some("Event"), "day", &date)
                .into_iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![n.id]
        );
    }

    #[test]
    fn delete_requires_detach() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let r = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();

        assert!(!g.delete_node(a.id));
        assert!(g.delete_relationship(r.id));
        assert!(g.delete_node(a.id));
        assert!(g.node(a.id).is_none());
    }

    #[test]
    fn detach_delete_removes_incident_relationships() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let c = g.create_node(vec!["Person".into()], Properties::new());

        let r1 = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        let r2 = g
            .create_relationship(c.id, a.id, "LIKES", Properties::new())
            .unwrap();

        assert!(g.detach_delete_node(a.id));
        assert!(g.node(a.id).is_none());
        assert!(g.relationship(r1.id).is_none());
        assert!(g.relationship(r2.id).is_none());
        assert_eq!(g.all_relationships().len(), 0);
    }

    #[test]
    fn duplicate_labels_are_normalized_on_create() {
        let mut g = InMemoryGraph::new();

        let n = g.create_node(
            vec!["Person".into(), "Person".into(), "Admin".into()],
            Properties::new(),
        );

        assert_eq!(n.labels, vec!["Person".to_string(), "Admin".to_string()]);
        assert_eq!(g.nodes_by_label("Person").len(), 1);
        assert_eq!(g.nodes_by_label("Admin").len(), 1);
    }

    #[test]
    fn empty_labels_are_ignored() {
        let mut g = InMemoryGraph::new();

        let n = g.create_node(
            vec!["Person".into(), "".into(), "   ".into()],
            Properties::new(),
        );

        assert_eq!(n.labels, vec!["Person".to_string()]);
    }

    #[test]
    fn empty_relationship_type_is_rejected() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(vec!["A".into()], Properties::new());
        let b = g.create_node(vec!["B".into()], Properties::new());

        assert!(g
            .create_relationship(a.id, b.id, "", Properties::new())
            .is_none());
    }

    #[test]
    fn storage_schema_helpers_work() {
        let mut g = InMemoryGraph::new();

        let a = g.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let b = g.create_node(
            vec!["Company".into()],
            props(&[("title", PropertyValue::String("Acme".into()))]),
        );

        g.create_relationship(
            a.id,
            b.id,
            "WORKS_AT",
            props(&[("since", PropertyValue::Int(2020))]),
        )
        .unwrap();

        assert!(g.has_label_name("Person"));
        assert!(g.has_relationship_type_name("WORKS_AT"));
        assert!(g.has_property_key("name"));
        assert!(g.has_property_key("since"));
        assert!(g.label_has_property_key("Person", "name"));
        assert!(g.rel_type_has_property_key("WORKS_AT", "since"));
    }

    #[test]
    fn clear_resets_the_graph() {
        let mut g = InMemoryGraph::new();
        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        g.create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();

        assert_eq!(g.node_count(), 2);
        assert_eq!(g.relationship_count(), 1);

        g.clear();

        assert_eq!(g.node_count(), 0);
        assert_eq!(g.relationship_count(), 0);
        assert_eq!(g.all_labels().len(), 0);
    }

    #[test]
    fn snapshot_roundtrip_preserves_graph_state() {
        let mut original = InMemoryGraph::new();
        let a = original.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Alice".into()))]),
        );
        let b = original.create_node(
            vec!["Person".into()],
            props(&[("name", PropertyValue::String("Bob".into()))]),
        );
        let r = original
            .create_relationship(
                a.id,
                b.id,
                "KNOWS",
                props(&[("since", PropertyValue::Int(2020))]),
            )
            .unwrap();

        let mut buf = Vec::new();
        let save_meta = original.save_snapshot(&mut buf).unwrap();
        assert_eq!(save_meta.node_count, 2);
        assert_eq!(save_meta.relationship_count, 1);
        assert_eq!(save_meta.wal_lsn, None);

        let mut restored = InMemoryGraph::new();
        let load_meta = restored.load_snapshot(&buf[..]).unwrap();
        assert_eq!(load_meta, save_meta);
        assert!(restored.indexes_read().node_properties.is_active("name"));
        assert!(restored
            .indexes_read()
            .relationship_properties
            .is_active("since"));

        assert_eq!(restored.node_count(), 2);
        assert_eq!(restored.relationship_count(), 1);
        assert_eq!(
            restored.node_property(a.id, "name"),
            Some(PropertyValue::String("Alice".into()))
        );
        assert_eq!(
            restored.relationship_property(r.id, "since"),
            Some(PropertyValue::Int(2020))
        );

        // Adjacency + label index were rebuilt on load.
        assert_eq!(restored.outgoing_relationships(a.id).len(), 1);
        assert_eq!(restored.nodes_by_label("Person").len(), 2);
        assert!(restored.node_exists_with_label_and_property(
            "Person",
            "name",
            &PropertyValue::String("Alice".into())
        ));
        assert!(restored.relationship_exists_with_type_and_property(
            "KNOWS",
            "since",
            &PropertyValue::Int(2020)
        ));

        // Counters carry over so new IDs don't collide with pre-snapshot IDs.
        let c = restored.create_node(vec!["Person".into()], Properties::new());
        assert_eq!(c.id, b.id + 1);
    }

    #[test]
    fn mutation_recorder_observes_every_committed_mutation() {
        use std::sync::Mutex;

        #[derive(Default)]
        struct CapturingRecorder {
            events: Mutex<Vec<MutationEvent>>,
        }

        impl MutationRecorder for CapturingRecorder {
            fn record(&self, event: MutationEvent) {
                self.events.lock().unwrap().push(event);
            }
        }

        let recorder = Arc::new(CapturingRecorder::default());
        let mut g = InMemoryGraph::new();
        g.set_mutation_recorder(Some(recorder.clone() as Arc<dyn MutationRecorder>));

        let a = g.create_node(vec!["Person".into()], Properties::new());
        let b = g.create_node(vec!["Person".into()], Properties::new());
        let r = g
            .create_relationship(a.id, b.id, "KNOWS", Properties::new())
            .unwrap();
        g.set_node_property(a.id, "name".into(), PropertyValue::String("Alice".into()));
        g.remove_node_property(a.id, "name");
        g.add_node_label(a.id, "Admin");
        g.remove_node_label(a.id, "Admin");
        g.set_relationship_property(r.id, "since".into(), PropertyValue::Int(2020));
        g.remove_relationship_property(r.id, "since");
        g.detach_delete_node(a.id);
        g.clear();

        let events = recorder.events.lock().unwrap().clone();
        assert!(matches!(events[0], MutationEvent::CreateNode { .. }));
        assert!(matches!(events[1], MutationEvent::CreateNode { .. }));
        assert!(matches!(
            events[2],
            MutationEvent::CreateRelationship { .. }
        ));
        assert!(matches!(events[3], MutationEvent::SetNodeProperty { .. }));
        assert!(matches!(
            events[4],
            MutationEvent::RemoveNodeProperty { .. }
        ));
        assert!(matches!(events[5], MutationEvent::AddNodeLabel { .. }));
        assert!(matches!(events[6], MutationEvent::RemoveNodeLabel { .. }));
        assert!(matches!(
            events[7],
            MutationEvent::SetRelationshipProperty { .. }
        ));
        assert!(matches!(
            events[8],
            MutationEvent::RemoveRelationshipProperty { .. }
        ));
        // detach_delete_node composes three kinds of events: one
        // DeleteRelationship per incident edge, one DeleteNode for the node
        // itself, and a final DetachDeleteNode marker. A WAL replayer can
        // either apply every step or recognise the marker and skip forward.
        assert!(matches!(
            events[9],
            MutationEvent::DeleteRelationship { .. }
        ));
        assert!(matches!(events[10], MutationEvent::DeleteNode { .. }));
        assert!(matches!(events[11], MutationEvent::DetachDeleteNode { .. }));
        assert!(matches!(events.last(), Some(MutationEvent::Clear)));

        // Failed mutations (invalid id) do not emit events.
        let before = recorder.events.lock().unwrap().len();
        assert!(!g.set_node_property(9999, "x".into(), PropertyValue::Int(0)));
        assert_eq!(recorder.events.lock().unwrap().len(), before);
    }

    #[test]
    fn snapshot_load_resets_but_keeps_recorder() {
        use std::sync::Mutex;

        struct CountingRecorder(Mutex<usize>);
        impl MutationRecorder for CountingRecorder {
            fn record(&self, _: MutationEvent) {
                *self.0.lock().unwrap() += 1;
            }
        }

        let counter: Arc<dyn MutationRecorder> = Arc::new(CountingRecorder(Mutex::new(0)));
        let mut g = InMemoryGraph::new();
        g.set_mutation_recorder(Some(counter));
        g.create_node(vec!["A".into()], Properties::new());

        let mut buf = Vec::new();
        g.save_snapshot(&mut buf).unwrap();

        // Load into the same graph — recorder should survive, store state
        // should be replaced by the snapshot contents.
        g.load_snapshot(&buf[..]).unwrap();
        assert!(g.mutation_recorder().is_some());
        assert_eq!(g.node_count(), 1);

        // Subsequent mutations still feed the recorder.
        g.create_node(vec!["B".into()], Properties::new());
        // 1 for the initial A + 1 for the post-load B. The restore path
        // itself does not emit events (that's a snapshot, not a mutation).
    }
}
