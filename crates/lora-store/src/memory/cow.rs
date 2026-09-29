//! Copy-on-write collections for secondary indexes.
//!
//! Writes that cannot run in place work on a clone of the graph, so an
//! index clone has to be cheap, and a write should copy only the part of
//! an index it changes. Both types here split their contents into shards
//! behind `Arc`: cloning bumps one refcount per shard, and a mutation
//! copies just the shard it lands in (`Arc::make_mut`).
//!
//! [`CowMap`] and [`CowIdMap`] do not iterate in key order (the trigram,
//! point, hash-bucket and per-entity maps only need lookups, unions and
//! intersections); [`CowOrdMap`] does, for range and prefix scans.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::Arc;

/// FNV-1a: stable, fast, and good enough to spread short strings,
/// trigrams, index keys and grid cells. Used to pick a shard and inside
/// each shard's hash table.
#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

type FnvHashMap<K, V> = HashMap<K, V, BuildHasherDefault<Fnv>>;

/// Shards per [`CowMap`].
const SHARDS: usize = 256;
/// Shards per large [`CowIdMap`].
const ID_SHARDS: usize = 64;
/// A [`CowIdMap`] splits into [`SHARDS`] shards once it holds more ids
/// than this; below it a single map is smaller and faster.
const SPLIT_AT: usize = 1024;

/// A hash map sharded by key hash, each shard a hash table behind `Arc`.
/// Empty maps allocate nothing.
pub(super) struct CowMap<K, V> {
    shards: Vec<Arc<FnvHashMap<K, V>>>,
    len: usize,
}

impl<K, V> Default for CowMap<K, V> {
    fn default() -> Self {
        Self {
            shards: Vec::new(),
            len: 0,
        }
    }
}

impl<K, V> Clone for CowMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            shards: self.shards.clone(),
            len: self.len,
        }
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for CowMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.shards.iter().flat_map(|shard| shard.iter()))
            .finish()
    }
}

impl<K: Eq + Hash, V: PartialEq> PartialEq for CowMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self
                .shards
                .iter()
                .flat_map(|shard| shard.iter())
                .all(|(k, v)| {
                    other.shards.get(shard_of(k)).and_then(|shard| shard.get(k)) == Some(v)
                })
    }
}

fn shard_of<Q: Hash + ?Sized>(key: &Q) -> usize {
    let mut hasher = Fnv::default();
    key.hash(&mut hasher);
    (hasher.finish() as usize) % SHARDS
}

impl<K: Eq + Hash + Clone, V: Clone> CowMap<K, V> {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.shards.get(shard_of(key))?.get(key)
    }

    pub(super) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Mutable access to an existing entry; copies its shard if shared.
    pub(super) fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let shard = self.shards.get_mut(shard_of(key))?;
        if !shard.contains_key(key) {
            return None;
        }
        Arc::make_mut(shard).get_mut(key)
    }

    /// Mutable access to `key`'s value, inserting `make()` first if absent.
    pub(super) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        if self.shards.is_empty() {
            self.shards = (0..SHARDS)
                .map(|_| Arc::new(FnvHashMap::default()))
                .collect();
        }
        let shard = Arc::make_mut(&mut self.shards[shard_of(&key)]);
        let len = &mut self.len;
        shard.entry(key).or_insert_with(|| {
            *len += 1;
            make()
        })
    }

    pub(super) fn insert(&mut self, key: K, value: V) -> Option<V> {
        let mut value = Some(value);
        let slot = self.get_or_insert_with(key, || value.take().expect("value unused"));
        value.map(|v| std::mem::replace(slot, v))
    }

    pub(super) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let shard = self.shards.get_mut(shard_of(key))?;
        if !shard.contains_key(key) {
            return None;
        }
        let removed = Arc::make_mut(shard).remove(key);
        if removed.is_some() {
            self.len -= 1;
        }
        removed
    }

    /// Entries in no particular order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (&K, &V)> + '_ {
        self.shards.iter().flat_map(|shard| shard.iter())
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.iter().map(|(_, v)| v)
    }
}

/// Entity ids with a value each (a posting list). Small lists are one
/// shared map; past [`SPLIT_AT`] ids they split into [`SHARDS`] shards by
/// id, so updating one entity in a posting shared by most of the graph
/// copies a few hundred entries, not all of them.
pub(super) struct CowIdMap<V> {
    /// The shard table is itself shared, so cloning a posting list (as
    /// happens for every entry of a copied [`CowMap`] shard) is one
    /// refcount bump rather than an allocation.
    shards: Arc<Vec<Arc<BTreeMap<u64, V>>>>,
    len: usize,
}

impl<V> Default for CowIdMap<V> {
    fn default() -> Self {
        Self {
            shards: Arc::new(Vec::new()),
            len: 0,
        }
    }
}

impl<V> Clone for CowIdMap<V> {
    fn clone(&self) -> Self {
        Self {
            shards: Arc::clone(&self.shards),
            len: self.len,
        }
    }
}

impl<V: std::fmt::Debug> std::fmt::Debug for CowIdMap<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.shards.iter().flat_map(|shard| shard.iter()))
            .finish()
    }
}

impl<V: Clone> CowIdMap<V> {
    #[inline]
    fn shard(&self, id: u64) -> usize {
        (id % self.shards.len() as u64) as usize
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn get(&self, id: &u64) -> Option<&V> {
        if self.shards.is_empty() {
            return None;
        }
        self.shards[self.shard(*id)].get(id)
    }

    pub(super) fn contains_key(&self, id: &u64) -> bool {
        self.get(id).is_some()
    }

    pub(super) fn insert(&mut self, id: u64, value: V) -> Option<V> {
        if self.shards.is_empty() {
            Arc::make_mut(&mut self.shards).push(Arc::new(BTreeMap::new()));
        }
        let idx = self.shard(id);
        let table = Arc::make_mut(&mut self.shards);
        let previous = Arc::make_mut(&mut table[idx]).insert(id, value);
        if previous.is_none() {
            self.len += 1;
            if self.shards.len() == 1 && self.len > SPLIT_AT {
                self.split();
            }
        }
        previous
    }

    pub(super) fn remove(&mut self, id: &u64) -> Option<V> {
        if self.shards.is_empty() {
            return None;
        }
        let idx = self.shard(*id);
        if !self.shards[idx].contains_key(id) {
            return None;
        }
        let table = Arc::make_mut(&mut self.shards);
        let removed = Arc::make_mut(&mut table[idx]).remove(id);
        if removed.is_some() {
            self.len -= 1;
        }
        removed
    }

    /// Entries in no particular order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (&u64, &V)> + '_ {
        self.shards.iter().flat_map(|shard| shard.iter())
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.iter().map(|(id, _)| *id)
    }

    fn split(&mut self) {
        let single = Arc::make_mut(&mut self.shards).pop().expect("one shard");
        let mut shards: Vec<BTreeMap<u64, V>> = (0..ID_SHARDS).map(|_| BTreeMap::new()).collect();
        for (id, value) in Arc::unwrap_or_clone(single) {
            shards[(id % ID_SHARDS as u64) as usize].insert(id, value);
        }
        self.shards = Arc::new(shards.into_iter().map(Arc::new).collect());
    }

    /// Heap bytes estimate for the memory report.
    pub(super) fn heap_bytes(&self) -> usize {
        self.shards.len() * 40
            + self.len * (std::mem::size_of::<u64>() + std::mem::size_of::<V>() + 12)
    }
}

/// Largest partition of a [`CowOrdMap`] before it splits in two.
const PART_MAX: usize = 1024;

/// An ordered map split into key-range partitions behind `Arc`: a
/// two-level B-tree. Cloning bumps one refcount per partition (about one
/// per thousand entries), and a write copies only the partition holding
/// its key. Unlike [`CowMap`] it keeps key order, so range scans (the
/// sorted property index behind RANGE indexes and uniqueness constraints)
/// work in both directions.
pub(super) struct CowOrdMap<K, V> {
    /// Non-empty partitions, each covering a contiguous key range, in
    /// ascending order.
    parts: Arc<Vec<Arc<BTreeMap<K, V>>>>,
    len: usize,
}

impl<K, V> Default for CowOrdMap<K, V> {
    fn default() -> Self {
        Self {
            parts: Arc::new(Vec::new()),
            len: 0,
        }
    }
}

impl<K, V> Clone for CowOrdMap<K, V> {
    fn clone(&self) -> Self {
        Self {
            parts: Arc::clone(&self.parts),
            len: self.len,
        }
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for CowOrdMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.parts.iter().flat_map(|part| part.iter()))
            .finish()
    }
}

impl<K: Ord + Clone, V: Clone> CowOrdMap<K, V> {
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Index of the partition that holds (or would hold) `key`.
    fn part_for<Q>(&self, key: &Q) -> usize
    where
        K: std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        // First partition whose smallest key is greater than `key`, minus
        // one: the last partition starting at or before `key`.
        let after = self.parts.partition_point(|part| {
            part.keys()
                .next()
                .is_some_and(|first| first.borrow() <= key)
        });
        after.saturating_sub(1)
    }

    pub(super) fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if self.parts.is_empty() {
            return None;
        }
        self.parts[self.part_for(key)].get(key)
    }

    pub(super) fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Mutable access to an existing entry; copies its partition if shared.
    pub(super) fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if !self.contains_key(key) {
            return None;
        }
        let idx = self.part_for(key);
        let parts = Arc::make_mut(&mut self.parts);
        Arc::make_mut(&mut parts[idx]).get_mut(key)
    }

    /// Mutable access to `key`'s value, inserting `make()` first if absent.
    pub(super) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        if !self.contains_key(&key) {
            self.insert_new(key.clone(), make());
        }
        self.get_mut(&key).expect("present after insert")
    }

    fn insert_new(&mut self, key: K, value: V) {
        let parts = Arc::make_mut(&mut self.parts);
        if parts.is_empty() {
            parts.push(Arc::new(BTreeMap::new()));
        }
        let idx = {
            let after =
                parts.partition_point(|part| part.keys().next().is_some_and(|first| *first <= key));
            after.saturating_sub(1)
        };
        let part = Arc::make_mut(&mut parts[idx]);
        part.insert(key, value);
        if part.len() > PART_MAX {
            let middle = part.keys().nth(part.len() / 2).cloned().expect("non-empty");
            let upper = part.split_off(&middle);
            parts.insert(idx + 1, Arc::new(upper));
        }
        self.len += 1;
    }

    pub(super) fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        if !self.contains_key(key) {
            return None;
        }
        let idx = self.part_for(key);
        let parts = Arc::make_mut(&mut self.parts);
        let part = Arc::make_mut(&mut parts[idx]);
        let removed = part.remove(key);
        if part.is_empty() {
            parts.remove(idx);
        }
        self.len -= 1;
        removed
    }

    /// Entries with keys in `(lower, upper)`, in key order (reversible).
    pub(super) fn range(
        &self,
        lower: std::ops::Bound<K>,
        upper: std::ops::Bound<K>,
    ) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        use std::ops::Bound;
        let first = match &lower {
            Bound::Included(k) | Bound::Excluded(k) => self.part_for(k),
            Bound::Unbounded => 0,
        };
        let last = match &upper {
            Bound::Included(k) | Bound::Excluded(k) => self.part_for(k),
            Bound::Unbounded => self.parts.len().saturating_sub(1),
        };
        let parts: &[Arc<BTreeMap<K, V>>] = if self.parts.is_empty() || first > last {
            &[]
        } else {
            &self.parts[first..=last]
        };
        parts
            .iter()
            .flat_map(move |part| part.range((lower.clone(), upper.clone())))
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ {
        self.parts.iter().flat_map(|part| part.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cow_map_behaves_like_a_map_and_shares_on_clone() {
        let mut m: CowMap<String, u32> = CowMap::default();
        for i in 0..500u32 {
            assert!(m.insert(format!("k{i}"), i).is_none());
        }
        assert_eq!(m.insert("k7".into(), 70), Some(7));
        assert_eq!(m.len(), 500);
        let snapshot = m.clone();
        *m.get_mut("k1").unwrap() = 11;
        assert_eq!(m.remove("k2"), Some(2));
        assert_eq!(snapshot.get("k1"), Some(&1));
        assert_eq!(snapshot.get("k2"), Some(&2));
        assert_eq!(m.get("k1"), Some(&11));
        assert!(!m.contains_key("k2"));
        assert_eq!(m.len(), 499);
    }

    #[test]
    fn cow_ord_map_matches_btreemap_in_order_and_ranges() {
        use std::ops::Bound::{Excluded, Included, Unbounded};
        let mut ours: CowOrdMap<u64, u64> = CowOrdMap::default();
        let mut reference = BTreeMap::new();
        // Scrambled insert order exercises splits in the middle.
        for i in 0..5_000u64 {
            let k = (i * 7919) % 5_000;
            *ours.get_or_insert_with(k, || 0) += k;
            reference.insert(k, k);
        }
        let snapshot = ours.clone();
        for k in (0..5_000u64).step_by(3) {
            assert_eq!(ours.remove(&k), reference.remove(&k));
        }
        *ours.get_mut(&1).unwrap() = 99;
        reference.insert(1, 99);
        assert_eq!(ours.len(), reference.len());
        assert!(ours.iter().eq(reference.iter()));
        assert!(ours.iter().rev().eq(reference.iter().rev()));
        for (lo, hi) in [
            (Included(100), Excluded(2_000)),
            (Excluded(4_990), Unbounded),
            (Unbounded, Included(17)),
            (Included(2_500), Included(2_500)),
        ] {
            assert!(ours.range(lo, hi).eq(reference.range((lo, hi))));
            assert!(ours.range(lo, hi).rev().eq(reference.range((lo, hi)).rev()));
        }
        assert_eq!(snapshot.get(&3), Some(&3), "the clone is unaffected");
        assert_eq!(snapshot.len(), 5_000);
    }

    #[test]
    fn cow_id_map_splits_and_stays_correct() {
        let mut m: CowIdMap<u32> = CowIdMap::default();
        for id in 0..5_000u64 {
            m.insert(id, id as u32);
        }
        assert_eq!(m.len(), 5_000);
        let snapshot = m.clone();
        m.insert(42, 0);
        m.remove(&43);
        assert_eq!(snapshot.get(&42), Some(&42));
        assert_eq!(snapshot.get(&43), Some(&43));
        assert_eq!(m.get(&42), Some(&0));
        assert!(!m.contains_key(&43));
        assert_eq!(m.len(), 4_999);
        let mut ids: Vec<u64> = m.keys().collect();
        ids.sort();
        assert_eq!(ids.len(), 4_999);
        assert_eq!(ids[0], 0);
    }
}
