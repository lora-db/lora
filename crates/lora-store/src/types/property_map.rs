//! Compact property bag: a key-sorted `Vec` with a `BTreeMap`-shaped API.
//!
//! Property bags are small (a handful of keys per node or relationship)
//! and there are millions of them, so their fixed overhead dominates the
//! store's footprint. `BTreeMap` allocates a whole leaf node on the first
//! insert, sized for eleven entries: with `Arc<str>` keys and
//! `PropertyValue` values that is ~720 bytes even for a single property,
//! measured with a counting allocator (`examples/heap_probe.rs` in
//! `lora-database`). A sorted `Vec` pays for exactly the entries it holds.
//!
//! Lookups are a binary search over contiguous memory, which is at least
//! as fast as a B-tree walk at these sizes. Inserting a new key into the
//! middle shifts the tail, O(n); building from an iterator sorts once
//! instead, so bulk construction stays O(n log n).
//!
//! Semantics mirror `BTreeMap<Arc<str>, PropertyValue>` exactly so the
//! swap is invisible to callers: iteration is in ascending key order,
//! `insert` returns the displaced value, later duplicates win when
//! collecting, equality is order-sensitive over sorted entries, and serde
//! reads and writes a plain map, so existing snapshots and WAL records
//! decode unchanged.

use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::PropertyValue;

type Entry = (Arc<str>, PropertyValue);

#[derive(Clone, Default, PartialEq)]
pub struct PropertyMap {
    /// Sorted by key, keys unique.
    entries: Vec<Entry>,
}

impl PropertyMap {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Allocated entry slots. Used by the memory estimator.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    pub fn shrink_to_fit(&mut self) {
        self.entries.shrink_to_fit();
    }

    #[inline]
    fn find<Q>(&self, key: &Q) -> Result<usize, usize>
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.entries
            .binary_search_by(|(k, _)| Borrow::<Q>::borrow(k).cmp(key))
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&PropertyValue>
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.find(key).ok().map(|i| &self.entries[i].1)
    }

    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&Arc<str>, &PropertyValue)>
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.find(key).ok().map(|i| {
            let (k, v) = &self.entries[i];
            (k, v)
        })
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut PropertyValue>
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match self.find(key) {
            Ok(i) => Some(&mut self.entries[i].1),
            Err(_) => None,
        }
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.find(key).is_ok()
    }

    /// Insert or replace. Returns the previous value, like `BTreeMap`.
    /// On replacement the stored key is kept, again like `BTreeMap`.
    pub fn insert(&mut self, key: Arc<str>, value: PropertyValue) -> Option<PropertyValue> {
        // Appending in key order is the common construction pattern
        // (sorted sources, replays); check the tail before searching.
        match self.entries.last() {
            None => {
                self.entries.push((key, value));
                return None;
            }
            Some((last, _)) if **last < *key => {
                self.entries.push((key, value));
                return None;
            }
            _ => {}
        }
        match self.find(&*key) {
            Ok(i) => Some(std::mem::replace(&mut self.entries[i].1, value)),
            Err(i) => {
                self.entries.insert(i, (key, value));
                None
            }
        }
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<PropertyValue>
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.remove_entry(key).map(|(_, v)| v)
    }

    pub fn remove_entry<Q>(&mut self, key: &Q) -> Option<(Arc<str>, PropertyValue)>
    where
        Arc<str>: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match self.find(key) {
            Ok(i) => Some(self.entries.remove(i)),
            Err(_) => None,
        }
    }

    pub fn retain(&mut self, mut f: impl FnMut(&Arc<str>, &mut PropertyValue) -> bool) {
        self.entries.retain_mut(|(k, v)| f(k, v));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Move every entry of `other` into `self`; `other`'s values win on
    /// key collisions. Leaves `other` empty, like `BTreeMap::append`.
    pub fn append(&mut self, other: &mut Self) {
        let incoming = std::mem::take(&mut other.entries);
        self.extend(incoming);
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            inner: self.entries.iter(),
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_> {
        IterMut {
            inner: self.entries.iter_mut(),
        }
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &Arc<str>> + ExactSizeIterator + '_ {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(
        &self,
    ) -> impl DoubleEndedIterator<Item = &PropertyValue> + ExactSizeIterator + '_ {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(
        &mut self,
    ) -> impl DoubleEndedIterator<Item = &mut PropertyValue> + ExactSizeIterator + '_ {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    pub fn into_keys(self) -> impl DoubleEndedIterator<Item = Arc<str>> + ExactSizeIterator {
        self.entries.into_iter().map(|(k, _)| k)
    }

    pub fn into_values(self) -> impl DoubleEndedIterator<Item = PropertyValue> + ExactSizeIterator {
        self.entries.into_iter().map(|(_, v)| v)
    }

    pub fn first_key_value(&self) -> Option<(&Arc<str>, &PropertyValue)> {
        self.entries.first().map(|(k, v)| (k, v))
    }

    pub fn last_key_value(&self) -> Option<(&Arc<str>, &PropertyValue)> {
        self.entries.last().map(|(k, v)| (k, v))
    }

    /// Build from entries in arbitrary order. Later duplicates win, the
    /// same rule `BTreeMap: FromIterator` applies.
    fn from_unsorted(mut entries: Vec<Entry>) -> Self {
        if !entries.windows(2).all(|w| w[0].0 < w[1].0) {
            // Stable sort keeps duplicates in input order; keep the last
            // of each run.
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut deduped: Vec<Entry> = Vec::with_capacity(entries.len());
            for entry in entries {
                match deduped.last_mut() {
                    Some(last) if last.0 == entry.0 => *last = entry,
                    _ => deduped.push(entry),
                }
            }
            entries = deduped;
        }
        entries.shrink_to_fit();
        Self { entries }
    }
}

impl fmt::Debug for PropertyMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl FromIterator<(Arc<str>, PropertyValue)> for PropertyMap {
    fn from_iter<I: IntoIterator<Item = (Arc<str>, PropertyValue)>>(iter: I) -> Self {
        Self::from_unsorted(iter.into_iter().collect())
    }
}

impl<const N: usize> From<[(Arc<str>, PropertyValue); N]> for PropertyMap {
    fn from(arr: [(Arc<str>, PropertyValue); N]) -> Self {
        Self::from_unsorted(Vec::from(arr))
    }
}

impl Extend<(Arc<str>, PropertyValue)> for PropertyMap {
    fn extend<I: IntoIterator<Item = (Arc<str>, PropertyValue)>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        if lower > 4 && lower >= self.entries.len() {
            // Large merge: concatenate and re-sort once instead of
            // shifting per insert. `from_unsorted` keeps the last
            // duplicate, so incoming values win as with `insert`.
            let mut all = std::mem::take(&mut self.entries);
            all.extend(iter);
            *self = Self::from_unsorted(all);
        } else {
            for (k, v) in iter {
                self.insert(k, v);
            }
        }
    }
}

impl<'a> Extend<(&'a Arc<str>, &'a PropertyValue)> for PropertyMap {
    fn extend<I: IntoIterator<Item = (&'a Arc<str>, &'a PropertyValue)>>(&mut self, iter: I) {
        self.extend(iter.into_iter().map(|(k, v)| (k.clone(), v.clone())));
    }
}

impl<Q> std::ops::Index<&Q> for PropertyMap
where
    Arc<str>: Borrow<Q>,
    Q: Ord + ?Sized,
{
    type Output = PropertyValue;

    fn index(&self, key: &Q) -> &PropertyValue {
        self.get(key).expect("no entry found for key")
    }
}

pub struct Iter<'a> {
    inner: std::slice::Iter<'a, Entry>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a Arc<str>, &'a PropertyValue);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| (k, v))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl DoubleEndedIterator for Iter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().map(|(k, v)| (k, v))
    }
}

impl ExactSizeIterator for Iter<'_> {}

pub struct IterMut<'a> {
    inner: std::slice::IterMut<'a, Entry>,
}

impl<'a> Iterator for IterMut<'a> {
    type Item = (&'a Arc<str>, &'a mut PropertyValue);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| (&*k, v))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl DoubleEndedIterator for IterMut<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().map(|(k, v)| (&*k, v))
    }
}

impl ExactSizeIterator for IterMut<'_> {}

impl<'a> IntoIterator for &'a PropertyMap {
    type Item = (&'a Arc<str>, &'a PropertyValue);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut PropertyMap {
    type Item = (&'a Arc<str>, &'a mut PropertyValue);
    type IntoIter = IterMut<'a>;

    fn into_iter(self) -> IterMut<'a> {
        self.iter_mut()
    }
}

impl IntoIterator for PropertyMap {
    type Item = (Arc<str>, PropertyValue);
    type IntoIter = std::vec::IntoIter<Entry>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl Serialize for PropertyMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for PropertyMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor;

        impl<'de> Visitor<'de> for MapVisitor {
            type Value = PropertyMap;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a property map")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<PropertyMap, A::Error> {
                // Cap the pre-allocation: size hints come from untrusted
                // input (snapshots, WAL, wire formats).
                let cap = access.size_hint().unwrap_or(0).min(1024);
                let mut entries: Vec<Entry> = Vec::with_capacity(cap);
                while let Some((k, v)) = access.next_entry::<String, PropertyValue>()? {
                    entries.push((crate::intern_owned(k), v));
                }
                Ok(PropertyMap::from_unsorted(entries))
            }
        }

        deserializer.deserialize_map(MapVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn k(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    #[test]
    fn behaves_like_btreemap() {
        let ops: &[(&str, i64)] = &[("b", 1), ("a", 2), ("c", 3), ("a", 4), ("d", 5), ("b", 6)];
        let mut ours = PropertyMap::new();
        let mut reference: BTreeMap<Arc<str>, PropertyValue> = BTreeMap::new();
        for (key, v) in ops {
            assert_eq!(
                ours.insert(k(key), PropertyValue::Int(*v)),
                reference.insert(k(key), PropertyValue::Int(*v)),
            );
        }
        assert!(ours.iter().eq(reference.iter()));
        assert_eq!(ours.remove("c"), reference.remove("c"));
        assert_eq!(ours.remove("zz"), reference.remove("zz"));
        assert!(ours.iter().eq(reference.iter()));
        assert_eq!(ours.get("a"), reference.get("a"));
        assert_eq!(ours.len(), reference.len());
    }

    #[test]
    fn collect_sorts_and_last_duplicate_wins() {
        let m: PropertyMap = vec![
            (k("z"), PropertyValue::Int(1)),
            (k("a"), PropertyValue::Int(2)),
            (k("z"), PropertyValue::Int(3)),
        ]
        .into_iter()
        .collect();
        let keys: Vec<&str> = m.keys().map(|k| &**k).collect();
        assert_eq!(keys, ["a", "z"]);
        assert_eq!(m.get("z"), Some(&PropertyValue::Int(3)));
        assert_eq!(m.capacity(), 2);
    }

    #[test]
    fn extend_large_batch_overrides_existing() {
        let mut m = PropertyMap::new();
        m.insert(k("a"), PropertyValue::Int(0));
        m.insert(k("m"), PropertyValue::Int(0));
        m.extend(
            (0..10)
                .map(|i| (k(&format!("k{i}")), PropertyValue::Int(i)))
                .chain([(k("a"), PropertyValue::Int(99))]),
        );
        assert_eq!(m.get("a"), Some(&PropertyValue::Int(99)));
        assert_eq!(m.get("m"), Some(&PropertyValue::Int(0)));
        assert_eq!(m.len(), 12);
        assert!(m.keys().zip(m.keys().skip(1)).all(|(a, b)| a < b));
    }

    #[test]
    fn serde_matches_btreemap_encoding() {
        let mut ours = PropertyMap::new();
        let mut reference: BTreeMap<Arc<str>, PropertyValue> = BTreeMap::new();
        for (key, v) in [("name", 1), ("age", 2)] {
            ours.insert(k(key), PropertyValue::Int(v));
            reference.insert(k(key), PropertyValue::Int(v));
        }
        let a = serde_json::to_string(&ours).unwrap();
        let b = serde_json::to_string(&reference).unwrap();
        assert_eq!(a, b);
        let back: PropertyMap = serde_json::from_str(&b).unwrap();
        assert_eq!(back, ours);
    }
}
