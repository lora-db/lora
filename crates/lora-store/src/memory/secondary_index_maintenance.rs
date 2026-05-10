//! Maintenance hooks for catalog-backed secondary indexes.
//!
//! The graph core owns mutation ordering and primary label/type/property
//! indexes. TEXT and sorted RANGE indexes are optional catalog-backed
//! structures, so their update logic lives here to keep `graph.rs`
//! focused on record storage and adjacency changes.

use crate::{NodeRecord, PropertyValue, RelationshipRecord};

use super::{InMemoryGraph, StoredIndexEntity};

#[derive(Debug, Clone, Copy)]
pub(super) enum SecondaryIndexMutation {
    Insert,
    Remove,
}

impl InMemoryGraph {
    pub(super) fn maintain_node_secondary_indexes(
        &self,
        node: &NodeRecord,
        mutation: SecondaryIndexMutation,
    ) {
        for label in &node.labels {
            for (key, value) in &node.properties {
                let (old, new) = match mutation {
                    SecondaryIndexMutation::Insert => (None, Some(value)),
                    SecondaryIndexMutation::Remove => (Some(value), None),
                };
                self.update_secondary_property(
                    StoredIndexEntity::Node,
                    [label.as_str()],
                    node.id,
                    key,
                    old,
                    new,
                );
            }
        }
    }

    pub(super) fn maintain_relationship_secondary_indexes(
        &self,
        rel: &RelationshipRecord,
        mutation: SecondaryIndexMutation,
    ) {
        for (key, value) in &rel.properties {
            let (old, new) = match mutation {
                SecondaryIndexMutation::Insert => (None, Some(value)),
                SecondaryIndexMutation::Remove => (Some(value), None),
            };
            self.update_secondary_property(
                StoredIndexEntity::Relationship,
                [rel.rel_type.as_str()],
                rel.id,
                key,
                old,
                new,
            );
        }
    }

    pub(super) fn update_secondary_property<'a>(
        &self,
        entity: StoredIndexEntity,
        scopes: impl IntoIterator<Item = &'a str> + Clone,
        entity_id: u64,
        key: &str,
        old: Option<&PropertyValue>,
        new: Option<&PropertyValue>,
    ) {
        self.update_text_property(entity, scopes.clone(), entity_id, key, old, new);
        self.update_sorted_property(entity, scopes.clone(), entity_id, key, old, new);
        self.update_point_property(entity, scopes, entity_id, key, old, new);
    }

    fn update_text_property<'a>(
        &self,
        entity: StoredIndexEntity,
        scopes: impl IntoIterator<Item = &'a str>,
        entity_id: u64,
        key: &str,
        old: Option<&PropertyValue>,
        new: Option<&PropertyValue>,
    ) {
        let old = match old {
            Some(PropertyValue::String(value)) => Some(value.as_str()),
            _ => None,
        };
        let new = match new {
            Some(PropertyValue::String(value)) => Some(value.as_str()),
            _ => None,
        };
        if old.is_none() && new.is_none() {
            return;
        }

        let mut registry = self.text_indexes_write(entity);
        for scope in scopes {
            registry.update(scope, key, entity_id, old, new);
        }
    }

    fn update_sorted_property<'a>(
        &self,
        entity: StoredIndexEntity,
        scopes: impl IntoIterator<Item = &'a str>,
        entity_id: u64,
        key: &str,
        old: Option<&PropertyValue>,
        new: Option<&PropertyValue>,
    ) {
        if old.is_none() && new.is_none() {
            return;
        }

        let mut registry = self.sorted_indexes_write(entity);
        for scope in scopes {
            registry.update(scope, key, entity_id, old, new);
        }
    }

    fn update_point_property<'a>(
        &self,
        entity: StoredIndexEntity,
        scopes: impl IntoIterator<Item = &'a str>,
        entity_id: u64,
        key: &str,
        old: Option<&PropertyValue>,
        new: Option<&PropertyValue>,
    ) {
        let old_pt = match old {
            Some(PropertyValue::Point(p)) => Some(p),
            _ => None,
        };
        let new_pt = match new {
            Some(PropertyValue::Point(p)) => Some(p),
            _ => None,
        };
        if old_pt.is_none() && new_pt.is_none() {
            return;
        }

        let mut registry = self.point_indexes_write(entity);
        for scope in scopes {
            registry.update(scope, key, entity_id, old_pt, new_pt);
        }
    }
}
