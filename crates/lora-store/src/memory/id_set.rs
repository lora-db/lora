//! Posting list for property-index buckets: the set of entity ids that
//! share one indexed value.
//!
//! Most indexed properties are unique or near-unique (ids, emails,
//! names), so the overwhelmingly common bucket holds one id. Storing
//! that id inline avoids a heap allocation per distinct value: with a
//! `Vec<u64>` the first push allocated room for four ids, and with a
//! `BTreeSet<u64>` it allocated a whole B-tree leaf (~100 bytes) for
//! eight bytes of payload.
//!
//! Buckets grow through a sorted `Vec` and switch to a `BTreeSet` past
//! [`SMALL_MAX`], so removal stays O(log n) for low-cardinality values
//! (a boolean flag on a million nodes) instead of the O(n) shift or
//! scan a flat vector would need on every delete.
//!
//! Ids are kept sorted and unique in every representation, so lookups
//! return candidates in ascending id order, the same order a label scan
//! produces.

use std::collections::BTreeSet;

/// Largest bucket kept as a sorted vector. Past this a mid-vector
/// insert or remove shifts more memory than a B-tree walk costs.
const SMALL_MAX: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IdSet {
    One(u64),
    Small(Vec<u64>),
    Large(BTreeSet<u64>),
}

impl IdSet {
    pub(super) fn new(id: u64) -> Self {
        IdSet::One(id)
    }

    /// Insert `id`; returns `false` if it was already present.
    pub(super) fn insert(&mut self, id: u64) -> bool {
        match self {
            IdSet::One(existing) => {
                let existing = *existing;
                if existing == id {
                    return false;
                }
                let pair = if existing < id {
                    vec![existing, id]
                } else {
                    vec![id, existing]
                };
                *self = IdSet::Small(pair);
                true
            }
            IdSet::Small(ids) => {
                // Ids are allocated monotonically, so appends dominate.
                if ids.last().is_some_and(|&last| last < id) {
                    ids.push(id);
                } else {
                    match ids.binary_search(&id) {
                        Ok(_) => return false,
                        Err(pos) => ids.insert(pos, id),
                    }
                }
                if ids.len() > SMALL_MAX {
                    *self = IdSet::Large(std::mem::take(ids).into_iter().collect());
                }
                true
            }
            IdSet::Large(ids) => ids.insert(id),
        }
    }

    /// Remove `id`; returns `true` if the set is now empty and the
    /// bucket should be dropped by the caller.
    pub(super) fn remove(&mut self, id: u64) -> bool {
        match self {
            IdSet::One(existing) => *existing == id,
            IdSet::Small(ids) => {
                if let Ok(pos) = ids.binary_search(&id) {
                    ids.remove(pos);
                }
                if let [only] = ids.as_slice() {
                    *self = IdSet::One(*only);
                }
                ids_is_empty(self)
            }
            IdSet::Large(ids) => {
                ids.remove(&id);
                ids.is_empty()
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        match self {
            IdSet::One(_) => 1,
            IdSet::Small(ids) => ids.len(),
            IdSet::Large(ids) => ids.len(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn to_vec(&self) -> Vec<u64> {
        self.iter().collect()
    }

    /// Ids in ascending order.
    pub(super) fn iter(&self) -> IdSetIter<'_> {
        match self {
            IdSet::One(id) => IdSetIter::Slice(std::slice::from_ref(id).iter()),
            IdSet::Small(ids) => IdSetIter::Slice(ids.iter()),
            IdSet::Large(ids) => IdSetIter::Tree(ids.iter()),
        }
    }

    /// Ids strictly after `cursor` in the given direction (ascending:
    /// `> cursor`; descending: `< cursor`), in that direction. Seeks by
    /// binary search / B-tree range instead of scanning past the cursor.
    pub(super) fn iter_after(
        &self,
        cursor: Option<u64>,
        descending: bool,
    ) -> Box<dyn Iterator<Item = u64> + '_> {
        use std::ops::Bound::{Excluded, Unbounded};
        match (self, cursor) {
            (_, None) if descending => Box::new(self.iter().rev()),
            (_, None) => Box::new(self.iter()),
            (IdSet::One(id), Some(c)) => {
                let keep = if descending { *id < c } else { *id > c };
                Box::new(keep.then_some(*id).into_iter())
            }
            (IdSet::Small(ids), Some(c)) => {
                if descending {
                    let end = ids.partition_point(|&id| id < c);
                    Box::new(ids[..end].iter().rev().copied())
                } else {
                    let start = ids.partition_point(|&id| id <= c);
                    Box::new(ids[start..].iter().copied())
                }
            }
            (IdSet::Large(ids), Some(c)) => {
                if descending {
                    Box::new(ids.range((Unbounded, Excluded(c))).rev().copied())
                } else {
                    Box::new(ids.range((Excluded(c), Unbounded)).copied())
                }
            }
        }
    }

    /// Heap bytes owned beyond the inline enum. Used by the memory
    /// estimator.
    pub(super) fn heap_bytes(&self) -> usize {
        match self {
            IdSet::One(_) => 0,
            IdSet::Small(ids) => ids.capacity() * std::mem::size_of::<u64>(),
            // B-tree leaves hold up to 11 keys; charge a half-full leaf
            // per 5.5 ids plus the node header, matching the estimator's
            // treatment of other B-trees.
            IdSet::Large(ids) => ids.len() * (std::mem::size_of::<u64>() + 12),
        }
    }
}

fn ids_is_empty(set: &IdSet) -> bool {
    matches!(set, IdSet::Small(ids) if ids.is_empty())
}

pub(super) enum IdSetIter<'a> {
    Slice(std::slice::Iter<'a, u64>),
    Tree(std::collections::btree_set::Iter<'a, u64>),
}

impl Iterator for IdSetIter<'_> {
    type Item = u64;

    #[inline]
    fn next(&mut self) -> Option<u64> {
        match self {
            IdSetIter::Slice(it) => it.next().copied(),
            IdSetIter::Tree(it) => it.next().copied(),
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            IdSetIter::Slice(it) => it.size_hint(),
            IdSetIter::Tree(it) => it.size_hint(),
        }
    }
}

impl DoubleEndedIterator for IdSetIter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<u64> {
        match self {
            IdSetIter::Slice(it) => it.next_back().copied(),
            IdSetIter::Tree(it) => it.next_back().copied(),
        }
    }
}

impl ExactSizeIterator for IdSetIter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(set: &IdSet) -> Vec<u64> {
        set.iter().collect()
    }

    #[test]
    fn grows_through_every_representation_in_order() {
        let mut set = IdSet::new(500);
        for id in (0..200).rev() {
            assert!(set.insert(id));
        }
        assert!(matches!(set, IdSet::Large(_)));
        assert!(!set.insert(7), "duplicate insert must be rejected");
        let got = collect(&set);
        let mut want: Vec<u64> = (0..200).collect();
        want.push(500);
        assert_eq!(got, want);
        assert_eq!(set.len(), 201);
    }

    #[test]
    fn small_set_dedups_and_sorts() {
        let mut set = IdSet::new(5);
        assert!(set.insert(3));
        assert!(set.insert(9));
        assert!(!set.insert(3));
        assert!(!set.insert(5));
        assert_eq!(collect(&set), vec![3, 5, 9]);
    }

    #[test]
    fn remove_reports_empty_and_collapses_to_one() {
        let mut set = IdSet::new(1);
        set.insert(2);
        assert!(!set.remove(1));
        assert!(matches!(set, IdSet::One(2)));
        assert!(!set.remove(42), "removing a missing id leaves the set");
        assert!(set.remove(2));

        let mut large = IdSet::new(0);
        for id in 1..100 {
            large.insert(id);
        }
        for id in 0..99 {
            assert!(!large.remove(id));
        }
        assert!(large.remove(99));
    }
}
