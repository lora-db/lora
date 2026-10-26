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
//! `PropertyValue → indexable key` projection rules apply. `Ord` follows
//! the variant ordering in `PropertyIndexKey` (integers and floats share
//! one run, by numeric value) plus lexicographic ordering for the inner
//! data (strings, lists, maps).

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
    /// graph copy shares them and copies only the path (root, group,
    /// partition) to the entry it changes.
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

    /// Ids whose value is a temporal of another kind than `like` (a DATE
    /// when `like` is a DATETIME, ...). Empty when `like` is not temporal;
    /// `None` when the scope is not indexed. Each temporal kind is one
    /// contiguous run of keys, so this reads only the other kinds' runs:
    /// two range probes plus the ids found.
    pub(super) fn other_temporal_kind_ids(
        &self,
        label: &str,
        property: &str,
        like: &PropertyValue,
    ) -> Option<Vec<u64>> {
        let scope = self
            .by_scope
            .get(&ScopedPropertyKey::new(label, property))?;
        let Some(key) = PropertyIndexKey::from_value(like).filter(PropertyIndexKey::is_temporal)
        else {
            return Some(Vec::new());
        };
        let (Some(kind_floor), Some(kind_ceiling)) = (key.kind_floor(), key.kind_ceiling()) else {
            return Some(Vec::new());
        };
        let (floor, ceiling) = PropertyIndexKey::all_temporals();
        let mut out = BTreeSet::new();
        extend_ids(
            &mut out,
            scope
                .by_value
                .range(Bound::Included(floor), Bound::Excluded(kind_floor)),
        );
        extend_ids(
            &mut out,
            scope
                .by_value
                .range(Bound::Excluded(kind_ceiling), Bound::Included(ceiling)),
        );
        Some(out.into_iter().collect())
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
/// one-sided temporal or numeric range is capped at the end of its kind.
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
    scope.by_value.upsert(
        key,
        || IdSet::new(id),
        |bucket| {
            bucket.insert(id);
        },
    );
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

    fn mixed_numbers() -> SortedPropertyIndex {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("P", "age");
        idx.insert("P", "age", 1, &PropertyValue::Int(30));
        idx.insert("P", "age", 2, &PropertyValue::Float(1.5));
        idx.insert("P", "age", 3, &PropertyValue::Int(10));
        idx.insert("P", "age", 4, &PropertyValue::Float(10.0));
        idx.insert("P", "age", 5, &PropertyValue::Null);
        idx.insert("P", "age", 6, &PropertyValue::Bool(true));
        idx.insert("P", "age", 7, &PropertyValue::String("x".into()));
        idx
    }

    fn ids(
        idx: &SortedPropertyIndex,
        lo: Option<PropertyValue>,
        hi: Option<PropertyValue>,
    ) -> Vec<u64> {
        idx.range_candidates("P", "age", lo.as_ref(), hi.as_ref())
            .unwrap()
            .into_iter()
            .collect()
    }

    /// Integers and floats share one numeric order: a float bound finds
    /// the integers past it and an integer bound the floats, and a
    /// one-sided range stays inside the numbers.
    #[test]
    fn mixed_int_and_float_ranges_compare_by_value() {
        use PropertyValue::{Float, Int};
        let idx = mixed_numbers();
        assert_eq!(ids(&idx, Some(Float(25.5)), None), vec![1]);
        assert_eq!(ids(&idx, None, Some(Int(9))), vec![2]);
        // Equal values of the other kind sit on the inclusive bound.
        assert_eq!(ids(&idx, Some(Float(10.0)), Some(Int(10))), vec![3, 4]);
        assert_eq!(ids(&idx, Some(Int(10)), Some(Float(10.0))), vec![3, 4]);
        assert_eq!(ids(&idx, None, Some(Int(10))), vec![2, 3, 4]);
        assert_eq!(ids(&idx, Some(Float(10.0)), None), vec![1, 3, 4]);
        assert_eq!(ids(&idx, Some(Int(2)), Some(Float(29.9))), vec![3, 4]);
    }

    #[test]
    fn ordered_walk_interleaves_ints_and_floats() {
        let idx = mixed_numbers();
        let lo = PropertyValue::Int(i64::MIN);
        let asc = idx
            .ordered_chunk("P", "age", Some(&lo), None, false, None, 100)
            .unwrap();
        assert_eq!(asc, vec![2, 3, 4, 1]);
        let desc = idx
            .ordered_chunk("P", "age", Some(&lo), None, true, None, 100)
            .unwrap();
        assert_eq!(desc, vec![1, 4, 3, 2]);
        // Resuming from a cursor on an integer continues with the float of
        // the same value.
        let after = PropertyValue::Int(10);
        let rest = idx
            .ordered_chunk("P", "age", Some(&lo), None, false, Some((&after, 3)), 100)
            .unwrap();
        assert_eq!(rest, vec![4, 1]);
    }

    /// Extremes: integers beyond 2^53 are not rounded through `f64`, and
    /// infinities bound every integer.
    #[test]
    fn numeric_order_is_exact_at_the_extremes() {
        use PropertyValue::{Float, Int};
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("P", "x");
        let big = 1i64 << 53;
        idx.insert("P", "x", 1, &Int(big + 1));
        idx.insert("P", "x", 2, &Float(big as f64));
        idx.insert("P", "x", 3, &Int(i64::MAX));
        idx.insert("P", "x", 4, &Float(9_223_372_036_854_775_808.0));
        idx.insert("P", "x", 5, &Int(i64::MIN));
        idx.insert("P", "x", 6, &Float(f64::NEG_INFINITY));
        idx.insert("P", "x", 7, &Float(-0.0));
        idx.insert("P", "x", 8, &Int(0));
        let lo = Float(f64::NEG_INFINITY);
        let order = idx
            .ordered_chunk("P", "x", Some(&lo), None, false, None, 100)
            .unwrap();
        assert_eq!(order, vec![6, 5, 8, 7, 2, 1, 3, 4]);
        let got: Vec<u64> = idx
            .range_candidates("P", "x", Some(&Float(big as f64)), Some(&Int(big)))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(got, vec![2]);
        let got: Vec<u64> = idx
            .range_candidates("P", "x", Some(&Int(0)), Some(&Float(0.0)))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(got, vec![7, 8]);
    }

    #[test]
    fn numeric_bounds_report_no_other_temporal_kind() {
        let idx = mixed_numbers();
        assert_eq!(
            idx.other_temporal_kind_ids("P", "age", &PropertyValue::Int(3)),
            Some(Vec::new())
        );
    }
}
