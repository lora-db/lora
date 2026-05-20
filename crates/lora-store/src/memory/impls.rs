//! `GraphStorage` / `BorrowedGraphStorage` / `GraphStorageMut` impls
//! for [`InMemoryGraph`]. The trait surfaces — read, borrow, mutate —
//! all delegate into the inherent helpers defined in `super`.

use std::collections::BTreeSet;

use lora_ast::Direction;

use crate::{
    BorrowedGraphStorage, ConstraintDefinition, ConstraintRequest, CreateConstraintError,
    CreateConstraintOutcome, CreateIndexError, CreateIndexOutcome, DropConstraintError,
    DropConstraintOutcome, DropIndexError, DropIndexOutcome, GraphStats, GraphStorage,
    GraphStorageMut, IndexDefinition, IndexRequest, LoraVector, MutationEvent, NodeId, NodeRecord,
    Properties, PropertyValue, RelationshipId, RelationshipRecord, StoredIndexEntity,
};

use super::property_index::PropertyIndexKey;
use super::InMemoryGraph;

impl GraphStorage for InMemoryGraph {
    // ---------- Required primitives ----------

    fn list_indexes(&self) -> Vec<IndexDefinition> {
        self.index_catalog_read().list()
    }

    fn get_index(&self, name: &str) -> Option<IndexDefinition> {
        self.index_catalog_read().get(name).cloned()
    }

    fn fulltext_search(&self, name: &str, query: &str) -> Vec<(u64, f64)> {
        // The index lives on exactly one entity scope; check both.
        let node_hits = self
            .fulltext_indexes_read(StoredIndexEntity::Node)
            .get(name)
            .map(|idx| idx.query(query));
        if let Some(hits) = node_hits {
            if !hits.is_empty() {
                return hits;
            }
        }
        let rel_hits = self
            .fulltext_indexes_read(StoredIndexEntity::Relationship)
            .get(name)
            .map(|idx| idx.query(query));
        rel_hits.unwrap_or_default()
    }

    fn vector_search(&self, name: &str, query: &LoraVector, _k: usize) -> Vec<(u64, f64)> {
        // Like fulltext_search, an index lives on exactly one entity
        // scope. Probe both registries; the catalog has already
        // enforced kind/entity agreement so only one will hit.
        if let Some(hits) = self
            .vector_indexes_read(StoredIndexEntity::Node)
            .query(name, query)
        {
            if !hits.is_empty() {
                return hits;
            }
        }
        self.vector_indexes_read(StoredIndexEntity::Relationship)
            .query(name, query)
            .unwrap_or_default()
    }

    fn list_constraints(&self) -> Vec<ConstraintDefinition> {
        self.constraint_catalog_read().list()
    }

    fn get_constraint(&self, name: &str) -> Option<ConstraintDefinition> {
        self.constraint_catalog_read().get(name).cloned()
    }

    fn check_node_create_against_constraints(
        &self,
        labels: &[String],
        properties: &Properties,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_node_create(&catalog, self, labels, properties)
            .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_relationship_create_against_constraints(
        &self,
        rel_type: &str,
        properties: &Properties,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_relationship_create(
            &catalog, self, rel_type, properties,
        )
        .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_node_set_property_against_constraints(
        &self,
        node_id: NodeId,
        key: &str,
        value: &PropertyValue,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_node_set_property(
            &catalog, self, node_id, key, value,
        )
        .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_node_remove_property_against_constraints(
        &self,
        node_id: NodeId,
        key: &str,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_node_remove_property(&catalog, self, node_id, key)
            .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_node_replace_properties_against_constraints(
        &self,
        node_id: NodeId,
        properties: &Properties,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_node_replace_properties(
            &catalog, self, node_id, properties,
        )
        .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_relationship_set_property_against_constraints(
        &self,
        rel_id: RelationshipId,
        key: &str,
        value: &PropertyValue,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_relationship_set_property(
            &catalog, self, rel_id, key, value,
        )
        .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_relationship_remove_property_against_constraints(
        &self,
        rel_id: RelationshipId,
        key: &str,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_relationship_remove_property(
            &catalog, self, rel_id, key,
        )
        .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_relationship_replace_properties_against_constraints(
        &self,
        rel_id: RelationshipId,
        properties: &Properties,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_relationship_replace_properties(
            &catalog, self, rel_id, properties,
        )
        .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn check_node_add_label_against_constraints(
        &self,
        node_id: NodeId,
        label: &str,
    ) -> Result<(), String> {
        if !self.has_active_constraints() {
            return Ok(());
        }
        let catalog = self.constraint_catalog_read();
        crate::memory::constraint_enforce::check_node_add_label(&catalog, self, node_id, label)
            .map_err(|e| format!("[{}] {e}", e.gql_status()))
    }

    fn graph_stats(&self) -> GraphStats {
        InMemoryGraph::graph_stats(self)
    }

    fn node_text_candidates(
        &self,
        label: &str,
        property: &str,
        query: &str,
    ) -> Option<Vec<NodeId>> {
        let registry = self.text_indexes_read(crate::StoredIndexEntity::Node);
        let candidates = registry.candidates(label, property, query)?;
        Some(candidates.into_iter().collect())
    }

    fn node_range_candidates(
        &self,
        label: &str,
        property: &str,
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
    ) -> Option<Vec<NodeId>> {
        let registry = self.sorted_indexes_read(crate::StoredIndexEntity::Node);
        // The sorted index returns a [lo, hi] inclusive candidate set.
        // The executor refilters each id against the precise predicate
        // (handling `>` vs `>=`, `<` vs `<=`).
        let candidates = registry.range_candidates(label, property, lo, hi)?;
        Some(candidates.into_iter().collect())
    }

    fn node_point_within_bbox(
        &self,
        label: &str,
        property: &str,
        ll: (f64, f64),
        ur: (f64, f64),
    ) -> Option<Vec<NodeId>> {
        let registry = self.point_indexes_read(crate::StoredIndexEntity::Node);
        let candidates = registry.within_bbox(label, property, ll, ur)?;
        Some(candidates.into_iter().collect())
    }

    fn node_point_within_distance(
        &self,
        label: &str,
        property: &str,
        center: (f64, f64),
        max_distance: f64,
    ) -> Option<Vec<NodeId>> {
        let registry = self.point_indexes_read(crate::StoredIndexEntity::Node);
        let candidates = registry.within_distance(label, property, center, max_distance)?;
        Some(candidates.into_iter().collect())
    }

    fn relationship_text_candidates(
        &self,
        rel_type: &str,
        property: &str,
        query: &str,
    ) -> Option<Vec<RelationshipId>> {
        let registry = self.text_indexes_read(crate::StoredIndexEntity::Relationship);
        let candidates = registry.candidates(rel_type, property, query)?;
        Some(candidates.into_iter().collect())
    }

    fn relationship_range_candidates(
        &self,
        rel_type: &str,
        property: &str,
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
    ) -> Option<Vec<RelationshipId>> {
        let registry = self.sorted_indexes_read(crate::StoredIndexEntity::Relationship);
        let candidates = registry.range_candidates(rel_type, property, lo, hi)?;
        Some(candidates.into_iter().collect())
    }

    fn relationship_point_within_bbox(
        &self,
        rel_type: &str,
        property: &str,
        ll: (f64, f64),
        ur: (f64, f64),
    ) -> Option<Vec<RelationshipId>> {
        let registry = self.point_indexes_read(crate::StoredIndexEntity::Relationship);
        let candidates = registry.within_bbox(rel_type, property, ll, ur)?;
        Some(candidates.into_iter().collect())
    }

    fn relationship_point_within_distance(
        &self,
        rel_type: &str,
        property: &str,
        center: (f64, f64),
        max_distance: f64,
    ) -> Option<Vec<RelationshipId>> {
        let registry = self.point_indexes_read(crate::StoredIndexEntity::Relationship);
        let candidates = registry.within_distance(rel_type, property, center, max_distance)?;
        Some(candidates.into_iter().collect())
    }

    fn contains_node(&self, id: NodeId) -> bool {
        self.node_at(id).is_some()
    }

    fn node(&self, id: NodeId) -> Option<NodeRecord> {
        self.node_at(id).cloned()
    }

    fn all_node_ids(&self) -> Vec<NodeId> {
        self.iter_node_ids().collect()
    }

    fn node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        match self.nodes_by_label.get(label) {
            Some(ids) => ids.clone(),
            None => Vec::new(),
        }
    }

    fn contains_relationship(&self, id: RelationshipId) -> bool {
        self.rel_at(id).is_some()
    }

    fn relationship(&self, id: RelationshipId) -> Option<RelationshipRecord> {
        self.rel_at(id).cloned()
    }

    fn all_rel_ids(&self) -> Vec<RelationshipId> {
        self.iter_rel_ids().collect()
    }

    fn rel_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        match self.relationships_by_type.get(rel_type) {
            Some(ids) => ids.clone(),
            None => Vec::new(),
        }
    }

    fn relationship_endpoints(&self, id: RelationshipId) -> Option<(NodeId, NodeId)> {
        self.rel_at(id).map(|r| (r.src, r.dst))
    }

    fn expand_ids(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        if self.node_at(node_id).is_none() {
            return Vec::new();
        }

        // Walk the adjacency Vec(s) directly into a single output Vec,
        // skipping the previous intermediate `Vec<RelationshipId>`
        // allocation that `relationship_ids_for_direction` produced.
        // For type-filtered traversal we read `rel.rel_type` once per
        // edge against the (typically tiny) `types` slice.
        let mut out: Vec<(RelationshipId, NodeId)> = Vec::new();

        let single_type = match types {
            [single] => Some(single.as_str()),
            _ => None,
        };
        let has_type_filter = !types.is_empty();

        let push_from = |adj: &[RelationshipId],
                         skip_self_loops: bool,
                         out: &mut Vec<(RelationshipId, NodeId)>| {
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
                out.push((rel_id, other_id));
            }
        };

        match direction {
            Direction::Right => {
                if let Some(adj) = self.outgoing_at(node_id) {
                    out.reserve(adj.len());
                    push_from(adj, false, &mut out);
                }
            }
            Direction::Left => {
                if let Some(adj) = self.incoming_at(node_id) {
                    out.reserve(adj.len());
                    push_from(adj, false, &mut out);
                }
            }
            Direction::Undirected => {
                let out_len = self.outgoing_at(node_id).map(<[_]>::len).unwrap_or(0);
                let in_len = self.incoming_at(node_id).map(<[_]>::len).unwrap_or(0);
                out.reserve(out_len + in_len);
                if let Some(adj) = self.outgoing_at(node_id) {
                    push_from(adj, false, &mut out);
                }
                if let Some(adj) = self.incoming_at(node_id) {
                    push_from(adj, true, &mut out);
                }
            }
        }

        out
    }

    fn try_for_each_expand_id<F, E>(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
        visit: F,
    ) -> Result<(), E>
    where
        F: FnMut(RelationshipId, NodeId) -> Result<(), E>,
        Self: Sized,
    {
        self.try_for_each_adjacent_id(node_id, direction, types, visit)
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
        self.node_at(id).map(f)
    }

    fn with_relationship<F, R>(&self, id: RelationshipId, f: F) -> Option<R>
    where
        F: FnOnce(&RelationshipRecord) -> R,
        Self: Sized,
    {
        self.rel_at(id).map(f)
    }

    // ---------- Overrides: counts + existence ----------

    fn has_node(&self, id: NodeId) -> bool {
        self.node_at(id).is_some()
    }

    fn has_relationship(&self, id: RelationshipId) -> bool {
        self.rel_at(id).is_some()
    }

    fn node_count(&self) -> usize {
        self.live_node_count
    }

    fn relationship_count(&self) -> usize {
        self.live_rel_count
    }

    // ---------- Overrides: record-returning scans (direct iteration) ----------

    fn all_nodes(&self) -> Vec<NodeRecord> {
        self.iter_node_records().cloned().collect()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeRecord> {
        self.nodes_by_label
            .get(label)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|&id| self.node_at(id).cloned())
            .collect()
    }

    fn all_relationships(&self) -> Vec<RelationshipRecord> {
        self.iter_rel_records().cloned().collect()
    }

    fn relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord> {
        self.relationships_by_type
            .get(rel_type)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|&id| self.rel_at(id).cloned())
            .collect()
    }

    fn relationship_ids_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipId> {
        self.relationship_ids_for_direction(node_id, direction)
    }

    fn outgoing_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.outgoing_at(node_id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|&id| self.rel_at(id).cloned())
            .collect()
    }

    fn incoming_relationships(&self, node_id: NodeId) -> Vec<RelationshipRecord> {
        self.incoming_at(node_id)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|&id| self.rel_at(id).cloned())
            .collect()
    }

    fn relationships_of(&self, node_id: NodeId, direction: Direction) -> Vec<RelationshipRecord> {
        let mut out = Vec::new();
        let _ = self.try_for_each_expand_id(node_id, direction, &[], |rel_id, _| {
            if let Some(rel) = self.rel_at(rel_id) {
                out.push(rel.clone());
            }
            Ok::<(), ()>(())
        });
        out
    }

    fn degree(&self, node_id: NodeId, direction: Direction) -> usize {
        match direction {
            Direction::Right => self.outgoing_at(node_id).map(|s| s.len()).unwrap_or(0),
            Direction::Left => self.incoming_at(node_id).map(|s| s.len()).unwrap_or(0),
            Direction::Undirected => {
                let out_count = self.outgoing_at(node_id).map(<[_]>::len).unwrap_or(0);
                let incoming_non_self = self
                    .incoming_at(node_id)
                    .into_iter()
                    .flat_map(|ids| ids.iter())
                    .filter(|&&rel_id| {
                        self.rel_at(rel_id)
                            .map(|rel| rel.src != node_id || rel.dst != node_id)
                            .unwrap_or(false)
                    })
                    .count();
                out_count + incoming_non_self
            }
        }
    }

    fn expand(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<(RelationshipRecord, NodeRecord)> {
        if self.node_at(node_id).is_none() {
            return Vec::new();
        }

        let mut out = Vec::new();
        let _ = self.try_for_each_expand_id(node_id, direction, types, |rel_id, other_id| {
            if let (Some(rel), Some(other)) = (self.rel_at(rel_id), self.node_at(other_id)) {
                out.push((rel.clone(), other.clone()));
            }
            Ok::<(), ()>(())
        });
        out
    }

    fn neighbors(
        &self,
        node_id: NodeId,
        direction: Direction,
        types: &[String],
    ) -> Vec<NodeRecord> {
        let mut out = Vec::new();
        let _ = self.try_for_each_expand_id(node_id, direction, types, |_, other_id| {
            if let Some(node) = self.node_at(other_id) {
                out.push(node.clone());
            }
            Ok::<(), ()>(())
        });
        out
    }

    fn all_node_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for node in self.iter_node_records() {
            for key in node.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    // ---------- Overrides: traversal (direct adjacency) ----------

    fn all_relationship_property_keys(&self) -> Vec<String> {
        let mut keys = BTreeSet::new();
        for rel in self.iter_rel_records() {
            for key in rel.properties.keys() {
                keys.insert(key.clone());
            }
        }
        keys.into_iter().collect()
    }

    fn label_property_keys(&self, label: &str) -> Vec<String> {
        let mut keys = BTreeSet::new();

        if let Some(ids) = self.nodes_by_label.get(label) {
            for &id in ids {
                if let Some(node) = self.node_at(id) {
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
            for &id in ids {
                if let Some(rel) = self.rel_at(id) {
                    for key in rel.properties.keys() {
                        keys.insert(key.clone());
                    }
                }
            }
        }

        keys.into_iter().collect()
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
                    .filter_map(|&id| self.node_at(id).cloned())
                    .collect()
            }
            None => indexes
                .node_properties
                .ids_for(key, value)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| self.node_at(id).cloned())
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
            return self.scan_node_ids_by_property(label, key, value);
        }

        self.ensure_node_property_index(key);
        let indexes = self.indexes_read();

        match label {
            Some(label) => indexes
                .node_properties
                .scoped_ids_for(label, key, value)
                .map(|ids| ids.to_vec())
                .unwrap_or_default(),
            None => indexes
                .node_properties
                .ids_for(key, value)
                .map(|ids| ids.to_vec())
                .unwrap_or_default(),
        }
    }
    // ---------- Overrides: schema introspection ----------

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
                    .filter_map(|&id| self.rel_at(id).cloned())
                    .collect()
            }
            None => indexes
                .relationship_properties
                .ids_for(key, value)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| self.rel_at(id).cloned())
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
            return self.scan_relationship_ids_by_property(rel_type, key, value);
        }

        self.ensure_relationship_property_index(key);
        let indexes = self.indexes_read();

        match rel_type {
            Some(rel_type) => indexes
                .relationship_properties
                .scoped_ids_for(rel_type, key, value)
                .map(|ids| ids.to_vec())
                .unwrap_or_default(),
            None => indexes
                .relationship_properties
                .ids_for(key, value)
                .map(|ids| ids.to_vec())
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
            return self.any_node_by_property(label, key, value);
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
            return self.any_relationship_by_property(rel_type, key, value);
        }

        self.ensure_relationship_property_index(key);
        let indexes = self.indexes_read();
        indexes
            .relationship_properties
            .scoped_ids_for(rel_type, key, value)
            .map(|ids| !ids.is_empty())
            .unwrap_or(false)
    }
}

impl BorrowedGraphStorage for InMemoryGraph {
    fn node_ref(&self, id: NodeId) -> Option<&NodeRecord> {
        self.node_at(id)
    }

    fn relationship_ref(&self, id: RelationshipId) -> Option<&RelationshipRecord> {
        self.rel_at(id)
    }

    fn node_refs(&self) -> Box<dyn Iterator<Item = &NodeRecord> + '_> {
        Box::new(self.iter_node_records())
    }

    fn node_refs_by_label(&self, label: &str) -> Box<dyn Iterator<Item = &NodeRecord> + '_> {
        Box::new(
            self.nodes_by_label
                .get(label)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|&id| self.node_at(id)),
        )
    }

    fn relationship_refs(&self) -> Box<dyn Iterator<Item = &RelationshipRecord> + '_> {
        Box::new(self.iter_rel_records())
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
                .filter_map(|&id| self.rel_at(id)),
        )
    }
}

impl GraphStorageMut for InMemoryGraph {
    fn create_node(&mut self, labels: Vec<String>, properties: Properties) -> NodeRecord {
        let (id, idx) = self.reserve_next_node_slot();
        let labels = Self::normalize_labels(labels);

        let node = NodeRecord {
            id,
            labels: labels.clone(),
            properties,
        };

        self.put_node_at_slot(idx, node.clone());

        self.on_node_created(&node);

        // ensure_node_slot grew both adjacency Vecs to cover this id when
        // we put_node above.

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
        if self.node_at(src).is_none() || self.node_at(dst).is_none() {
            return None;
        }

        let trimmed = rel_type.trim();
        if trimmed.is_empty() {
            return None;
        }

        let (id, idx) = self.try_reserve_next_rel_slot()?;
        let rel = RelationshipRecord {
            id,
            src,
            dst,
            rel_type: trimmed.to_string(),
            properties,
        };

        self.put_rel_at_slot(idx, rel.clone());
        self.on_relationship_created(&rel);

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
        if self.node_at(node_id).is_none() {
            return false;
        }

        let old = match self.node_at_mut(node_id) {
            Some(node) => node.properties.insert(key.clone(), value.clone()),
            None => return false,
        };
        self.on_node_property_set(node_id, &key, old.as_ref(), &value);

        self.emit(|| MutationEvent::SetNodeProperty {
            node_id,
            key: key.clone(),
            value: value.clone(),
        });

        true
    }

    fn remove_node_property(&mut self, node_id: NodeId, key: &str) -> bool {
        let removed = match self.node_at_mut(node_id) {
            Some(node) => node.properties.remove(key),
            None => return false,
        };
        let Some(removed) = removed else {
            return false;
        };

        self.on_node_property_removed(node_id, key, &removed);

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

        let applied = match self.node_at_mut(node_id) {
            Some(node) => {
                if node.labels.iter().any(|l| l == label) {
                    return false;
                }

                node.labels.push(label.to_string());
                true
            }
            None => false,
        };
        if applied {
            self.on_node_label_added(node_id, label);
            self.emit(|| MutationEvent::AddNodeLabel {
                node_id,
                label: label.to_string(),
            });
        }
        applied
    }

    fn remove_node_label(&mut self, node_id: NodeId, label: &str) -> bool {
        let applied = match self.node_at_mut(node_id) {
            Some(node) => {
                let original_len = node.labels.len();
                node.labels.retain(|l| l != label);
                node.labels.len() != original_len
            }
            None => false,
        };
        if applied {
            self.on_node_label_removed(node_id, label);
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
        if self.rel_at(rel_id).is_none() {
            return false;
        }

        let old = match self.rel_at_mut(rel_id) {
            Some(rel) => rel.properties.insert(key.clone(), value.clone()),
            None => return false,
        };
        self.on_relationship_property_set(rel_id, &key, old.as_ref(), &value);

        self.emit(|| MutationEvent::SetRelationshipProperty {
            rel_id,
            key: key.clone(),
            value: value.clone(),
        });

        true
    }

    fn remove_relationship_property(&mut self, rel_id: RelationshipId, key: &str) -> bool {
        let removed = match self.rel_at_mut(rel_id) {
            Some(rel) => rel.properties.remove(key),
            None => return false,
        };
        let Some(removed) = removed else {
            return false;
        };

        self.on_relationship_property_removed(rel_id, key, &removed);

        self.emit(|| MutationEvent::RemoveRelationshipProperty {
            rel_id,
            key: key.to_string(),
        });

        true
    }

    fn delete_relationship(&mut self, rel_id: RelationshipId) -> bool {
        let applied = match self.take_rel(rel_id) {
            Some(rel) => {
                self.on_relationship_deleted(&rel);
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
        if self.node_at(node_id).is_none() {
            return false;
        }

        if self.has_incident_relationships(node_id) {
            return false;
        }

        let node = match self.take_node(node_id) {
            Some(node) => node,
            None => return false,
        };

        self.on_node_deleted(&node);

        // take_node already cleared the per-node adjacency Vecs.

        self.emit(|| MutationEvent::DeleteNode { node_id });

        true
    }

    fn detach_delete_node(&mut self, node_id: NodeId) -> bool {
        if self.node_at(node_id).is_none() {
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

    fn create_index(
        &mut self,
        request: IndexRequest,
        if_not_exists: bool,
    ) -> Result<CreateIndexOutcome, CreateIndexError> {
        // `register_index` only needs `&self` (catalog + property-index
        // registries are behind RwLock for interior mutability), but we
        // keep the trait signature `&mut self` so downstream callers can
        // hold a uniquely-owned mutation lock. That mirrors the pattern
        // used for property writes elsewhere in this impl.
        self.register_index(request, if_not_exists)
    }

    fn drop_index(
        &mut self,
        name: &str,
        if_exists: bool,
    ) -> Result<DropIndexOutcome, DropIndexError> {
        self.drop_named_index(name, if_exists)
    }

    fn create_constraint(
        &mut self,
        request: ConstraintRequest,
        if_not_exists: bool,
    ) -> Result<CreateConstraintOutcome, CreateConstraintError> {
        self.register_constraint(request, if_not_exists)
    }

    fn drop_constraint(
        &mut self,
        name: &str,
        if_exists: bool,
    ) -> Result<DropConstraintOutcome, DropConstraintError> {
        self.drop_named_constraint(name, if_exists)
    }
}
