//! Maintenance hooks for catalog-backed secondary indexes.
//!
//! The graph core owns mutation ordering and primary label/type/property
//! indexes. TEXT and sorted RANGE indexes are optional catalog-backed
//! structures, so their update logic lives here to keep `graph.rs`
//! focused on record storage and adjacency changes.

use std::collections::BTreeMap;

use crate::{NodeRecord, PropertyValue, RelationshipRecord};

use super::{InMemoryGraph, StoredIndexEntity};

use super::fulltext_index::{PropertyTermCounts, TermCounts};

type FulltextEntitySnapshot = (Vec<String>, PropertyTermCounts);

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
        // Fulltext indexes union multiple properties per entity, so they
        // need the full property map up-front (not per-key snapshots).
        // Inserts add or replace the entity's contribution; removes drop
        // it entirely.
        match mutation {
            SecondaryIndexMutation::Insert => self.fulltext_reindex_node(node),
            SecondaryIndexMutation::Remove => self.fulltext_remove_node(node.id),
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
        match mutation {
            SecondaryIndexMutation::Insert => self.fulltext_reindex_relationship(rel),
            SecondaryIndexMutation::Remove => self.fulltext_remove_relationship(rel.id),
        }
    }

    fn fulltext_reindex_node(&self, node: &NodeRecord) {
        use super::fulltext_index::term_counts_for_properties;

        let mut registry = self.fulltext_indexes_write(StoredIndexEntity::Node);
        for (_, idx) in registry.by_name_mut() {
            if !idx.covers_any_label(node.labels.iter().map(String::as_str)) {
                continue;
            }
            let counts = term_counts_for_properties(&node.properties, &idx.properties);
            idx.reindex_entity(node.id, counts);
        }
    }

    fn fulltext_remove_node(&self, node_id: u64) {
        self.fulltext_indexes_write(StoredIndexEntity::Node)
            .remove_entity_everywhere(node_id);
    }

    fn fulltext_reindex_relationship(&self, rel: &RelationshipRecord) {
        use super::fulltext_index::term_counts_for_properties;

        let mut registry = self.fulltext_indexes_write(StoredIndexEntity::Relationship);
        for (_, idx) in registry.by_name_mut() {
            if !idx.covers_any_label([rel.rel_type.as_str()]) {
                continue;
            }
            let counts = term_counts_for_properties(&rel.properties, &idx.properties);
            idx.reindex_entity(rel.id, counts);
        }
    }

    fn fulltext_remove_relationship(&self, rel_id: u64) {
        self.fulltext_indexes_write(StoredIndexEntity::Relationship)
            .remove_entity_everywhere(rel_id);
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
        self.update_point_property(entity, scopes.clone(), entity_id, key, old, new);
        if self.has_active_fulltext_indexes() {
            self.update_fulltext_property(entity, scopes, entity_id, key);
        }
    }

    /// On per-property `SET` / `REMOVE`, re-derive the entity's fulltext
    /// contribution from current slab state. The node/rel record must
    /// already be in the slab at this point (this is invoked from
    /// `on_node_property_set` / `on_relationship_property_set`, which
    /// run after the slab has the new value). Re-index `entity_id`
    /// against every FULLTEXT index whose label
    /// set intersects `scopes` and whose property set includes `key`.
    /// Unlike the per-(label, property) update used by trigram / sorted
    /// indexes, fulltext indexes union multiple properties per entity,
    /// so we re-derive the entity's full contribution from current
    /// state rather than diffing old/new.
    fn update_fulltext_property<'a>(
        &self,
        entity: StoredIndexEntity,
        scopes: impl IntoIterator<Item = &'a str>,
        entity_id: u64,
        key: &str,
    ) {
        use super::fulltext_index::{
            string_property_term_counts, term_counts_for_selected_properties,
        };

        let scopes: Vec<&str> = scopes.into_iter().collect();
        // Cheap early-out: no fulltext indexes registered for this entity
        // kind. The registry read is contended only when we actually have
        // something to maintain.
        {
            let registry = self.fulltext_indexes_read(entity);
            let mut any = false;
            for (_, idx) in registry.iter() {
                if idx.property_is_covered(key)
                    && scopes
                        .iter()
                        .any(|s| idx.labels.iter().any(|l| l.as_str() == *s))
                {
                    any = true;
                    break;
                }
            }
            if !any {
                return;
            }
        }

        // Re-derive the entity's full contribution from current state.
        // We need to look up the entity record; for nodes/rels that's
        // through the slab. If the entity is gone, drop its postings.
        let snapshot: Option<FulltextEntitySnapshot> = match entity {
            StoredIndexEntity::Node => self.node_at(entity_id).map(|node| {
                // Map property -> term counts for that property; we'll
                // pick the relevant subset per matching index below.
                (
                    node.labels.clone(),
                    string_property_term_counts(&node.properties),
                )
            }),
            StoredIndexEntity::Relationship => self.rel_at(entity_id).map(|rel| {
                (
                    vec![rel.rel_type.clone()],
                    string_property_term_counts(&rel.properties),
                )
            }),
        };

        let mut registry = self.fulltext_indexes_write(entity);
        let owned_scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        for (_, idx) in registry.by_name_mut().filter(|(_, idx)| {
            idx.property_is_covered(key)
                && owned_scopes
                    .iter()
                    .any(|s| idx.labels.iter().any(|l| l == s))
        }) {
            let new_counts = match &snapshot {
                None => BTreeMap::new(),
                Some((entity_labels, per_prop)) => {
                    // Entity might have lost labels; verify it still matches.
                    if !entity_labels
                        .iter()
                        .any(|label| idx.covers_any_label([label.as_str()]))
                    {
                        TermCounts::new()
                    } else {
                        term_counts_for_selected_properties(per_prop, &idx.properties)
                    }
                }
            };
            idx.reindex_entity(entity_id, new_counts);
        }
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
