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
use std::ops::Bound;

use crate::types::PropertyValue;

use super::cow::CowOrdMap;
use super::entity_index_store::ScopedPropertyKey;
use super::id_set::IdSet;
use super::property_index::PropertyIndexKey;

/// Sorted bucket: every value seen for an indexed property mapped to
/// the ids carrying that value. The outer map is keyed by
/// `(label-or-type, property)` so range queries don't have to scan
/// across labels.
#[derive(Debug, Default, Clone)]
pub(super) struct SortedPropertyIndex {
    pub(super) by_scope: BTreeMap<ScopedPropertyKey, SortedScope>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct SortedScope {
    /// Keyed by sortable property key. Values are ids in that bucket.
    /// Copy-on-write partitions (see [`CowOrdMap`]): a write's staged
    /// graph copy shares them and copies only the partition it changes.
    pub(super) by_value: CowOrdMap<PropertyIndexKey, IdSet>,
    /// Refcount of catalog entries pointing at this scope.
    refcount: u32,
}

impl SortedPropertyIndex {
    /// Whether any of `scopes` has an index on `property`. Checked through
    /// a read guard before maintenance takes a (copy-on-write) write guard.
    pub(super) fn covers_any<'a>(
        &self,
        scopes: impl IntoIterator<Item = &'a str>,
        property: &str,
    ) -> bool {
        !self.by_scope.is_empty()
            && scopes.into_iter().any(|scope| {
                self.by_scope
                    .contains_key(&ScopedPropertyKey::new(scope, property))
            })
    }

    pub(super) fn add_scope(&mut self, label: &str, property: &str) -> bool {
        let entry = self
            .by_scope
            .entry(ScopedPropertyKey::new(label, property))
            .or_default();
        let was_empty = entry.refcount == 0;
        entry.refcount = entry.refcount.saturating_add(1);
        was_empty
    }

    pub(super) fn remove_scope(&mut self, label: &str, property: &str) {
        let key = ScopedPropertyKey::new(label, property);
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
        if let Some(scope) = self
            .by_scope
            .get_mut(&ScopedPropertyKey::new(label, property))
        {
            insert_id(scope, key, id);
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
        let Some(scope) = self
            .by_scope
            .get_mut(&ScopedPropertyKey::new(label, property))
        else {
            return;
        };
        if let Some(old) = old.and_then(PropertyIndexKey::from_value) {
            Self::remove_from_scope(scope, id, &old);
        }
        if let Some(new) = new.and_then(PropertyIndexKey::from_value) {
            insert_id(scope, new, id);
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
        let scope = self
            .by_scope
            .get(&ScopedPropertyKey::new(label, property))?;
        let (lower, upper) = probe_bounds(lo, hi)?;
        let mut out = BTreeSet::new();
        if let (Bound::Included(l), Bound::Included(u)) = (&lower, &upper) {
            if l > u {
                return Some(out);
            }
        }
        extend_ids(&mut out, scope.by_value.range(lower, upper));
        Some(out)
    }

    /// Whether the scope holds a temporal value of another kind than
    /// `like` (a DATE when `like` is a DATETIME, ...). `Some(false)` when
    /// `like` is not temporal; `None` when the scope is not indexed. Two
    /// probes: the temporal kinds are contiguous, so only the first and
    /// last temporal keys can differ in kind from `like`.
    pub(super) fn holds_other_temporal_kind(
        &self,
        label: &str,
        property: &str,
        like: &PropertyValue,
    ) -> Option<bool> {
        let scope = self
            .by_scope
            .get(&ScopedPropertyKey::new(label, property))?;
        let Some(kind) = PropertyIndexKey::from_value(like).and_then(|k| k.temporal_kind()) else {
            return Some(false);
        };
        let (floor, ceiling) = PropertyIndexKey::all_temporals();
        let mut temporals = scope
            .by_value
            .range(Bound::Included(floor), Bound::Included(ceiling));
        let other = |key: Option<(&PropertyIndexKey, &IdSet)>| {
            key.and_then(|(k, _)| k.temporal_kind())
                .is_some_and(|k| k != kind)
        };
        Some(other(temporals.next()) || other(temporals.next_back()))
    }

    /// Ids in `[lo, hi]` (inclusive; the caller refilters exact bounds)
    /// in value order, then id order within a value, starting strictly
    /// after `after` and returning at most `max`. `descending` walks the
    /// same order backwards. `None` when the scope is not indexed.
    ///
    /// Paging by `(value, id)` cursor rather than holding an iterator lets
    /// a streaming scan pull one chunk at a time without keeping the index
    /// lock between pulls; a `LIMIT` above it stops after a few chunks.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ordered_chunk(
        &self,
        label: &str,
        property: &str,
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
        descending: bool,
        after: Option<(&PropertyValue, u64)>,
        max: usize,
    ) -> Option<Vec<u64>> {
        let scope = self
            .by_scope
            .get(&ScopedPropertyKey::new(label, property))?;
        let after = after.and_then(|(v, id)| PropertyIndexKey::from_value(v).map(|k| (k, id)));

        // Narrow the value range to start at the cursor's value.
        let (mut lower, mut upper) = probe_bounds(lo, hi)?;
        if let Some((key, _)) = &after {
            if descending {
                upper = Bound::Included(key.clone());
            } else {
                lower = Bound::Included(key.clone());
            }
        }
        if let (Bound::Included(l), Bound::Included(u)) = (&lower, &upper) {
            if l > u {
                return Some(Vec::new());
            }
        }

        let mut out = Vec::with_capacity(max.min(1024));
        // Returns true once the chunk is full.
        let mut push_bucket = |key: &PropertyIndexKey, ids: &IdSet| -> bool {
            let cursor_id = after.as_ref().filter(|(k, _)| k == key).map(|(_, id)| *id);
            for id in ids.iter_after(cursor_id, descending) {
                if out.len() >= max {
                    return true;
                }
                out.push(id);
            }
            out.len() >= max
        };
        let range = scope.by_value.range(lower, upper);
        if descending {
            for (key, ids) in range.rev() {
                if push_bucket(key, ids) {
                    break;
                }
            }
        } else {
            for (key, ids) in range {
                if push_bucket(key, ids) {
                    break;
                }
            }
        }
        Some(out)
    }

    fn remove_from_scope(scope: &mut SortedScope, id: u64, key: &PropertyIndexKey) {
        let emptied = scope
            .by_value
            .get_mut(key)
            .is_some_and(|bucket| bucket.remove(id));
        if emptied {
            scope.by_value.remove(key);
        }
    }
}

/// Index bounds for an inclusive `[lo, hi]` probe. `None` when a bound has
/// no index image (a duration, say): values of that type are not indexed,
/// so the caller must scan. Temporal bounds widen across UTC offsets, and a
/// one-sided temporal range is capped at the end of its temporal kind.
fn probe_bounds(
    lo: Option<&PropertyValue>,
    hi: Option<&PropertyValue>,
) -> Option<(Bound<PropertyIndexKey>, Bound<PropertyIndexKey>)> {
    let lo_key = match lo {
        Some(v) => Some(PropertyIndexKey::range_lower(v)?),
        None => None,
    };
    let hi_key = match hi {
        Some(v) => Some(PropertyIndexKey::range_upper(v)?),
        None => None,
    };
    let (lo_key, hi_key) = match (lo_key, hi_key) {
        (Some(l), None) => {
            let h = l.kind_ceiling();
            (Some(l), h)
        }
        (None, Some(h)) => (h.kind_floor(), Some(h)),
        other => other,
    };
    Some((
        lo_key.map_or(Bound::Unbounded, Bound::Included),
        hi_key.map_or(Bound::Unbounded, Bound::Included),
    ))
}

fn insert_id(scope: &mut SortedScope, key: PropertyIndexKey, id: u64) {
    match scope.by_value.get_mut(&key) {
        Some(bucket) => {
            bucket.insert(id);
        }
        None => {
            scope.by_value.get_or_insert_with(key, || IdSet::new(id));
        }
    }
}

fn extend_ids<'a>(
    out: &mut BTreeSet<u64>,
    iter: impl Iterator<Item = (&'a PropertyIndexKey, &'a IdSet)>,
) {
    for (_, ids) in iter {
        out.extend(ids.iter());
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
