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

pub(super) type PropertyValueBuckets = HashMap<PropertyIndexKey, Vec<u64>>;
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
            .push(entity_id);
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
                if let Some(pos) = ids.iter().position(|&id| id == entity_id) {
                    ids.swap_remove(pos);
                }
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

    pub(super) fn ids_for(&self, key: &str, value: &PropertyValue) -> Option<&[u64]> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.values
            .get(key)
            .and_then(|values| values.get(&indexed_value))
            .map(Vec::as_slice)
    }

    pub(super) fn scoped_ids_for(
        &self,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) -> Option<&[u64]> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.scoped_values
            .get(scope)
            .and_then(|values| values.get(key))
            .and_then(|values| values.get(&indexed_value))
            .map(Vec::as_slice)
    }
}

/// Hashable & sortable image of a [`PropertyValue`]. `None` from
/// [`PropertyIndexKey::from_value`] means "no stable image" — temporal,
/// spatial, and vector values fall through to the scan fallback today.
///
/// `Ord` is hand-rolled (not derived) because [`crate::LoraBinary`]
/// doesn't expose `Ord` on its segmented byte representation. The
/// custom impl walks variants in their declaration order and falls
/// through to lexicographic ordering of inner data.
///
/// Floats use a sortable IEEE-754 bit projection, so range indexes
/// preserve numeric ordering across negative and positive values.
/// `f64::NAN` is rejected upstream by `from_value`.
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

impl PartialOrd for PropertyIndexKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PropertyIndexKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let tag = |k: &PropertyIndexKey| match k {
            PropertyIndexKey::Null => 0,
            PropertyIndexKey::Bool(_) => 1,
            PropertyIndexKey::Int(_) => 2,
            PropertyIndexKey::Float(_) => 3,
            PropertyIndexKey::String(_) => 4,
            PropertyIndexKey::Binary(_) => 5,
            PropertyIndexKey::List(_) => 6,
            PropertyIndexKey::Map(_) => 7,
        };
        match tag(self).cmp(&tag(other)) {
            Ordering::Equal => match (self, other) {
                (PropertyIndexKey::Null, PropertyIndexKey::Null) => Ordering::Equal,
                (PropertyIndexKey::Bool(a), PropertyIndexKey::Bool(b)) => a.cmp(b),
                (PropertyIndexKey::Int(a), PropertyIndexKey::Int(b)) => a.cmp(b),
                (PropertyIndexKey::Float(a), PropertyIndexKey::Float(b)) => a.cmp(b),
                (PropertyIndexKey::String(a), PropertyIndexKey::String(b)) => a.cmp(b),
                (PropertyIndexKey::Binary(a), PropertyIndexKey::Binary(b)) => {
                    // Lexicographic byte comparison across segments.
                    // Allocates only when LoraBinary doesn't expose a
                    // contiguous view; for sorted-index inserts this
                    // happens once per insertion and is bounded by the
                    // value size.
                    let aa: Vec<u8> = a.segments().iter().flatten().copied().collect();
                    let bb: Vec<u8> = b.segments().iter().flatten().copied().collect();
                    aa.cmp(&bb)
                }
                (PropertyIndexKey::List(a), PropertyIndexKey::List(b)) => a.cmp(b),
                (PropertyIndexKey::Map(a), PropertyIndexKey::Map(b)) => a.cmp(b),
                _ => Ordering::Equal, // unreachable given equal tags
            },
            ord => ord,
        }
    }
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
                } else {
                    Some(Self::Float(sortable_f64_bits(*v)))
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

fn sortable_f64_bits(value: f64) -> u64 {
    let bits = if value == 0.0 {
        0.0f64.to_bits()
    } else {
        value.to_bits()
    };
    if bits & (1 << 63) == 0 {
        bits | (1 << 63)
    } else {
        !bits
    }
}
