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
//! Buckets grow through a sorted `Vec` and switch to a chunked
//! copy-on-write list past [`SMALL_MAX`]: sorted runs of at most
//! [`CHUNK_MAX`] ids, each behind an `Arc`, grouped up to [`GROUP_MAX`]
//! chunks to a group behind an `Arc`, under a root table of groups behind
//! an `Arc`. Low-cardinality values (an enum, a boolean, a default) put
//! most of the graph in one bucket, and the index maps are copy-on-write,
//! so a write that lands in a shared bucket copies it. Chunked, that copy
//! is the root (one pointer per ~32k ids), one group (at most
//! [`GROUP_MAX`] pointers) and the one chunk the id lands in, not every id
//! in the bucket, nor (as with a flat chunk table) one pointer per 512 of
//! them; cloning the bucket itself (when its map leaf is copied) is one
//! refcount bump. Inserts and removes find their chunk by binary search,
//! so they stay O(log n) plus a shift inside one chunk.
//!
//! Ids are kept sorted and unique in every representation, so lookups
//! return candidates in ascending id order, the same order a label scan
//! produces.

use super::ARC_HEADER;
use std::ptr::NonNull;
use std::sync::Arc;

/// Largest bucket kept as a sorted vector. Past this a mid-vector
/// insert or remove shifts more memory than a chunk walk costs.
const SMALL_MAX: usize = 64;

/// Target ids per chunk. Appends (ids are allocated monotonically) fill
/// the last chunk to this size before starting a new one.
const CHUNK: usize = 512;
/// A chunk that grows past this (inserts in the middle) splits in half.
const CHUNK_MAX: usize = 2 * CHUNK;
/// A chunk that shrinks below this merges into a neighbour when the two
/// fit in one chunk, so mass deletes do not leave a table of tiny chunks.
const CHUNK_MIN: usize = CHUNK / 8;

/// Target chunks per group. Appends fill the last group to this many
/// chunks before starting a new one.
const GROUP: usize = 64;
/// A group that grows past this (chunk splits in the middle) splits in half.
const GROUP_MAX: usize = 2 * GROUP;
/// A group that shrinks below this merges into a neighbour when the two
/// fit in one group.
const GROUP_MIN: usize = GROUP / 4;

type Chunk = Arc<Vec<u64>>;
/// Non-empty chunks in ascending order.
type Group = Arc<Vec<Chunk>>;

/// The chunks of a set too large for a sorted vector: non-empty groups of
/// non-empty sorted chunks in ascending order, and the count of all ids.
#[derive(Debug, Clone)]
struct Large {
    groups: Vec<Group>,
    len: usize,
}

/// An [`IdSet`] taken apart, for code that changes it.
#[derive(Debug)]
enum Unpacked {
    One(u64),
    /// Boxed: the packed set points at the `Vec` with one thin pointer.
    #[allow(clippy::box_collection)]
    Small(Box<Vec<u64>>),
    /// Shared: cloning the set (when its index leaf is copied) is one
    /// refcount bump, and a write copies the root table on first touch.
    Large(Arc<Large>),
}

/// An [`IdSet`] read in place.
#[derive(Debug)]
enum View<'a> {
    One(u64),
    Small(&'a Vec<u64>),
    Large(&'a Large),
}

/// The ids that share one indexed value, in one word: an index holds a
/// set per distinct value, and for a unique key every one of them holds
/// a single id. The word is that id shifted left with the low bit set,
/// or a pointer to a sorted vector (low bits clear) or to the shared
/// chunks of a large set (bit 1 set). An enum of the three took two
/// words. An id too large to shift (2^63 and up on 64-bit targets, 2^31
/// on 32-bit ones) is kept as a vector of one.
pub(super) struct IdSet {
    ptr: NonNull<u8>,
}

/// Low bit of an inline id.
const ONE: usize = 0b01;
/// Tag bit of a pointer to [`Large`].
const LARGE: usize = 0b10;
/// Largest id kept inline.
const INLINE_MAX: u64 = (usize::MAX >> 1) as u64;

// Both pointees are 8-aligned, which leaves the tag bits clear.
const _: () = assert!(std::mem::align_of::<Vec<u64>>() >= 4 && std::mem::align_of::<Large>() >= 4);
const _: () = assert!(std::mem::size_of::<IdSet>() == std::mem::size_of::<usize>());
const _: () = assert!(std::mem::size_of::<Option<IdSet>>() == std::mem::size_of::<usize>());

// SAFETY: an `IdSet` owns a `Box<Vec<u64>>` or shares an `Arc<Large>`,
// both `Send + Sync`, and mutation needs `&mut IdSet`.
unsafe impl Send for IdSet {}
unsafe impl Sync for IdSet {}

impl IdSet {
    fn pack(set: Unpacked) -> Self {
        let ptr = match set {
            Unpacked::One(id) if id <= INLINE_MAX => {
                std::ptr::without_provenance_mut::<u8>(((id as usize) << 1) | ONE)
            }
            Unpacked::One(id) => Box::into_raw(Box::new(vec![id])).cast::<u8>(),
            Unpacked::Small(ids) => Box::into_raw(ids).cast::<u8>(),
            Unpacked::Large(large) => Arc::into_raw(large)
                .cast_mut()
                .cast::<u8>()
                .map_addr(|addr| addr | LARGE),
        };
        Self {
            ptr: NonNull::new(ptr).expect("a tagged id or an allocation"),
        }
    }

    #[inline]
    fn view(&self) -> View<'_> {
        let ptr = self.ptr.as_ptr();
        let addr = ptr.addr();
        if addr & ONE != 0 {
            View::One((addr >> 1) as u64)
        } else if addr & LARGE != 0 {
            // SAFETY: packed from `Arc::into_raw`, and the reference the
            // set holds keeps it alive while `self` is borrowed.
            View::Large(unsafe { &*ptr.map_addr(|a| a & !LARGE).cast::<Large>() })
        } else {
            // SAFETY: packed from `Box::into_raw`, owned by `self`.
            View::Small(unsafe { &*ptr.cast::<Vec<u64>>() })
        }
    }

    fn unpack(self) -> Unpacked {
        let ptr = std::mem::ManuallyDrop::new(self).ptr.as_ptr();
        let addr = ptr.addr();
        if addr & ONE != 0 {
            Unpacked::One((addr >> 1) as u64)
        } else if addr & LARGE != 0 {
            // SAFETY: packed from `Arc::into_raw`; this takes back the
            // reference `self` held, and `self` is not dropped.
            Unpacked::Large(unsafe { Arc::from_raw(ptr.map_addr(|a| a & !LARGE).cast::<Large>()) })
        } else {
            // SAFETY: packed from `Box::into_raw`; ownership moves back.
            Unpacked::Small(unsafe { Box::from_raw(ptr.cast::<Vec<u64>>()) })
        }
    }

    /// Change the set through its unpacked form.
    #[inline]
    fn update<R>(&mut self, change: impl FnOnce(&mut Unpacked) -> R) -> R {
        let mut set = std::mem::replace(self, Self::pack(Unpacked::One(0))).unpack();
        let result = change(&mut set);
        *self = Self::pack(set);
        result
    }
}

impl Clone for IdSet {
    fn clone(&self) -> Self {
        match self.view() {
            View::One(_) => Self { ptr: self.ptr },
            View::Small(ids) => Self::pack(Unpacked::Small(Box::new(ids.clone()))),
            View::Large(_) => {
                // The count sits before the `Large` in the `Arc`'s
                // allocation, so the pointer comes from the packed one,
                // whose provenance covers it, not from the `&Large`.
                let large = self.ptr.as_ptr().map_addr(|a| a & !LARGE).cast::<Large>();
                // SAFETY: packed from `Arc::into_raw` and alive while `self`
                // is; the new reference is owned by the returned copy.
                unsafe { Arc::increment_strong_count(large) };
                Self { ptr: self.ptr }
            }
        }
    }
}

impl Drop for IdSet {
    fn drop(&mut self) {
        if self.ptr.as_ptr().addr() & ONE == 0 {
            // `unpack` takes the allocation back without dropping the
            // copy it consumes, so this frees it exactly once.
            drop(Self { ptr: self.ptr }.unpack());
        }
    }
}

impl std::fmt::Debug for IdSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.view().fmt(f)
    }
}

impl PartialEq for IdSet {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl Eq for IdSet {}

/// Index of the chunk that holds `id` or would receive it: the first
/// chunk whose last id is `>= id`, clamped to the last chunk.
fn chunk_for(chunks: &[Chunk], id: u64) -> usize {
    let pos = chunks.partition_point(|c| *c.last().expect("non-empty chunk") < id);
    pos.min(chunks.len() - 1)
}

fn last_id(group: &[Chunk]) -> u64 {
    *group
        .last()
        .and_then(|c| c.last())
        .expect("non-empty group")
}

/// Group and chunk that hold `id` or would receive it, as [`chunk_for`]
/// over the groups and then the chunks of one.
fn locate(groups: &[Group], id: u64) -> (usize, usize) {
    let g = groups
        .partition_point(|group| last_id(group) < id)
        .min(groups.len() - 1);
    (g, chunk_for(&groups[g], id))
}

impl IdSet {
    pub(super) fn new(id: u64) -> Self {
        Self::pack(Unpacked::One(id))
    }

    /// Insert `id`; returns `false` if it was already present.
    pub(super) fn insert(&mut self, id: u64) -> bool {
        self.update(|set| set.insert(id))
    }

    /// Remove `id`; returns `true` if the set is now empty and the
    /// bucket should be dropped by the caller.
    pub(super) fn remove(&mut self, id: u64) -> bool {
        self.update(|set| set.remove(id))
    }

    pub(super) fn len(&self) -> usize {
        match self.view() {
            View::One(_) => 1,
            View::Small(ids) => ids.len(),
            View::Large(large) => large.len,
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn to_vec(&self) -> Vec<u64> {
        match self.view() {
            View::One(id) => vec![id],
            View::Small(ids) => ids.clone(),
            View::Large(large) => {
                let mut out = Vec::with_capacity(large.len);
                for chunk in large.groups.iter().flat_map(|group| group.iter()) {
                    out.extend_from_slice(chunk);
                }
                out
            }
        }
    }

    /// Ids in ascending order.
    pub(super) fn iter(&self) -> IdSetIter<'_> {
        match self.view() {
            View::One(id) => IdSetIter::one(id),
            View::Small(ids) => IdSetIter::slice(ids),
            View::Large(large) => IdSetIter::groups(&large.groups, large.len),
        }
    }

    /// Ids strictly after `cursor` in the given direction (ascending:
    /// `> cursor`; descending: `< cursor`), in that direction. Seeks by
    /// binary search instead of scanning past the cursor.
    pub(super) fn iter_after(
        &self,
        cursor: Option<u64>,
        descending: bool,
    ) -> Box<dyn Iterator<Item = u64> + '_> {
        match (self.view(), cursor) {
            (_, None) if descending => Box::new(self.iter().rev()),
            (_, None) => Box::new(self.iter()),
            (View::One(id), Some(c)) => {
                let keep = if descending { id < c } else { id > c };
                Box::new(keep.then_some(id).into_iter())
            }
            (View::Small(ids), Some(c)) => {
                if descending {
                    let end = ids.partition_point(|&id| id < c);
                    Box::new(ids[..end].iter().rev().copied())
                } else {
                    let start = ids.partition_point(|&id| id <= c);
                    Box::new(ids[start..].iter().copied())
                }
            }
            (View::Large(large), Some(c)) => {
                let groups = &large.groups;
                let (g, at) = locate(groups, c);
                let chunks = &groups[g];
                let chunk = &chunks[at];
                if descending {
                    let end = chunk.partition_point(|&id| id < c);
                    let head = chunk[..end].iter().rev().copied();
                    let rest = chunks[..at]
                        .iter()
                        .rev()
                        .chain(groups[..g].iter().rev().flat_map(|g| g.iter().rev()))
                        .flat_map(|c| c.iter().rev().copied());
                    Box::new(head.chain(rest))
                } else {
                    let start = chunk.partition_point(|&id| id <= c);
                    let head = chunk[start..].iter().copied();
                    let rest = chunks[at + 1..]
                        .iter()
                        .chain(groups[g + 1..].iter().flat_map(|g| g.iter()))
                        .flat_map(|c| c.iter().copied());
                    Box::new(head.chain(rest))
                }
            }
        }
    }

    /// Heap bytes owned beyond the inline word. Used by the memory
    /// estimator. Chunks shared with another graph copy are charged to
    /// both, like the other copy-on-write structures.
    pub(super) fn heap_bytes(&self) -> usize {
        match self.view() {
            View::One(_) => 0,
            View::Small(ids) => {
                std::mem::size_of::<Vec<u64>>() + ids.capacity() * std::mem::size_of::<u64>()
            }
            View::Large(large) => {
                let groups = &large.groups;
                ARC_HEADER
                    + std::mem::size_of::<Large>()
                    + groups.capacity() * std::mem::size_of::<Group>()
                    + groups
                        .iter()
                        .map(|group| {
                            ARC_HEADER
                                + std::mem::size_of::<Vec<Chunk>>()
                                + group.capacity() * std::mem::size_of::<Chunk>()
                        })
                        .sum::<usize>()
                    + groups
                        .iter()
                        .flat_map(|group| group.iter())
                        .map(|c| {
                            ARC_HEADER
                                + std::mem::size_of::<Vec<u64>>()
                                + c.capacity() * std::mem::size_of::<u64>()
                        })
                        .sum::<usize>()
            }
        }
    }
}

impl Unpacked {
    fn insert(&mut self, id: u64) -> bool {
        match self {
            Unpacked::One(existing) => {
                let existing = *existing;
                if existing == id {
                    return false;
                }
                let pair = if existing < id {
                    vec![existing, id]
                } else {
                    vec![id, existing]
                };
                *self = Unpacked::Small(Box::new(pair));
                true
            }
            Unpacked::Small(ids) => {
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
                    let len = ids.len();
                    let chunk = Arc::new(std::mem::take(&mut **ids));
                    let groups = vec![Arc::new(vec![chunk])];
                    *self = Unpacked::Large(Arc::new(Large { groups, len }));
                }
                true
            }
            Unpacked::Large(large) => {
                let last = large.groups.last().expect("non-empty table");
                let tail = last.last().expect("non-empty group");
                let (chunks, tail_len) = (last.len(), tail.len());
                if *tail.last().expect("non-empty chunk") < id {
                    // Append: fill the last chunk, then open a new one,
                    // and a new group once the last one is full.
                    let large = Arc::make_mut(large);
                    let root = &mut large.groups;
                    if tail_len < CHUNK {
                        let group = Arc::make_mut(root.last_mut().expect("non-empty table"));
                        Arc::make_mut(group.last_mut().expect("non-empty group")).push(id);
                    } else if chunks < GROUP {
                        let group = Arc::make_mut(root.last_mut().expect("non-empty table"));
                        group.push(Arc::new(vec![id]));
                    } else {
                        root.push(Arc::new(vec![Arc::new(vec![id])]));
                    }
                    large.len += 1;
                    return true;
                }
                let (g, c) = locate(&large.groups, id);
                let Err(pos) = large.groups[g][c].binary_search(&id) else {
                    return false;
                };
                let large = Arc::make_mut(large);
                let root = &mut large.groups;
                let group = Arc::make_mut(&mut root[g]);
                let chunk = Arc::make_mut(&mut group[c]);
                chunk.insert(pos, id);
                if chunk.len() > CHUNK_MAX {
                    let upper = chunk.split_off(chunk.len() / 2);
                    group.insert(c + 1, Arc::new(upper));
                    if group.len() > GROUP_MAX {
                        let upper = group.split_off(group.len() / 2);
                        root.insert(g + 1, Arc::new(upper));
                    }
                }
                large.len += 1;
                true
            }
        }
    }

    fn remove(&mut self, id: u64) -> bool {
        match self {
            Unpacked::One(existing) => *existing == id,
            Unpacked::Small(ids) => {
                if let Ok(pos) = ids.binary_search(&id) {
                    ids.remove(pos);
                }
                if let [only] = ids.as_slice() {
                    *self = Unpacked::One(*only);
                }
                ids_is_empty(self)
            }
            Unpacked::Large(large) => {
                let (g, c) = locate(&large.groups, id);
                let Ok(pos) = large.groups[g][c].binary_search(&id) else {
                    return false;
                };
                let large = Arc::make_mut(large);
                let root = &mut large.groups;
                let group = Arc::make_mut(&mut root[g]);
                let chunk = Arc::make_mut(&mut group[c]);
                chunk.remove(pos);
                large.len -= 1;
                if chunk.is_empty() {
                    group.remove(c);
                } else if chunk.len() < CHUNK_MIN {
                    merge_small(group, c, CHUNK, |a, b| a.extend_from_slice(&b));
                }
                if group.is_empty() {
                    root.remove(g);
                } else if group.len() < GROUP_MIN {
                    merge_small(root, g, GROUP, |a, b| a.extend_from_slice(&b));
                }
                large.len == 0
            }
        }
    }
}

/// Fold the undersized chunk (or group) at `at` into a neighbour when the
/// two fit in `fit` entries, `append` moving the second's entries onto the
/// end of the first.
fn merge_small<T: Clone>(
    table: &mut Vec<Arc<Vec<T>>>,
    at: usize,
    fit: usize,
    append: impl Fn(&mut Vec<T>, Arc<Vec<T>>),
) {
    let small = table[at].len();
    let target = if at + 1 < table.len() && small + table[at + 1].len() <= fit {
        at
    } else if at > 0 && small + table[at - 1].len() <= fit {
        at - 1
    } else {
        return;
    };
    let next = table.remove(target + 1);
    append(Arc::make_mut(&mut table[target]), next);
}

fn ids_is_empty(set: &Unpacked) -> bool {
    matches!(set, Unpacked::Small(ids) if ids.is_empty())
}

/// Ascending iterator over any representation: the lone id of a set of
/// one, a flat slice for small sets, a walk over the groups and chunks
/// for large ones. Double-ended and exact-size. Each end opens a group,
/// then a chunk; once the groups run out it takes over what the other
/// end opened.
pub(super) struct IdSetIter<'a> {
    /// Groups not yet opened by either end.
    groups: std::slice::Iter<'a, Group>,
    front_chunks: std::slice::Iter<'a, Chunk>,
    back_chunks: std::slice::Iter<'a, Chunk>,
    front: std::slice::Iter<'a, u64>,
    back: std::slice::Iter<'a, u64>,
    /// A set of one's id, which lives in the set's word and has no slice
    /// to borrow. Read after everything else, so other sets pay for it
    /// once, at the end, and not per id.
    one: Option<u64>,
    remaining: usize,
}

impl<'a> IdSetIter<'a> {
    fn one(id: u64) -> Self {
        Self {
            one: Some(id),
            remaining: 1,
            ..Self::slice(&[])
        }
    }

    fn slice(ids: &'a [u64]) -> Self {
        Self {
            groups: [].iter(),
            front_chunks: [].iter(),
            back_chunks: [].iter(),
            front: ids.iter(),
            back: [].iter(),
            one: None,
            remaining: ids.len(),
        }
    }

    fn groups(groups: &'a [Group], len: usize) -> Self {
        Self {
            groups: groups.iter(),
            front_chunks: [].iter(),
            back_chunks: [].iter(),
            front: [].iter(),
            back: [].iter(),
            one: None,
            remaining: len,
        }
    }
}

impl Iterator for IdSetIter<'_> {
    type Item = u64;

    #[inline]
    fn next(&mut self) -> Option<u64> {
        loop {
            if let Some(&id) = self.front.next() {
                self.remaining -= 1;
                return Some(id);
            }
            if let Some(chunk) = self.front_chunks.next() {
                self.front = chunk.iter();
            } else if let Some(group) = self.groups.next() {
                self.front_chunks = group.iter();
            } else if let Some(chunk) = self.back_chunks.next() {
                self.front = chunk.iter();
            } else {
                let id = match self.back.next() {
                    Some(&id) => id,
                    None => self.one.take()?,
                };
                self.remaining -= 1;
                return Some(id);
            }
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl DoubleEndedIterator for IdSetIter<'_> {
    #[inline]
    fn next_back(&mut self) -> Option<u64> {
        loop {
            if let Some(&id) = self.back.next_back() {
                self.remaining -= 1;
                return Some(id);
            }
            if let Some(chunk) = self.back_chunks.next_back() {
                self.back = chunk.iter();
            } else if let Some(group) = self.groups.next_back() {
                self.back_chunks = group.iter();
            } else if let Some(chunk) = self.front_chunks.next_back() {
                self.back = chunk.iter();
            } else {
                let id = match self.front.next_back() {
                    Some(&id) => id,
                    None => self.one.take()?,
                };
                self.remaining -= 1;
                return Some(id);
            }
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

    /// Every representation clones, compares and drops on its own: run
    /// under miri, this checks the packing for leaks and double frees.
    #[test]
    fn packed_sets_clone_and_drop_in_every_form() {
        let mut sets = vec![IdSet::new(0), IdSet::new(INLINE_MAX)];
        let mut small = IdSet::new(3);
        small.insert(1);
        sets.push(small);
        let mut large = IdSet::new(0);
        for id in 1..=SMALL_MAX as u64 {
            large.insert(id);
        }
        assert!(matches!(large.view(), View::Large(_)));
        sets.push(large);
        for set in &sets {
            let copy = set.clone();
            assert_eq!(&copy, set);
            assert_eq!(copy.to_vec(), set.iter().collect::<Vec<_>>());
        }
        // A write to a copy of a shared large set leaves the original.
        let mut staged = sets[3].clone();
        staged.insert(1_000);
        assert_eq!(staged.len(), sets[3].len() + 1);
        assert!(!sets[3].iter().any(|id| id == 1_000));
    }

    #[test]
    fn an_id_too_large_to_inline_is_kept_whole() {
        let big = INLINE_MAX + 1;
        for id in [big, u64::MAX] {
            let mut set = IdSet::new(id);
            assert_eq!(set.to_vec(), vec![id]);
            assert_eq!(set.len(), 1);
            assert!(set.insert(5));
            assert_eq!(set.to_vec(), vec![5, id]);
            assert!(!set.remove(5));
            assert_eq!(set.iter().collect::<Vec<_>>(), vec![id]);
            assert!(set.remove(id));
        }
    }

    #[test]
    fn a_set_of_one_iterates_from_either_end() {
        let set = IdSet::new(7);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![7]);
        assert_eq!(set.iter().rev().collect::<Vec<_>>(), vec![7]);
        assert_eq!(set.iter().len(), 1);
        assert_eq!(set.iter_after(Some(6), false).collect::<Vec<_>>(), vec![7]);
        assert_eq!(set.iter_after(Some(7), false).count(), 0);
        assert_eq!(set.iter_after(Some(8), true).collect::<Vec<_>>(), vec![7]);
    }

    fn chunk_ptrs(set: &IdSet) -> Vec<*const Vec<u64>> {
        let View::Large(Large { groups, .. }) = set.view() else {
            panic!("expected a chunked set");
        };
        groups
            .iter()
            .flat_map(|g| g.iter().map(Arc::as_ptr))
            .collect()
    }

    /// Chunks and groups of a large set are non-empty and within bounds.
    fn check_large(set: &IdSet) {
        if let View::Large(Large { groups, len }) = set.view() {
            assert!(!groups.is_empty());
            let mut n = 0;
            for group in groups.iter() {
                assert!(!group.is_empty() && group.len() <= GROUP_MAX);
                for c in group.iter() {
                    assert!(!c.is_empty() && c.len() <= CHUNK_MAX);
                    n += c.len();
                }
            }
            assert_eq!(n, *len);
        }
    }

    #[test]
    fn grows_through_every_representation_in_order() {
        let mut set = IdSet::new(500);
        for id in (0..200).rev() {
            assert!(set.insert(id));
        }
        assert!(matches!(set.view(), View::Large(_)));
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
        assert!(matches!(set.view(), View::One(2)));
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

    /// Deterministic xorshift so the model test needs no extra crate.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn matches_a_btreeset_under_random_inserts_and_removes() {
        use std::collections::BTreeSet;
        let mut seed = 0x9e37_79b9_7f4a_7c15;
        let mut model = BTreeSet::new();
        let mut set: Option<IdSet> = None;
        for step in 0..60_000 {
            // Mostly appends (monotonic ids), some random inserts and
            // removes, with a delete-heavy phase in the middle.
            let roll = rng(&mut seed) % 100;
            let deleting = (20_000..35_000).contains(&step);
            let id = if roll < 50 && !deleting {
                model.last().map_or(0, |m| m + 1 + rng(&mut seed) % 3)
            } else {
                rng(&mut seed) % 8_000
            };
            if roll < 70 && !deleting || roll < 10 {
                let fresh = model.insert(id);
                match set.as_mut() {
                    Some(s) => assert_eq!(s.insert(id), fresh),
                    None => set = Some(IdSet::new(id)),
                }
            } else if let Some(s) = set.as_mut() {
                let present = model.remove(&id);
                let emptied = s.remove(id);
                if present {
                    assert_eq!(emptied, model.is_empty());
                }
                if emptied {
                    set = None;
                }
            }
            if step % 997 == 0 {
                let got: Vec<u64> = set.as_ref().map(IdSet::to_vec).unwrap_or_default();
                assert!(got.iter().copied().eq(model.iter().copied()), "step {step}");
                if let Some(s) = &set {
                    assert_eq!(s.len(), model.len());
                    assert!(s.iter().rev().eq(model.iter().rev().copied()));
                    assert_eq!(s.iter().len(), model.len());
                    let cursor = rng(&mut seed) % 9_000;
                    let asc: Vec<u64> = s.iter_after(Some(cursor), false).collect();
                    let want: Vec<u64> = model.range(cursor + 1..).copied().collect();
                    assert_eq!(asc, want);
                    let desc: Vec<u64> = s.iter_after(Some(cursor), true).collect();
                    let want: Vec<u64> = model.range(..cursor).rev().copied().collect();
                    assert_eq!(desc, want);
                    check_large(s);
                }
            }
        }
    }

    #[test]
    fn iterator_meets_in_the_middle_from_both_ends() {
        let mut set = IdSet::new(0);
        for id in 1..3_000 {
            set.insert(id);
        }
        let mut it = set.iter();
        let mut front = Vec::new();
        let mut back = Vec::new();
        loop {
            match (it.next(), it.next_back()) {
                (Some(a), Some(b)) => {
                    front.push(a);
                    back.push(b);
                }
                (Some(a), None) => front.push(a),
                (None, Some(b)) => back.push(b),
                (None, None) => break,
            }
        }
        back.reverse();
        front.extend(back);
        assert_eq!(front, (0..3_000).collect::<Vec<_>>());
    }

    #[test]
    fn a_write_to_a_shared_large_set_copies_one_chunk() {
        let mut base = IdSet::new(0);
        for id in 1..20_000 {
            base.insert(id * 2);
        }
        let before = chunk_ptrs(&base);

        let mut staged = base.clone();
        assert!(staged.insert(5_001));
        assert!(!staged.remove(3));
        assert!(!staged.remove(40));
        let after = chunk_ptrs(&staged);
        let shared = after.iter().filter(|p| before.contains(p)).count();
        assert!(shared >= before.len() - 2, "{shared} of {}", before.len());

        // The original is untouched.
        assert_eq!(base.len(), 20_000);
        assert!(base.iter().eq((0..20_000).map(|i| i * 2)));
        assert_eq!(staged.len(), 20_000);
        assert!(staged.iter().any(|id| id == 5_001));
        assert!(!staged.iter().any(|id| id == 40));
    }

    #[test]
    fn large_sets_group_their_chunks_and_match_a_model() {
        use std::collections::BTreeSet;
        let mut seed = 0x51_7cc1_b727_220a_u64;
        let mut model = BTreeSet::new();
        let mut set = IdSet::new(0);
        model.insert(0u64);
        // Appends up to ~300k ids (several groups), then random inserts
        // that split chunks and groups in the middle, then mass removes
        // that merge them, then appends again.
        for step in 0..900_000u64 {
            let roll = rng(&mut seed) % 100;
            if step < 300_000 {
                let id = model.last().unwrap() + 1 + roll % 2;
                assert_eq!(set.insert(id), model.insert(id));
            } else if step < 450_000 {
                let id = rng(&mut seed) % 700_000;
                assert_eq!(set.insert(id), model.insert(id));
            } else if step < 800_000 {
                let id = rng(&mut seed) % 700_000;
                let present = model.remove(&id);
                let emptied = set.remove(id);
                assert!(!emptied || model.is_empty());
                let _ = present;
            } else {
                let id = model.last().map_or(0, |m| m + 1);
                assert_eq!(set.insert(id), model.insert(id));
            }
            if step % 49_999 == 0 {
                check_large(&set);
                assert_eq!(set.len(), model.len());
                assert!(set.iter().eq(model.iter().copied()));
                assert!(set.iter().rev().eq(model.iter().rev().copied()));
                let cursor = rng(&mut seed) % 700_000;
                assert!(set
                    .iter_after(Some(cursor), false)
                    .eq(model.range(cursor + 1..).copied()));
                assert!(set
                    .iter_after(Some(cursor), true)
                    .eq(model.range(..cursor).rev().copied()));
            }
        }
        let View::Large(Large { groups, .. }) = set.view() else {
            panic!("expected a chunked set");
        };
        assert!(groups.len() > 1, "{} groups", groups.len());
        check_large(&set);
        assert!(set.to_vec().into_iter().eq(model.iter().copied()));
        // Meeting in the middle across groups.
        let mut it = set.iter();
        let (mut front, mut back) = (Vec::new(), Vec::new());
        loop {
            match (it.next(), it.next_back()) {
                (None, None) => break,
                (a, b) => {
                    front.extend(a);
                    back.extend(b);
                }
            }
        }
        back.reverse();
        front.extend(back);
        assert!(front.into_iter().eq(model.iter().copied()));
    }

    #[test]
    fn a_write_to_a_shared_huge_set_copies_one_group_and_one_chunk() {
        let mut base = IdSet::new(0);
        for id in 1..2_000_000 {
            base.insert(id * 2);
        }
        let View::Large(Large { groups, .. }) = base.view() else {
            panic!("expected a chunked set");
        };
        let groups_before: Vec<*const Vec<Chunk>> = groups.iter().map(Arc::as_ptr).collect();
        assert!(groups_before.len() >= 32, "{} groups", groups_before.len());
        let chunks_before = chunk_ptrs(&base);

        let mut staged = base.clone();
        assert!(staged.insert(1_000_001));
        let View::Large(Large { groups, .. }) = staged.view() else {
            panic!("expected a chunked set");
        };
        let new_groups = groups
            .iter()
            .filter(|g| !groups_before.contains(&Arc::as_ptr(g)))
            .count();
        let new_chunks = chunk_ptrs(&staged)
            .into_iter()
            .filter(|c| !chunks_before.contains(c))
            .count();
        assert_eq!((new_groups, new_chunks), (1, 1));
        assert_eq!(base.len(), 2_000_000);
        assert_eq!(staged.len(), 2_000_001);
        assert!(!base.iter().any(|id| id == 1_000_001));
    }
}
