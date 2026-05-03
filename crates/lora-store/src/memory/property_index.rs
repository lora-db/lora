//! Hash-bucket property indexes used by [`InMemoryGraph`] for
//! `find_*_by_property` lookups.
//!
//! Two registries live on the graph (one for nodes, one for
//! relationships); each registry owns a flat property→value→ids map
//! plus a parallel scope-keyed map for label/type filtered lookups.
//!
//! Activation is lazy: a property is *activated* the first time a
//! lookup asks for it. Subsequent lookups read from the index;
//! mutations to active keys are mirrored into it. Inactive keys still
//! work via the scan fallback in `super::scan_*`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::types::PropertyValue;
use crate::LoraBinary;

pub(super) type PropertyValueBuckets = HashMap<PropertyIndexKey, BTreeSet<u64>>;
pub(super) type PropertyIndex = HashMap<String, PropertyValueBuckets>;
pub(super) type ScopedPropertyIndex = HashMap<String, PropertyIndex>;

/// Pair of [`PropertyIndexState`] registries: one for node properties,
/// one for relationship properties. Lives behind an `RwLock` on the
/// graph so cold lookups can take a read guard while activate-on-write
/// paths take a write guard briefly.
#[derive(Default)]
pub(super) struct PropertyIndexRegistry {
    pub(super) node_properties: PropertyIndexState,
    pub(super) relationship_properties: PropertyIndexState,
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

/// Per-namespace property index — flat values plus a scope-keyed
/// (label / rel-type) variant for filtered lookups.
#[derive(Debug, Default, Clone)]
pub(super) struct PropertyIndexState {
    pub(super) active_keys: BTreeSet<String>,
    pub(super) values: PropertyIndex,
    pub(super) scoped_values: ScopedPropertyIndex,
}

impl PropertyIndexState {
    pub(super) fn is_active(&self, key: &str) -> bool {
        self.active_keys.contains(key)
    }

    pub(super) fn activate(&mut self, key: &str) -> bool {
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

    pub(super) fn insert_scoped(
        &mut self,
        entity_id: u64,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) {
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

    pub(super) fn insert_with_scopes<'a>(
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

    pub(super) fn remove_scoped(
        &mut self,
        entity_id: u64,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) {
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

    pub(super) fn remove_with_scopes<'a>(
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

    pub(super) fn ids_for(&self, key: &str, value: &PropertyValue) -> Option<&BTreeSet<u64>> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.values
            .get(key)
            .and_then(|values| values.get(&indexed_value))
    }

    pub(super) fn scoped_ids_for(
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

/// Hashable image of a [`PropertyValue`]. `None` from
/// [`PropertyIndexKey::from_value`] means "no stable hash" — temporal,
/// spatial, and vector values fall through to the scan fallback today.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum PropertyIndexKey {
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
    pub(super) fn from_value(value: &PropertyValue) -> Option<Self> {
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
