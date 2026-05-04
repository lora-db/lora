//! Direct graph read + mutation surface on [`Database`].
//!
//! These `graph_*` methods are the storage-agnostic façade language
//! bindings reach for when they need a single record / list / mutation
//! without compiling a Cypher query. Reads route through
//! [`Database::with_store`] (lock-free snapshot); mutations route
//! through [`Database::with_logged_store_mut`] so the WAL stays in
//! step with the in-memory state.

use std::any::Any;
use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use lora_executor::{lora_value_to_property, LoraValue};
use lora_store::{
    GraphStorage, GraphStorageMut, NodeId, NodeRecord, RelationshipId, RelationshipRecord,
};

use crate::database::{values_to_properties, Database, GraphDirection};
use crate::error::LoraError;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    // ---------- Direct graph read surface ----------

    pub fn graph_contains_node(&self, id: NodeId) -> bool {
        self.with_store(|store| store.contains_node(id))
    }

    pub fn graph_node(&self, id: NodeId) -> Option<NodeRecord> {
        self.with_store(|store| store.node(id))
    }

    pub fn graph_all_node_ids(&self) -> Vec<NodeId> {
        self.with_store(|store| store.all_node_ids())
    }

    pub fn graph_node_ids_by_label(&self, label: &str) -> Vec<NodeId> {
        self.with_store(|store| store.node_ids_by_label(label))
    }

    pub fn graph_all_nodes(&self) -> Vec<NodeRecord> {
        self.with_store(|store| store.all_nodes())
    }

    pub fn graph_nodes_by_label(&self, label: &str) -> Vec<NodeRecord> {
        self.with_store(|store| store.nodes_by_label(label))
    }

    pub fn graph_node_has_label(&self, id: NodeId, label: &str) -> bool {
        self.with_store(|store| store.node_has_label(id, label))
    }

    pub fn graph_node_labels(&self, id: NodeId) -> Option<Vec<String>> {
        self.with_store(|store| store.node_labels(id))
    }

    pub fn graph_node_properties(&self, id: NodeId) -> Option<BTreeMap<String, LoraValue>> {
        self.with_store(|store| {
            store.node_properties(id).map(|props| {
                props
                    .into_iter()
                    .map(|(key, value)| (key, LoraValue::from(value)))
                    .collect()
            })
        })
    }

    pub fn graph_node_property(&self, id: NodeId, key: &str) -> Option<LoraValue> {
        self.with_store(|store| store.node_property(id, key).map(LoraValue::from))
    }

    pub fn graph_contains_relationship(&self, id: RelationshipId) -> bool {
        self.with_store(|store| store.contains_relationship(id))
    }

    pub fn graph_relationship(&self, id: RelationshipId) -> Option<RelationshipRecord> {
        self.with_store(|store| store.relationship(id))
    }

    pub fn graph_all_relationship_ids(&self) -> Vec<RelationshipId> {
        self.with_store(|store| store.all_rel_ids())
    }

    pub fn graph_relationship_ids_by_type(&self, rel_type: &str) -> Vec<RelationshipId> {
        self.with_store(|store| store.rel_ids_by_type(rel_type))
    }

    pub fn graph_all_relationships(&self) -> Vec<RelationshipRecord> {
        self.with_store(|store| store.all_relationships())
    }

    pub fn graph_relationships_by_type(&self, rel_type: &str) -> Vec<RelationshipRecord> {
        self.with_store(|store| store.relationships_by_type(rel_type))
    }

    pub fn graph_relationship_endpoints(&self, id: RelationshipId) -> Option<(NodeId, NodeId)> {
        self.with_store(|store| store.relationship_endpoints(id))
    }

    pub fn graph_relationship_type(&self, id: RelationshipId) -> Option<String> {
        self.with_store(|store| store.relationship_type(id))
    }

    pub fn graph_relationship_properties(
        &self,
        id: RelationshipId,
    ) -> Option<BTreeMap<String, LoraValue>> {
        self.with_store(|store| {
            store.relationship_properties(id).map(|props| {
                props
                    .into_iter()
                    .map(|(key, value)| (key, LoraValue::from(value)))
                    .collect()
            })
        })
    }

    pub fn graph_relationship_property(&self, id: RelationshipId, key: &str) -> Option<LoraValue> {
        self.with_store(|store| store.relationship_property(id, key).map(LoraValue::from))
    }

    pub fn graph_relationship_ids_of(
        &self,
        node_id: NodeId,
        direction: GraphDirection,
    ) -> Vec<RelationshipId> {
        self.with_store(|store| store.relationship_ids_of(node_id, direction.as_store_direction()))
    }

    pub fn graph_degree(&self, node_id: NodeId, direction: GraphDirection) -> usize {
        self.with_store(|store| store.degree(node_id, direction.as_store_direction()))
    }

    pub fn graph_neighbors(
        &self,
        node_id: NodeId,
        direction: GraphDirection,
        types: &[String],
    ) -> Vec<NodeRecord> {
        self.with_store(|store| store.neighbors(node_id, direction.as_store_direction(), types))
    }

    pub fn graph_expand_ids(
        &self,
        node_id: NodeId,
        direction: GraphDirection,
        types: &[String],
    ) -> Vec<(RelationshipId, NodeId)> {
        self.with_store(|store| store.expand_ids(node_id, direction.as_store_direction(), types))
    }

    pub fn graph_all_labels(&self) -> Vec<String> {
        self.with_store(|store| store.all_labels())
    }

    pub fn graph_all_relationship_types(&self) -> Vec<String> {
        self.with_store(|store| store.all_relationship_types())
    }

    pub fn graph_all_property_keys(&self) -> Vec<String> {
        self.with_store(|store| store.all_property_keys())
    }

    pub fn graph_all_node_property_keys(&self) -> Vec<String> {
        self.with_store(|store| store.all_node_property_keys())
    }

    pub fn graph_all_relationship_property_keys(&self) -> Vec<String> {
        self.with_store(|store| store.all_relationship_property_keys())
    }

    pub fn graph_label_property_keys(&self, label: &str) -> Vec<String> {
        self.with_store(|store| store.label_property_keys(label))
    }

    pub fn graph_relationship_type_property_keys(&self, rel_type: &str) -> Vec<String> {
        self.with_store(|store| store.rel_type_property_keys(rel_type))
    }

    pub fn graph_find_node_ids_by_property(
        &self,
        label: Option<&str>,
        key: &str,
        value: LoraValue,
    ) -> Result<Vec<NodeId>> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        Ok(self.with_store(|store| store.find_node_ids_by_property(label, key, &value)))
    }

    pub fn graph_find_relationship_ids_by_property(
        &self,
        rel_type: Option<&str>,
        key: &str,
        value: LoraValue,
    ) -> Result<Vec<RelationshipId>> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        Ok(self.with_store(|store| store.find_relationship_ids_by_property(rel_type, key, &value)))
    }

    // ---------- Direct graph mutation surface ----------

    pub fn graph_create_node(
        &self,
        labels: Vec<String>,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<NodeRecord, LoraError> {
        let properties = values_to_properties(properties).map_err(LoraError::from_anyhow)?;
        self.with_logged_store_mut(|store| Ok(store.create_node(labels, properties)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_create_relationship(
        &self,
        src: NodeId,
        dst: NodeId,
        rel_type: &str,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<Option<RelationshipRecord>, LoraError> {
        let properties = values_to_properties(properties).map_err(LoraError::from_anyhow)?;
        self.with_logged_store_mut(|store| {
            Ok(store.create_relationship(src, dst, rel_type, properties))
        })
        .map_err(LoraError::from_anyhow)
    }

    pub fn graph_set_node_property(
        &self,
        node_id: NodeId,
        key: String,
        value: LoraValue,
    ) -> Result<bool, LoraError> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        self.with_logged_store_mut(|store| Ok(store.set_node_property(node_id, key, value)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_remove_node_property(
        &self,
        node_id: NodeId,
        key: &str,
    ) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.remove_node_property(node_id, key)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_add_node_label(&self, node_id: NodeId, label: &str) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.add_node_label(node_id, label)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_remove_node_label(&self, node_id: NodeId, label: &str) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.remove_node_label(node_id, label)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_set_node_labels(
        &self,
        node_id: NodeId,
        labels: Vec<String>,
    ) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.set_node_labels(node_id, labels)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_replace_node_properties(
        &self,
        node_id: NodeId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool, LoraError> {
        let properties = values_to_properties(properties).map_err(LoraError::from_anyhow)?;
        self.with_logged_store_mut(|store| Ok(store.replace_node_properties(node_id, properties)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_merge_node_properties(
        &self,
        node_id: NodeId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool, LoraError> {
        let properties = values_to_properties(properties).map_err(LoraError::from_anyhow)?;
        self.with_logged_store_mut(|store| Ok(store.merge_node_properties(node_id, properties)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_set_relationship_property(
        &self,
        rel_id: RelationshipId,
        key: String,
        value: LoraValue,
    ) -> Result<bool, LoraError> {
        let value = lora_value_to_property(value).map_err(|e| anyhow!(e))?;
        self.with_logged_store_mut(|store| Ok(store.set_relationship_property(rel_id, key, value)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_remove_relationship_property(
        &self,
        rel_id: RelationshipId,
        key: &str,
    ) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.remove_relationship_property(rel_id, key)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_replace_relationship_properties(
        &self,
        rel_id: RelationshipId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool, LoraError> {
        let properties = values_to_properties(properties).map_err(LoraError::from_anyhow)?;
        self.with_logged_store_mut(|store| {
            Ok(store.replace_relationship_properties(rel_id, properties))
        })
        .map_err(LoraError::from_anyhow)
    }

    pub fn graph_merge_relationship_properties(
        &self,
        rel_id: RelationshipId,
        properties: BTreeMap<String, LoraValue>,
    ) -> Result<bool, LoraError> {
        let properties = values_to_properties(properties).map_err(LoraError::from_anyhow)?;
        self.with_logged_store_mut(|store| {
            Ok(store.merge_relationship_properties(rel_id, properties))
        })
        .map_err(LoraError::from_anyhow)
    }

    pub fn graph_delete_relationship(&self, rel_id: RelationshipId) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.delete_relationship(rel_id)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_delete_node(&self, node_id: NodeId) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.delete_node(node_id)))
            .map_err(LoraError::from_anyhow)
    }

    pub fn graph_detach_delete_node(&self, node_id: NodeId) -> Result<bool, LoraError> {
        self.with_logged_store_mut(|store| Ok(store.detach_delete_node(node_id)))
            .map_err(LoraError::from_anyhow)
    }
}
