//! Sorted property index — a `BTreeMap`-backed companion to the
//! hash-bucket [`super::property_index::PropertyIndex`].
//!
//! The hash index answers point equality in O(1); the sorted index
//! lets the optimizer answer range predicates (`>`, `<`, `BETWEEN`,
//! prefix `STARTS WITH` over strings) without scanning. Both
//! structures are populated for `RANGE` catalog entries; the
//! optimizer chooses between them based on the predicate shape.
//!
//! ## Why a separate structure
//!
//! `PropertyIndex` already stores `(value → ids)`; making it sorted
//! by switching `HashMap<PropertyIndexKey, …>` to `BTreeMap` would
//! reorder semantics for every existing caller. Keeping the structures
//! parallel means we pay for sort order only when a `RANGE` catalog
//! entry exists and a range predicate is on the hot path.
//!
//! ## Comparison key
//!
//! Reuses [`super::property_index::PropertyIndexKey`] so the same
//! `PropertyValue → indexable key` projection rules apply. `Ord` is
//! derived from the variant ordering in `PropertyIndexKey` plus
//! lexicographic ordering for the inner data (strings, lists, maps).

use std::collections::{BTreeMap, BTreeSet};

use crate::types::PropertyValue;

use super::property_index::PropertyIndexKey;

/// Sorted bucket: every value seen for an indexed property mapped to
/// the ids carrying that value. The outer map is keyed by
/// `(label-or-type, property)` so range queries don't have to scan
/// across labels.
#[derive(Debug, Default, Clone)]
pub(super) struct SortedPropertyIndex {
    by_scope: BTreeMap<SortedScopeKey, SortedScope>,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub(super) struct SortedScopeKey {
    pub label: String,
    pub property: String,
}

#[derive(Debug, Default, Clone)]
pub(super) struct SortedScope {
    /// Keyed by sortable property key. Values are ids in that bucket.
    by_value: BTreeMap<PropertyIndexKey, BTreeSet<u64>>,
    /// Refcount of catalog entries pointing at this scope.
    refcount: u32,
}

impl SortedPropertyIndex {
    pub(super) fn add_scope(&mut self, label: &str, property: &str) -> bool {
        let entry = self
            .by_scope
            .entry(SortedScopeKey {
                label: label.to_string(),
                property: property.to_string(),
            })
            .or_default();
        let was_empty = entry.refcount == 0;
        entry.refcount = entry.refcount.saturating_add(1);
        was_empty
    }

    pub(super) fn remove_scope(&mut self, label: &str, property: &str) {
        let key = SortedScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        };
        if let Some(scope) = self.by_scope.get_mut(&key) {
            scope.refcount = scope.refcount.saturating_sub(1);
            if scope.refcount == 0 {
                self.by_scope.remove(&key);
            }
        }
    }

    pub(super) fn insert(&mut self, label: &str, property: &str, id: u64, value: &PropertyValue) {
        let Some(key) = PropertyIndexKey::from_value(value) else {
            return;
        };
        if let Some(scope) = self.by_scope.get_mut(&SortedScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        }) {
            scope.by_value.entry(key).or_default().insert(id);
        }
    }

    pub(super) fn update(
        &mut self,
        label: &str,
        property: &str,
        id: u64,
        old: Option<&PropertyValue>,
        new: Option<&PropertyValue>,
    ) {
        let Some(scope) = self.by_scope.get_mut(&SortedScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        }) else {
            return;
        };
        if let Some(old) = old.and_then(PropertyIndexKey::from_value) {
            Self::remove_from_scope(scope, id, &old);
        }
        if let Some(new) = new.and_then(PropertyIndexKey::from_value) {
            scope.by_value.entry(new).or_default().insert(id);
        }
    }

    /// Range probe: every id whose value falls in `[lo, hi]`. Both
    /// bounds are inclusive at this layer — the caller refilters with
    /// the precise predicate inclusivity. `lo == None` means `-∞`;
    /// `hi == None` means `+∞`. Returns `None` when no scope exists,
    /// signalling "fall back to scan."
    pub(super) fn range_candidates(
        &self,
        label: &str,
        property: &str,
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
    ) -> Option<BTreeSet<u64>> {
        let scope = self.by_scope.get(&SortedScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        })?;
        let lo_key = lo.and_then(PropertyIndexKey::from_value);
        let hi_key = hi.and_then(PropertyIndexKey::from_value);
        let mut out = BTreeSet::new();
        match (&lo_key, &hi_key) {
            (Some(l), Some(h)) => extend_ids(&mut out, scope.by_value.range(l..=h)),
            (Some(l), None) => extend_ids(&mut out, scope.by_value.range(l..)),
            (None, Some(h)) => extend_ids(&mut out, scope.by_value.range(..=h)),
            (None, None) => extend_ids(&mut out, scope.by_value.iter()),
        }
        Some(out)
    }

    fn remove_from_scope(scope: &mut SortedScope, id: u64, key: &PropertyIndexKey) {
        if let Some(bucket) = scope.by_value.get_mut(key) {
            bucket.remove(&id);
            if bucket.is_empty() {
                scope.by_value.remove(key);
            }
        }
    }
}

fn extend_ids<'a>(
    out: &mut BTreeSet<u64>,
    iter: impl Iterator<Item = (&'a PropertyIndexKey, &'a BTreeSet<u64>)>,
) {
    for (_, ids) in iter {
        out.extend(ids.iter().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PropertyValue;

    #[test]
    fn range_excludes_outside_bucket() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Person", "age");
        idx.insert("Person", "age", 1, &PropertyValue::Int(20));
        idx.insert("Person", "age", 2, &PropertyValue::Int(30));
        idx.insert("Person", "age", 3, &PropertyValue::Int(40));

        let lo = PropertyValue::Int(25);
        let hi = PropertyValue::Int(35);
        let got = idx
            .range_candidates("Person", "age", Some(&lo), Some(&hi))
            .unwrap();
        assert!(got.contains(&2));
        assert!(!got.contains(&1));
        assert!(!got.contains(&3));
    }

    #[test]
    fn range_includes_inclusive_boundaries() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Person", "age");
        idx.insert("Person", "age", 1, &PropertyValue::Int(20));
        idx.insert("Person", "age", 2, &PropertyValue::Int(30));

        let lo = PropertyValue::Int(20);
        let hi = PropertyValue::Int(30);
        let got = idx
            .range_candidates("Person", "age", Some(&lo), Some(&hi))
            .unwrap();
        assert!(got.contains(&1));
        assert!(got.contains(&2));
    }

    #[test]
    fn open_lower_bound_includes_all_below() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Person", "age");
        idx.insert("Person", "age", 1, &PropertyValue::Int(20));
        idx.insert("Person", "age", 2, &PropertyValue::Int(30));

        let hi = PropertyValue::Int(25);
        let got = idx
            .range_candidates("Person", "age", None, Some(&hi))
            .unwrap();
        assert!(got.contains(&1));
        assert!(!got.contains(&2));
    }

    #[test]
    fn float_ranges_keep_numeric_order_across_zero() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Reading", "temperature");
        idx.insert("Reading", "temperature", 1, &PropertyValue::Float(-10.0));
        idx.insert("Reading", "temperature", 2, &PropertyValue::Float(-1.5));
        idx.insert("Reading", "temperature", 3, &PropertyValue::Float(2.0));

        let lo = PropertyValue::Float(-2.0);
        let hi = PropertyValue::Float(1.0);
        let got = idx
            .range_candidates("Reading", "temperature", Some(&lo), Some(&hi))
            .unwrap();
        assert!(!got.contains(&1));
        assert!(got.contains(&2));
        assert!(!got.contains(&3));
    }
}
