//! The string keys of a sorted property index scope, packed (G-64).
//!
//! A sorted index kept each string key as its own `Arc<str>` beside a
//! 32-byte entry: a 58-byte key cost about 110 bytes, twice the bytes of
//! the key. Here the keys of up to [`BLOCK_MAX`] neighbours share one
//! [`StrBlock`]: their common prefix once, then each key's suffix, an
//! end offset and its ids. Keys that share a namespace
//! (`musicbrainz:release-group:…`) cost their suffix and 12 bytes.
//!
//! ```text
//! blocks: CowOrdMap<first key, Arc<StrBlock>>
//! block:  bytes   prefix | suffix 0 | suffix 1 | ...
//!         ends    end of suffix 0, end of suffix 1, ...
//!         ids     IdSet 0, IdSet 1, ...
//! ```
//!
//! The blocks sit in a [`CowOrdMap`] keyed by their smallest key, so a
//! clone of the index shares them and a write copies the path to one
//! block and that block. Key order is byte order, as for `str`.

use std::cmp::Ordering;
use std::ops::Bound;
use std::sync::Arc;

use super::cow::CowOrdMap;
use super::id_set::IdSet;
use super::ARC_HEADER;

/// Keys per block before it splits in two. A write to a shared block
/// copies it whole: about 3 KB at 64 keys of 40 bytes.
const BLOCK_MAX: usize = 64;
/// Blocks per partition of the block map.
const BLOCKS_PER_PART: usize = 64;

/// A key in a block: its block's prefix, then its own suffix.
#[derive(Clone, Copy, Debug)]
pub(super) struct StrKey<'a> {
    prefix: &'a [u8],
    suffix: &'a [u8],
}

impl StrKey<'_> {
    pub(super) fn eq_str(&self, other: &str) -> bool {
        let other = other.as_bytes();
        other.len() == self.prefix.len() + self.suffix.len()
            && other.starts_with(self.prefix)
            && &other[self.prefix.len()..] == self.suffix
    }

    /// The key whole.
    pub(super) fn to_arc(self) -> Arc<str> {
        let mut bytes = Vec::with_capacity(self.prefix.len() + self.suffix.len());
        bytes.extend_from_slice(self.prefix);
        bytes.extend_from_slice(self.suffix);
        // Keys go in as `&str` and come out as the same bytes.
        Arc::from(String::from_utf8(bytes).expect("keys are whole strings"))
    }

    #[cfg(test)]
    fn into_string(self) -> String {
        let mut bytes = self.prefix.to_vec();
        bytes.extend_from_slice(self.suffix);
        String::from_utf8(bytes).expect("keys are whole strings")
    }
}

/// Up to [`BLOCK_MAX`] neighbouring keys and their ids, in key order.
#[derive(Clone, Debug, Default)]
struct StrBlock {
    /// The common prefix (`prefix` bytes), then each key's suffix.
    bytes: Vec<u8>,
    prefix: u32,
    /// End of each key's suffix in `bytes`.
    ends: Vec<u32>,
    ids: Vec<IdSet>,
}

/// Length of the common prefix of `a` and `b`.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

impl StrBlock {
    /// A block of `keys` (strictly ascending) and their ids.
    fn packed<'k>(keys: impl ExactSizeIterator<Item = &'k [u8]> + Clone, ids: Vec<IdSet>) -> Self {
        let len = keys.len();
        debug_assert_eq!(len, ids.len());
        let mut first_last = keys.clone();
        let prefix = match (first_last.next(), first_last.last()) {
            (Some(first), Some(last)) => common_prefix(first, last),
            // One key: all of it is the prefix.
            (Some(only), None) => only.len(),
            _ => 0,
        };
        let total: usize = keys.clone().map(|k| k.len() - prefix).sum();
        let mut bytes = Vec::with_capacity(prefix + total);
        let mut ends = Vec::with_capacity(len);
        for (i, key) in keys.enumerate() {
            if i == 0 {
                bytes.extend_from_slice(&key[..prefix]);
            }
            bytes.extend_from_slice(&key[prefix..]);
            ends.push(bytes.len() as u32);
        }
        StrBlock {
            bytes,
            prefix: prefix as u32,
            ends,
            ids,
        }
    }

    fn len(&self) -> usize {
        self.ends.len()
    }

    fn prefix(&self) -> &[u8] {
        &self.bytes[..self.prefix as usize]
    }

    fn start(&self, i: usize) -> usize {
        if i == 0 {
            self.prefix as usize
        } else {
            self.ends[i - 1] as usize
        }
    }

    fn suffix(&self, i: usize) -> &[u8] {
        &self.bytes[self.start(i)..self.ends[i] as usize]
    }

    fn key(&self, i: usize) -> StrKey<'_> {
        StrKey {
            prefix: self.prefix(),
            suffix: self.suffix(i),
        }
    }

    /// Key `i` whole.
    fn owned_key(&self, i: usize) -> Arc<str> {
        self.key(i).to_arc()
    }

    /// Where `key` is, or would go.
    fn search(&self, key: &[u8]) -> Result<usize, usize> {
        let prefix = self.prefix();
        if !key.starts_with(prefix) {
            // Every key here starts with `prefix`: `key` sorts before all
            // of them or after all of them.
            return Err(if key < prefix { 0 } else { self.len() });
        }
        let rest = &key[prefix.len()..];
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.suffix(mid).cmp(rest) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(mid),
            }
        }
        Err(lo)
    }

    /// The first position whose key is not below `bound` (a lower bound),
    /// or past every key `bound` admits (an upper bound).
    fn lower(&self, bound: Bound<&str>) -> usize {
        match bound {
            Bound::Unbounded => 0,
            Bound::Included(k) => self.search(k.as_bytes()).unwrap_or_else(|i| i),
            Bound::Excluded(k) => match self.search(k.as_bytes()) {
                Ok(i) => i + 1,
                Err(i) => i,
            },
        }
    }

    fn upper(&self, bound: Bound<&str>) -> usize {
        match bound {
            Bound::Unbounded => self.len(),
            Bound::Included(k) => match self.search(k.as_bytes()) {
                Ok(i) => i + 1,
                Err(i) => i,
            },
            Bound::Excluded(k) => self.search(k.as_bytes()).unwrap_or_else(|i| i),
        }
    }

    /// Insert `key` at `at` (from [`Self::search`]) with `ids`.
    fn insert_at(&mut self, at: usize, key: &[u8], ids: IdSet) {
        if !key.starts_with(self.prefix()) {
            self.shorten_prefix(common_prefix(self.prefix(), key));
        }
        let rest = &key[self.prefix as usize..];
        let start = self.start(at);
        self.bytes.splice(start..start, rest.iter().copied());
        for end in &mut self.ends[at..] {
            *end += rest.len() as u32;
        }
        self.ends.insert(at, (start + rest.len()) as u32);
        self.ids.insert(at, ids);
    }

    /// Keep only `len` bytes of the prefix, moving the rest into every
    /// suffix.
    fn shorten_prefix(&mut self, len: usize) {
        let keys: Vec<Vec<u8>> = (0..self.len())
            .map(|i| [self.prefix(), self.suffix(i)].concat())
            .collect();
        let ids = std::mem::take(&mut self.ids);
        let mut bytes =
            Vec::with_capacity(self.bytes.len() + (self.prefix as usize - len) * keys.len());
        bytes.extend_from_slice(&self.bytes[..len]);
        let mut ends = Vec::with_capacity(keys.len() + 1);
        for key in &keys {
            bytes.extend_from_slice(&key[len..]);
            ends.push(bytes.len() as u32);
        }
        *self = StrBlock {
            bytes,
            prefix: len as u32,
            ends,
            ids,
        };
    }

    fn remove_at(&mut self, at: usize) -> IdSet {
        let (start, end) = (self.start(at), self.ends[at] as usize);
        self.bytes.drain(start..end);
        self.ends.remove(at);
        for e in &mut self.ends[at..] {
            *e -= (end - start) as u32;
        }
        self.ids.remove(at)
    }

    /// Move the keys from `at` on into a block of their own. Both halves
    /// are repacked: each gets the longest prefix its keys share, and no
    /// spare capacity.
    fn split_off(&mut self, at: usize) -> StrBlock {
        let keys: Vec<Vec<u8>> = (0..self.len())
            .map(|i| [self.prefix(), self.suffix(i)].concat())
            .collect();
        let mut ids = std::mem::take(&mut self.ids);
        let upper_ids = ids.split_off(at);
        let (low, high) = keys.split_at(at);
        *self = StrBlock::packed(low.iter().map(Vec::as_slice), ids);
        StrBlock::packed(high.iter().map(Vec::as_slice), upper_ids)
    }

    fn heap_bytes(&self) -> usize {
        self.bytes.capacity()
            + self.ends.capacity() * std::mem::size_of::<u32>()
            + self.ids.capacity() * std::mem::size_of::<IdSet>()
            + self.ids.iter().map(IdSet::heap_bytes).sum::<usize>()
    }
}

/// Builds [`StrPostings`] from keys in strictly ascending order, a block
/// at a time: full blocks, as a bulk build leaves them.
#[derive(Default)]
pub(super) struct StrPostingsBuilder<'k> {
    blocks: Vec<(Arc<str>, Arc<StrBlock>)>,
    keys: Vec<&'k str>,
    ids: Vec<IdSet>,
    len: usize,
}

impl<'k> StrPostingsBuilder<'k> {
    pub(super) fn push(&mut self, key: &'k str, ids: IdSet) {
        debug_assert!(self.keys.last().is_none_or(|last| *last < key));
        if self.keys.len() == BLOCK_MAX {
            self.close();
        }
        self.keys.push(key);
        self.ids.push(ids);
        self.len += 1;
    }

    fn close(&mut self) {
        let block = StrBlock::packed(
            self.keys.iter().map(|k| k.as_bytes()),
            std::mem::replace(&mut self.ids, Vec::with_capacity(BLOCK_MAX)),
        );
        self.blocks.push((Arc::from(self.keys[0]), Arc::new(block)));
        self.keys.clear();
    }

    pub(super) fn finish(mut self) -> StrPostings {
        if !self.keys.is_empty() {
            self.close();
        }
        StrPostings {
            blocks: CowOrdMap::from_sorted(self.blocks),
            len: self.len,
        }
    }
}

/// String keys and their ids, in key order (see the module docs).
#[derive(Clone, Debug, Default)]
pub(super) struct StrPostings {
    blocks: CowOrdMap<Arc<str>, Arc<StrBlock>, BLOCKS_PER_PART>,
    len: usize,
}

impl StrPostings {
    #[cfg(test)]
    /// Keys held.
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Postings of `keys` and their ids, keys strictly ascending: full
    /// blocks, as a bulk build leaves them.
    #[cfg(test)]
    pub(super) fn from_sorted<'k>(entries: impl IntoIterator<Item = (&'k str, IdSet)>) -> Self {
        let mut builder = StrPostingsBuilder::default();
        for (key, ids) in entries {
            builder.push(key, ids);
        }
        builder.finish()
    }

    /// The block holding `key`, or the one it would go to: the last
    /// whose first key is `<= key`, else the first.
    fn block_for(&self, key: &str) -> Option<&Arc<str>> {
        self.blocks
            .floor(key)
            .or_else(|| self.blocks.iter().next())
            .map(|(first, _)| first)
    }

    #[cfg(test)]
    pub(super) fn get(&self, key: &str) -> Option<&IdSet> {
        let (_, block) = self.blocks.floor(key)?;
        let at = block.search(key.as_bytes()).ok()?;
        Some(&block.ids[at])
    }

    /// File `block` under `first`, a key no block has.
    fn put(&mut self, first: Arc<str>, block: Arc<StrBlock>) {
        debug_assert!(!self.blocks.contains_key(&*first));
        self.blocks.get_or_insert_with(first, || block);
    }

    /// Add `id` under `key`.
    pub(super) fn insert(&mut self, key: &str, id: u64) {
        let Some(first) = self.block_for(key).cloned() else {
            let block = StrBlock::packed(std::iter::once(key.as_bytes()), vec![IdSet::new(id)]);
            self.put(Arc::from(key), Arc::new(block));
            self.len = 1;
            return;
        };
        let block = Arc::make_mut(self.blocks.get_mut(&*first).expect("found above"));
        let at = match block.search(key.as_bytes()) {
            Ok(at) => {
                block.ids[at].insert(id);
                return;
            }
            Err(at) => at,
        };
        block.insert_at(at, key.as_bytes(), IdSet::new(id));
        self.len += 1;
        let split = (block.len() > BLOCK_MAX).then(|| block.split_off(block.len() / 2));
        if at == 0 {
            // A new smallest key: the block moves to it.
            let block = self.blocks.remove(&*first).expect("found above");
            self.put(Arc::from(key), block);
        }
        if let Some(upper) = split {
            self.put(upper.owned_key(0), Arc::new(upper));
        }
    }

    /// Remove `id` from under `key`. Returns whether `key` was held and
    /// now has no ids.
    pub(super) fn remove(&mut self, key: &str, id: u64) -> bool {
        let Some((first, block)) = self.blocks.floor(key) else {
            return false;
        };
        let Ok(at) = block.search(key.as_bytes()) else {
            return false;
        };
        let first = first.clone();
        let block = Arc::make_mut(self.blocks.get_mut(&*first).expect("found above"));
        if !block.ids[at].remove(id) {
            return false;
        }
        block.remove_at(at);
        self.len -= 1;
        if block.len() == 0 {
            self.blocks.remove(&*first);
        } else if at == 0 {
            let moved = block.owned_key(0);
            let block = self.blocks.remove(&*first).expect("found above");
            self.put(moved, block);
        }
        true
    }

    /// Keys in `(lower, upper)` and their ids, in key order (reversible).
    pub(super) fn range<'a>(
        &'a self,
        lower: Bound<&'a str>,
        upper: Bound<&'a str>,
    ) -> impl DoubleEndedIterator<Item = (StrKey<'a>, &'a IdSet)> + 'a {
        // From the block that would hold `lower` to the last whose first
        // key is within `upper`; the first and last are cut to the range.
        let from = match lower {
            Bound::Included(k) | Bound::Excluded(k) => self
                .blocks
                .floor(k)
                .map_or(Bound::Unbounded, |(first, _)| Bound::Included(&**first)),
            Bound::Unbounded => Bound::Unbounded,
        };
        let to = match upper {
            Bound::Included(k) | Bound::Excluded(k) => Bound::Included(k),
            Bound::Unbounded => Bound::Unbounded,
        };
        let empty = match (lower, upper) {
            (Bound::Included(l), Bound::Included(u)) => l > u,
            (Bound::Included(l) | Bound::Excluded(l), Bound::Included(u) | Bound::Excluded(u)) => {
                l >= u
            }
            _ => false,
        };
        let blocks = (!empty).then(|| self.blocks.range_by(from, to));
        blocks.into_iter().flatten().flat_map(move |(_, block)| {
            let start = block.lower(lower);
            let end = block.upper(upper).max(start);
            (start..end).map(move |i| (block.key(i), &block.ids[i]))
        })
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = (StrKey<'_>, &IdSet)> + '_ {
        self.range(Bound::Unbounded, Bound::Unbounded)
    }

    /// Heap bytes, for the memory report: the blocks and the map above
    /// them (its first keys are counted once each).
    pub(super) fn heap_bytes(&self) -> usize {
        self.blocks.structure_bytes()
            + self.blocks.entry_slots() * std::mem::size_of::<(Arc<str>, Arc<StrBlock>)>()
            + self
                .blocks
                .iter()
                .map(|(first, block)| {
                    ARC_HEADER
                        + first.len()
                        + ARC_HEADER
                        + std::mem::size_of::<StrBlock>()
                        + block.heap_bytes()
                })
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    /// Keys sharing namespaces, with multi-byte characters a prefix can
    /// split, the empty string and keys that are prefixes of others.
    fn key(seed: &mut u64) -> String {
        let spaces = [
            "",
            "a",
            "musicbrainz:release-group:",
            "musicbrainz:artist:",
            "wikidata:item:Q",
            "é",
            "éa",
            "日本:",
        ];
        let space = spaces[(rng(seed) % spaces.len() as u64) as usize];
        let tail = rng(seed) % 600;
        match rng(seed) % 4 {
            0 => space.to_string(),
            1 => format!("{space}{tail}"),
            2 => format!("{space}{tail:x}-0000-ßé"),
            _ => format!("{space}{}", "z".repeat((tail % 7) as usize)),
        }
    }

    type Model = BTreeMap<String, BTreeSet<u64>>;

    fn check(p: &StrPostings, model: &Model, seed: &mut u64) {
        assert_eq!(p.len(), model.len());
        let all: Vec<(String, Vec<u64>)> = p
            .iter()
            .map(|(k, ids)| (k.into_string(), ids.to_vec()))
            .collect();
        let want: Vec<(String, Vec<u64>)> = model
            .iter()
            .map(|(k, ids)| (k.clone(), ids.iter().copied().collect()))
            .collect();
        assert_eq!(all, want);
        let rev: Vec<String> = p.iter().rev().map(|(k, _)| k.into_string()).collect();
        assert_eq!(rev, model.keys().rev().cloned().collect::<Vec<_>>());
        for _ in 0..20 {
            let (a, b) = (key(seed), key(seed));
            let bound = |k: &String, kind: u64| match kind {
                0 => Bound::Unbounded,
                1 => Bound::Included(k.clone()),
                _ => Bound::Excluded(k.clone()),
            };
            let (lo, hi) = (bound(&a, rng(seed) % 3), bound(&b, rng(seed) % 3));
            fn as_ref(b: &Bound<String>) -> Bound<&str> {
                match b {
                    Bound::Unbounded => Bound::Unbounded,
                    Bound::Included(k) => Bound::Included(k.as_str()),
                    Bound::Excluded(k) => Bound::Excluded(k.as_str()),
                }
            }
            let got: Vec<String> = p
                .range(as_ref(&lo), as_ref(&hi))
                .map(|(k, _)| k.into_string())
                .collect();
            let valid = match (&lo, &hi) {
                (Bound::Included(l), Bound::Included(u)) => l <= u,
                (
                    Bound::Included(l) | Bound::Excluded(l),
                    Bound::Included(u) | Bound::Excluded(u),
                ) => l < u,
                _ => true,
            };
            let want: Vec<String> = if valid {
                model
                    .range((lo.clone(), hi.clone()))
                    .map(|(k, _)| k.clone())
                    .collect()
            } else {
                Vec::new()
            };
            assert_eq!(got, want, "range {lo:?} {hi:?}");
            let got_rev: Vec<String> = p
                .range(as_ref(&lo), as_ref(&hi))
                .rev()
                .map(|(k, _)| k.into_string())
                .collect();
            assert_eq!(got_rev, want.iter().rev().cloned().collect::<Vec<_>>());
            assert_eq!(
                p.get(&a).map(IdSet::to_vec),
                model.get(&a).map(|s| s.iter().copied().collect())
            );
            assert!(p
                .get(&a)
                .is_none_or(|_| p.iter().any(|(k, _)| k.eq_str(&a))));
        }
    }

    #[test]
    fn matches_a_btree_map_through_inserts_removes_and_clones() {
        for start in [1u64, 7, 99, 12345] {
            let mut seed = start;
            let mut p = StrPostings::default();
            let mut model = Model::new();
            let mut saved: Option<(StrPostings, Model)> = None;
            for step in 0..4000 {
                let k = key(&mut seed);
                let id = rng(&mut seed) % 5;
                if rng(&mut seed).is_multiple_of(3) {
                    let emptied = p.remove(&k, id);
                    let held = model.get_mut(&k).is_some_and(|ids| ids.remove(&id));
                    let now_empty = held && model[&k].is_empty();
                    if now_empty {
                        model.remove(&k);
                    }
                    assert_eq!(emptied, now_empty, "remove {k:?} {id}");
                } else {
                    p.insert(&k, id);
                    model.entry(k).or_default().insert(id);
                }
                if step % 500 == 0 {
                    check(&p, &model, &mut seed);
                    // A clone shares blocks; writes after it must not
                    // show through.
                    if let Some((old, old_model)) = saved.take() {
                        check(&old, &old_model, &mut seed);
                    }
                    saved = Some((p.clone(), model.clone()));
                }
            }
            check(&p, &model, &mut seed);
            let rebuilt = StrPostings::from_sorted(model.iter().map(|(k, ids)| {
                let mut it = ids.iter();
                let mut set = IdSet::new(*it.next().unwrap());
                for id in it {
                    set.insert(*id);
                }
                (k.as_str(), set)
            }));
            check(&rebuilt, &model, &mut seed);
        }
    }

    #[test]
    fn keys_in_one_namespace_cost_their_suffix() {
        let keys: Vec<String> = (0..10_000)
            .map(|i| format!("musicbrainz:release-group:{i:08}-0000-0000-0000-000000000000"))
            .collect();
        let p = StrPostings::from_sorted(
            keys.iter()
                .enumerate()
                .map(|(i, k)| (k.as_str(), IdSet::new(i as u64))),
        );
        let per_key = p.heap_bytes() as f64 / keys.len() as f64;
        // 58-byte keys: about 34 bytes of suffix, 12 of offset and ids.
        assert!(per_key < 55.0, "{per_key} B per key");
    }
}
