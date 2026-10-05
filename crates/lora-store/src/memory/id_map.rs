//! Posting list with a value per entity: [`CowIdMap`], the entity ids of
//! a trigram, a fulltext term or a point-grid cell, each with its term
//! frequency or point.
//!
//! The same copy-on-write layout as a large [`super::id_set::IdSet`]:
//! entries sorted by id in chunks of ~4 KiB behind `Arc`, grouped up to
//! [`GROUP_MAX`] chunks to a group behind `Arc`, under a root table behind
//! `Arc`, each table keeping its children's largest ids. Cloning is one
//! refcount bump; a write copies the root (one entry per ~16k ids), one
//! group and one chunk, however many entities share the posting. A common
//! trigram or term holds most of the graph, so a flat table (or one hash
//! shard of it) is O(N) per write.
//!
//! Sorted by id rather than hashed: new entities get ascending ids, so an
//! insert is an append to the last chunk, which stays in cache, and
//! readers get ids in order (they build sorted sets and maps from them).
//! Lookups binary-search the largest-id columns and then one chunk.

use std::sync::Arc;

/// `std::sync::Arc<T>`: strong + weak refcount header before the value.
const ARC_HEADER: usize = 2 * std::mem::size_of::<usize>();

/// Target chunks per group. Appends fill the last group to this many
/// chunks before starting a new one.
const GROUP: usize = 64;
/// A group that grows past this (chunk splits in the middle) splits in half.
const GROUP_MAX: usize = 2 * GROUP;
/// A group that shrinks below this merges into a neighbour when the two
/// fit in one group.
const GROUP_MIN: usize = GROUP / 4;

/// A run of entries sorted by id.
type Chunk<V> = Arc<Vec<(u64, V)>>;
/// Chunks in id order, each with its largest id.
type Group<V> = Arc<Vec<(u64, Chunk<V>)>>;
/// Groups in id order, each with its largest id.
type Root<V> = Arc<Vec<(u64, Group<V>)>>;

enum Repr<V> {
    Empty,
    /// Up to one chunk's worth of entries, sorted by id.
    Small(Chunk<V>),
    /// Non-empty groups of non-empty chunks, each with its largest id, in
    /// id order.
    Large(Root<V>),
}

/// Entity ids with a value each, sorted by id (see the module docs).
pub(super) struct CowIdMap<V> {
    repr: Repr<V>,
    len: usize,
}

/// Target entries per chunk: ~4 KiB of them, a copy cheap enough for a
/// write and long enough that the tables above stay short.
const fn chunk_len<V>() -> usize {
    let entry = std::mem::size_of::<(u64, V)>();
    let fit = 4096 / if entry == 0 { 1 } else { entry };
    if fit < 32 {
        32
    } else if fit > 512 {
        512
    } else {
        fit
    }
}

/// Index of the first slot whose largest id is `>= id`, clamped to the
/// last slot: the slot that holds `id` or would receive it.
fn slot_for<X>(slots: &[(u64, X)], id: u64) -> usize {
    slots
        .partition_point(|(max, _)| *max < id)
        .min(slots.len() - 1)
}

/// Fold the undersized slot at `at` into a neighbour when the two fit in
/// `fit` entries, then refresh the largest id of the slot that remains.
fn merge_small<T: Clone>(
    slots: &mut Vec<(u64, Arc<Vec<T>>)>,
    at: usize,
    fit: usize,
    max_of: impl Fn(&[T]) -> u64,
) {
    let small = slots[at].1.len();
    let target = if at + 1 < slots.len() && small + slots[at + 1].1.len() <= fit {
        at
    } else if at > 0 && small + slots[at - 1].1.len() <= fit {
        at - 1
    } else {
        return;
    };
    let (_, next) = slots.remove(target + 1);
    let merged = Arc::make_mut(&mut slots[target].1);
    merged.extend_from_slice(&next);
    let max = max_of(merged);
    slots[target].0 = max;
}

fn chunk_max<V>(chunk: &[(u64, V)]) -> u64 {
    chunk.last().expect("non-empty chunk").0
}

fn group_max<V>(group: &[(u64, Chunk<V>)]) -> u64 {
    group.last().expect("non-empty group").0
}

impl<V> Default for CowIdMap<V> {
    fn default() -> Self {
        Self {
            repr: Repr::Empty,
            len: 0,
        }
    }
}

impl<V> Clone for CowIdMap<V> {
    /// O(1), as for every entry of a copied leaf that holds posting lists.
    fn clone(&self) -> Self {
        let repr = match &self.repr {
            Repr::Empty => Repr::Empty,
            Repr::Small(chunk) => Repr::Small(Arc::clone(chunk)),
            Repr::Large(root) => Repr::Large(Arc::clone(root)),
        };
        Self {
            repr,
            len: self.len,
        }
    }
}

impl<V: std::fmt::Debug> std::fmt::Debug for CowIdMap<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<V> CowIdMap<V> {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entries in ascending id order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (&u64, &V)> + '_ {
        let (small, groups): (&[_], &[_]) = match &self.repr {
            Repr::Empty => (&[], &[]),
            Repr::Small(chunk) => (chunk.as_slice(), &[]),
            Repr::Large(root) => (&[], root.as_slice()),
        };
        small
            .iter()
            .chain(
                groups
                    .iter()
                    .flat_map(|(_, group)| group.iter())
                    .flat_map(|(_, chunk)| chunk.iter()),
            )
            .map(|(id, value)| (id, value))
    }

    /// Ids in ascending order.
    pub(super) fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.iter().map(|(id, _)| *id)
    }

    pub(super) fn get(&self, id: &u64) -> Option<&V> {
        let chunk: &[(u64, V)] = match &self.repr {
            Repr::Empty => return None,
            Repr::Small(chunk) => chunk,
            Repr::Large(root) => {
                let group = &root[slot_for(root, *id)].1;
                &group[slot_for(group, *id)].1
            }
        };
        let at = chunk.binary_search_by_key(id, |(k, _)| *k).ok()?;
        Some(&chunk[at].1)
    }

    pub(super) fn contains_key(&self, id: &u64) -> bool {
        self.get(id).is_some()
    }

    /// Heap bytes for the memory report: the tables and chunks by
    /// capacity. Chunks shared with another graph copy are charged to
    /// both, like the other copy-on-write structures.
    pub(super) fn heap_bytes(&self) -> usize {
        let chunk_bytes = |chunk: &Chunk<V>| {
            ARC_HEADER
                + std::mem::size_of::<Vec<(u64, V)>>()
                + chunk.capacity() * std::mem::size_of::<(u64, V)>()
        };
        match &self.repr {
            Repr::Empty => 0,
            Repr::Small(chunk) => chunk_bytes(chunk),
            Repr::Large(root) => {
                ARC_HEADER
                    + std::mem::size_of::<Vec<(u64, Group<V>)>>()
                    + root.capacity() * std::mem::size_of::<(u64, Group<V>)>()
                    + root
                        .iter()
                        .map(|(_, group)| {
                            ARC_HEADER
                                + std::mem::size_of::<Vec<(u64, Chunk<V>)>>()
                                + group.capacity() * std::mem::size_of::<(u64, Chunk<V>)>()
                                + group
                                    .iter()
                                    .map(|(_, chunk)| chunk_bytes(chunk))
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
            }
        }
    }
}

impl<V: Clone> CowIdMap<V> {
    /// Insert or replace `id`'s value; returns the value it replaced.
    pub(super) fn insert(&mut self, id: u64, value: V) -> Option<V> {
        let cap = chunk_len::<V>();
        match &mut self.repr {
            Repr::Empty => {
                self.repr = Repr::Small(Arc::new(vec![(id, value)]));
                self.len = 1;
                return None;
            }
            Repr::Small(chunk) => {
                match chunk.binary_search_by_key(&id, |(k, _)| *k) {
                    Ok(at) => {
                        let slot = &mut Arc::make_mut(chunk)[at].1;
                        return Some(std::mem::replace(slot, value));
                    }
                    Err(at) if chunk.len() < cap => {
                        Arc::make_mut(chunk).insert(at, (id, value));
                        self.len += 1;
                        return None;
                    }
                    // Full: becomes one chunk of a large map, below.
                    Err(_) => {}
                }
                let chunk = Arc::clone(chunk);
                let max = chunk_max(&chunk);
                let group = Arc::new(vec![(max, chunk)]);
                self.repr = Repr::Large(Arc::new(vec![(max, group)]));
            }
            Repr::Large(_) => {}
        }
        let Repr::Large(root) = &mut self.repr else {
            unreachable!("made large above");
        };
        let replaced = Self::insert_large(root, id, value, cap);
        if replaced.is_none() {
            self.len += 1;
        }
        replaced
    }

    fn insert_large(root: &mut Root<V>, id: u64, value: V, cap: usize) -> Option<V> {
        let (last_max, last_chunks, tail_len) = {
            let (max, group) = root.last().expect("non-empty root");
            let (_, tail) = group.last().expect("non-empty group");
            (*max, group.len(), tail.len())
        };
        if last_max < id {
            // Append: fill the last chunk, then open a new one, and a new
            // group once the last one is full. Every largest id on the
            // way becomes `id`.
            let root = Arc::make_mut(root);
            if last_chunks >= GROUP && tail_len >= cap {
                let chunk = Arc::new(vec![(id, value)]);
                root.push((id, Arc::new(vec![(id, chunk)])));
                return None;
            }
            let (root_max, group) = root.last_mut().expect("non-empty root");
            *root_max = id;
            let group = Arc::make_mut(group);
            if tail_len >= cap {
                group.push((id, Arc::new(vec![(id, value)])));
            } else {
                let (chunk_max, chunk) = group.last_mut().expect("non-empty group");
                *chunk_max = id;
                Arc::make_mut(chunk).push((id, value));
            }
            return None;
        }
        let g = slot_for(root, id);
        let c = slot_for(&root[g].1, id);
        let found = root[g].1[c].1.binary_search_by_key(&id, |(k, _)| *k);
        let root = Arc::make_mut(root);
        let group = Arc::make_mut(&mut root[g].1);
        let chunk = Arc::make_mut(&mut group[c].1);
        let at = match found {
            Ok(at) => return Some(std::mem::replace(&mut chunk[at].1, value)),
            Err(at) => at,
        };
        // `id` is below the chunk's largest id: no largest id changes.
        chunk.insert(at, (id, value));
        if chunk.len() > 2 * cap {
            let upper = chunk.split_off(chunk.len() / 2);
            let lower_max = chunk_max(chunk);
            let upper_max = std::mem::replace(&mut group[c].0, lower_max);
            group.insert(c + 1, (upper_max, Arc::new(upper)));
            if group.len() > GROUP_MAX {
                let upper = group.split_off(group.len() / 2);
                let lower_max = group_max(group);
                let upper_max = std::mem::replace(&mut root[g].0, lower_max);
                root.insert(g + 1, (upper_max, Arc::new(upper)));
            }
        }
        None
    }

    pub(super) fn remove(&mut self, id: &u64) -> Option<V> {
        let cap = chunk_len::<V>();
        let removed = match &mut self.repr {
            Repr::Empty => return None,
            Repr::Small(chunk) => {
                let at = chunk.binary_search_by_key(id, |(k, _)| *k).ok()?;
                Arc::make_mut(chunk).remove(at).1
            }
            Repr::Large(root) => {
                let g = slot_for(root, *id);
                let c = slot_for(&root[g].1, *id);
                let at = root[g].1[c].1.binary_search_by_key(id, |(k, _)| *k).ok()?;
                let root = Arc::make_mut(root);
                let group = Arc::make_mut(&mut root[g].1);
                let chunk = Arc::make_mut(&mut group[c].1);
                let (_, removed) = chunk.remove(at);
                let left = chunk.last().map(|(max, _)| (*max, chunk.len()));
                match left {
                    None => {
                        group.remove(c);
                    }
                    Some((max, len)) => {
                        group[c].0 = max;
                        if len < cap / 8 {
                            merge_small(group, c, cap, chunk_max);
                        }
                    }
                }
                let left = group.last().map(|(max, _)| (*max, group.len()));
                match left {
                    None => {
                        root.remove(g);
                    }
                    Some((max, len)) => {
                        root[g].0 = max;
                        if len < GROUP_MIN {
                            merge_small(root, g, GROUP, group_max);
                        }
                    }
                }
                removed
            }
        };
        self.len -= 1;
        if self.len == 0 {
            self.repr = Repr::Empty;
        }
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Deterministic xorshift so the model test needs no extra crate.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// Chunks and groups are non-empty, within bounds, sorted, and carry
    /// their true largest ids.
    fn check<V: Clone>(m: &CowIdMap<V>) {
        let cap = chunk_len::<V>();
        let mut prev: Option<u64> = None;
        let mut n = 0;
        let mut visit = |chunk: &[(u64, V)]| {
            assert!(!chunk.is_empty() && chunk.len() <= 2 * cap);
            for (id, _) in chunk {
                assert!(prev.is_none_or(|p| p < *id), "out of order");
                prev = Some(*id);
                n += 1;
            }
        };
        match &m.repr {
            Repr::Empty => {}
            Repr::Small(chunk) => {
                assert!(chunk.len() <= cap);
                visit(chunk);
            }
            Repr::Large(root) => {
                assert!(!root.is_empty());
                for (max, group) in root.iter() {
                    assert!(!group.is_empty() && group.len() <= GROUP_MAX);
                    assert_eq!(*max, group_max(group), "group largest id");
                    for (max, chunk) in group.iter() {
                        assert_eq!(*max, chunk_max(chunk), "chunk largest id");
                        visit(chunk);
                    }
                }
            }
        }
        assert_eq!(n, m.len());
        assert_eq!(m.is_empty(), matches!(m.repr, Repr::Empty));
    }

    #[test]
    fn matches_a_btreemap_through_appends_splits_and_merges() {
        let mut seed = 0x2545_f491_4f6c_dd1d;
        let mut model: BTreeMap<u64, u32> = BTreeMap::new();
        let mut m: CowIdMap<u32> = CowIdMap::default();
        let mut snapshots = Vec::new();
        // Appends to ~100k ids (several groups), random inserts that split
        // chunks and groups in the middle, a delete-heavy phase that merges
        // them, then appends again.
        for step in 0..400_000u64 {
            let roll = rng(&mut seed) % 100;
            if step < 100_000 {
                let id = model.last_key_value().map_or(0, |(k, _)| k + 1 + roll % 3);
                assert_eq!(m.insert(id, step as u32), model.insert(id, step as u32));
            } else if step < 200_000 {
                let id = rng(&mut seed) % 400_000;
                assert_eq!(m.insert(id, step as u32), model.insert(id, step as u32));
            } else if step < 350_000 {
                let id = rng(&mut seed) % 400_000;
                assert_eq!(m.remove(&id), model.remove(&id));
            } else {
                let id = model.last_key_value().map_or(0, |(k, _)| k + 1);
                assert_eq!(m.insert(id, 7), model.insert(id, 7));
            }
            if step % 19_997 == 0 {
                check(&m);
                assert!(m.iter().map(|(k, v)| (*k, *v)).eq(model.clone()));
                for probe in 0..50 {
                    let id = rng(&mut seed) % 400_000 + probe;
                    assert_eq!(m.get(&id), model.get(&id));
                }
                snapshots.push((m.clone(), model.clone()));
            }
        }
        check(&m);
        assert!(matches!(m.repr, Repr::Large(_)));
        for (snap, want) in &snapshots {
            check(snap);
            assert!(snap.iter().map(|(k, v)| (*k, *v)).eq(want.clone()));
        }
        // Down to nothing.
        let ids: Vec<u64> = model.keys().copied().collect();
        for id in ids {
            assert!(m.remove(&id).is_some());
            assert_eq!(m.remove(&id), None);
        }
        assert!(m.is_empty() && matches!(m.repr, Repr::Empty));
        assert_eq!(m.heap_bytes(), 0);
    }

    #[test]
    fn small_maps_are_one_chunk_and_grow_into_groups() {
        let cap = chunk_len::<u32>() as u64;
        let mut m: CowIdMap<u32> = CowIdMap::default();
        assert_eq!(m.heap_bytes(), 0);
        for id in (0..cap).rev() {
            assert_eq!(m.insert(id * 2, 1), None);
        }
        assert!(matches!(m.repr, Repr::Small(_)));
        assert_eq!(m.insert(4, 9), Some(1));
        assert_eq!(m.insert(5, 1), None, "past a chunk: large");
        assert!(matches!(m.repr, Repr::Large(_)));
        check(&m);
        assert_eq!(m.get(&4), Some(&9));
        assert_eq!(m.get(&5), Some(&1));
        assert_eq!(m.get(&3), None);
        assert!(m.keys().eq((0..cap)
            .map(|i| i * 2)
            .chain([5])
            .collect::<std::collections::BTreeSet<_>>()));
    }

    #[test]
    fn a_write_to_a_shared_large_map_copies_one_group_and_one_chunk() {
        let mut base: CowIdMap<u32> = CowIdMap::default();
        for id in 0..2_000_000u64 {
            base.insert(id * 2, 1);
        }
        let Repr::Large(root) = &base.repr else {
            panic!("expected a large map");
        };
        assert!(root.len() >= 16, "{} groups", root.len());
        let groups: Vec<_> = root.iter().map(|(_, g)| Arc::as_ptr(g)).collect();
        let chunks: Vec<_> = root
            .iter()
            .flat_map(|(_, g)| g.iter().map(|(_, c)| Arc::as_ptr(c)))
            .collect();

        for write in 0..3 {
            let mut staged = base.clone();
            match write {
                0 => assert_eq!(staged.insert(1_000_001, 5), None),
                1 => assert_eq!(staged.remove(&1_000_000), Some(1)),
                _ => assert_eq!(staged.insert(1_000_000, 5), Some(1)),
            }
            let Repr::Large(root) = &staged.repr else {
                panic!("expected a large map");
            };
            let new_groups = root
                .iter()
                .filter(|(_, g)| !groups.contains(&Arc::as_ptr(g)))
                .count();
            let new_chunks = root
                .iter()
                .flat_map(|(_, g)| g.iter())
                .filter(|(_, c)| !chunks.contains(&Arc::as_ptr(c)))
                .count();
            assert_eq!((new_groups, new_chunks), (1, 1), "write {write}");
            check(&staged);
        }
        assert_eq!(base.len(), 2_000_000);
        assert_eq!(base.get(&1_000_000), Some(&1));
        assert_eq!(base.get(&1_000_001), None);
    }
}
