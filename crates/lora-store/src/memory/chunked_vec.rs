//! A `Vec` stored as a two-level persistent radix tree of `Arc`-shared
//! chunks.
//!
//! Every write that cannot run in place works on a clone of the graph
//! (so a failure can discard it), and every reader snapshot is a clone
//! too. With plain `Vec`s that clone copied every node slot, adjacency
//! list and label index entry: O(graph) per write. A flat table of
//! `Arc`'d chunks made it one refcount bump per [`CHUNK`] entries, which
//! is still O(graph): 218 µs at 2M nodes / 8M relationships.
//!
//! Here the chunks (leaves of [`CHUNK`] entries) hang off interior nodes
//! of [`FANOUT`] leaf pointers, which hang off one shared root:
//!
//! ```text
//! root: Arc<[node]>  →  node: Arc<[leaf]>  →  leaf: Arc<Vec<T>>
//!       len / SPAN            ≤ FANOUT              ≤ CHUNK
//! ```
//!
//! Cloning bumps the root's refcount: O(1) whatever the length. A write
//! (`Arc::make_mut` down the path) copies only what a clone still shares:
//! the root's pointer table (len / [`SPAN`] entries), one interior node
//! ([`FANOUT`] pointers) and one leaf ([`CHUNK`] entries), and a second
//! write to the same leaf copies nothing.
//!
//! The root and interior nodes are `Arc<[_]>` slices rather than
//! `Arc<Vec<_>>`, so the pointer to each carries its length and points
//! straight at the entries: a read costs one more dependent load than a
//! flat chunk table, not three. They only change size when a leaf or an
//! interior node is added or dropped (once per [`CHUNK`] pushes or pops),
//! and are rebuilt then. The root and interior nodes are small and stay
//! in cache, so the leaf and the entry are the only likely misses, as
//! with a flat table.
//!
//! The API mirrors the subset of `Vec` the store uses.

use std::collections::TryReserveError;
use std::sync::Arc;

/// Entries per leaf chunk. A power of two so indexing is a shift and a mask.
pub(super) const CHUNK: usize = 512;
const SHIFT: u32 = CHUNK.trailing_zeros();
const MASK: usize = CHUNK - 1;

/// Leaf pointers per interior node.
///
/// The first write after a clone copies the root table (len / SPAN
/// pointers) and one interior node (FANOUT pointers), bumping a refcount
/// for each, on top of the leaf copy every fan-out pays. A small fan-out
/// keeps the interior copy cheap but grows the root, a large one the
/// reverse. At 128 a node covers 65,536 entries: the root holds 31
/// pointers at 2M nodes and 123 for their 8M relationships. Measured on
/// that graph (M1 Max), staged writes were the same within noise at 64
/// and 128 and 5–50% slower at 512 (whose interior copy is 512 bumps);
/// 128 was marginally ahead on relationship CREATE and keeps the root
/// table under ~150 pointers up to 10M entries.
pub(super) const FANOUT: usize = 128;
const FAN_SHIFT: u32 = FANOUT.trailing_zeros();
const FAN_MASK: usize = FANOUT - 1;

/// Entries under one interior node.
pub(super) const SPAN: usize = CHUNK * FANOUT;
const SPAN_SHIFT: u32 = SHIFT + FAN_SHIFT;
const SPAN_MASK: usize = SPAN - 1;

const _: () = assert!(CHUNK.is_power_of_two() && FANOUT.is_power_of_two());

type Leaf<T> = Arc<Vec<T>>;
type Interior<T> = Arc<[Leaf<T>]>;

pub(super) struct ChunkedVec<T> {
    root: Arc<[Interior<T>]>,
    len: usize,
}

impl<T> Default for ChunkedVec<T> {
    fn default() -> Self {
        Self {
            root: Arc::new([]),
            len: 0,
        }
    }
}

/// `slice` with `last` appended, as a new allocation sharing the entries.
fn appended<X: Clone>(slice: &[X], last: X) -> Arc<[X]> {
    slice.iter().cloned().chain(std::iter::once(last)).collect()
}

/// `slice` without its last entry, as a new allocation sharing the rest.
fn truncated<X: Clone>(slice: &[X]) -> Arc<[X]> {
    slice[..slice.len() - 1].iter().cloned().collect()
}

impl<T> Clone for ChunkedVec<T> {
    /// O(1): one refcount bump on the root; every node and leaf is shared.
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            len: self.len,
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ChunkedVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: PartialEq> PartialEq for ChunkedVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl<T> ChunkedVec<T> {
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(super) fn get(&self, i: usize) -> Option<&T> {
        if i >= self.len {
            return None;
        }
        // SAFETY: the tree holds exactly `len` entries packed from the
        // front (every node but the last is full, and so is every leaf
        // but the last of the last node; `push` and `pop` keep that), so
        // `i < len` puts the node, leaf and slot in bounds. Skipping the
        // three bounds checks keeps a read as cheap as a flat chunk table.
        unsafe {
            let node = self.root.get_unchecked(i >> SPAN_SHIFT);
            let leaf = node.get_unchecked((i >> SHIFT) & FAN_MASK);
            Some(leaf.get_unchecked(i & MASK))
        }
    }

    /// The leaf chunks in order.
    fn leaves(&self) -> impl DoubleEndedIterator<Item = &Vec<T>> + '_ {
        self.root
            .iter()
            .flat_map(|node| node.iter())
            .map(|leaf| &**leaf)
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + '_ {
        self.leaves().flat_map(|leaf| leaf.iter())
    }

    /// The elements as one `Vec`, copied a chunk at a time into an exact
    /// allocation. Collecting `iter()` instead grows the vector by doubling
    /// and copies element by element, which doubled the cost of opening a
    /// label scan.
    pub(super) fn to_vec(&self) -> Vec<T>
    where
        T: Clone,
    {
        let mut out = Vec::with_capacity(self.len);
        for leaf in self.leaves() {
            out.extend_from_slice(leaf);
        }
        out
    }

    /// Allocated slots, for the memory estimator.
    pub(super) fn capacity(&self) -> usize {
        self.leaves().map(|leaf| leaf.capacity()).sum()
    }

    /// Number of leaf chunks, for the memory estimator.
    pub(super) fn chunk_count(&self) -> usize {
        self.root.iter().map(|node| node.len()).sum()
    }

    /// Bytes of the tree above the leaves (the root and interior nodes,
    /// less the one pointer per leaf the estimator charges per chunk),
    /// for the memory estimator.
    pub(super) fn tree_overhead_bytes(&self) -> usize {
        const ARC_HEADER: usize = 2 * size_of::<usize>();
        ARC_HEADER + self.root.len() * (size_of::<Interior<T>>() + ARC_HEADER)
    }
}

/// The error `Vec` reports for a size it cannot represent.
fn capacity_overflow() -> TryReserveError {
    Vec::<u8>::new()
        .try_reserve_exact(usize::MAX)
        .expect_err("usize::MAX bytes exceeds isize::MAX")
}

impl<T: Clone> ChunkedVec<T> {
    /// Check that `additional` more entries are representable, failing
    /// (instead of looping or aborting) when they are not, e.g. a corrupt
    /// id asking for 2^60 slots. Leaves are allocated as the vector grows,
    /// so there is nothing to reserve up front.
    pub(super) fn try_reserve_exact(&mut self, additional: usize) -> Result<(), TryReserveError> {
        let bytes = self
            .len
            .checked_add(additional)
            .and_then(|total| total.checked_mul(size_of::<T>().max(1)));
        if bytes.is_none_or(|bytes| bytes > isize::MAX as usize) {
            return Err(capacity_overflow());
        }
        Ok(())
    }

    /// Mutable access; copies the root table, interior node and leaf on
    /// the path first if a clone of this vector still shares them.
    #[inline]
    pub(super) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i >= self.len {
            return None;
        }
        let node = &mut Arc::make_mut(&mut self.root)[i >> SPAN_SHIFT];
        let leaf = &mut Arc::make_mut(node)[(i >> SHIFT) & FAN_MASK];
        Some(&mut Arc::make_mut(leaf)[i & MASK])
    }

    pub(super) fn push(&mut self, value: T) {
        let len = self.len;
        if len & MASK == 0 {
            // The first leaf grows like a Vec so small vectors (a label
            // carried by three nodes) stay small; later leaves are
            // allocated at full size once.
            let capacity = if len == 0 { 0 } else { CHUNK };
            let leaf: Leaf<T> = Arc::new(Vec::with_capacity(capacity));
            if len & SPAN_MASK == 0 {
                self.root = appended(&self.root, Arc::new([leaf]));
            } else {
                let root = Arc::make_mut(&mut self.root);
                let node = root.last_mut().expect("len > 0");
                *node = appended(node, leaf);
            }
        }
        let root = Arc::make_mut(&mut self.root);
        let node = Arc::make_mut(root.last_mut().expect("node exists"));
        Arc::make_mut(node.last_mut().expect("leaf exists")).push(value);
        self.len += 1;
    }

    pub(super) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let root = Arc::make_mut(&mut self.root);
        let node = root.last_mut().expect("len > 0");
        let leaf = Arc::make_mut(node).last_mut().expect("len > 0");
        let value = Arc::make_mut(leaf).pop();
        if leaf.is_empty() {
            if node.len() == 1 {
                self.root = truncated(&self.root);
            } else {
                *node = truncated(node);
            }
        }
        self.len -= 1;
        value
    }

    /// Grow to `new_len` with `fill`, or shrink to it.
    pub(super) fn resize_with(&mut self, new_len: usize, mut fill: impl FnMut() -> T) {
        while self.len > new_len {
            self.pop();
        }
        while self.len < new_len {
            self.push(fill());
        }
    }

    /// Remove element `i` by moving the last element into its place.
    pub(super) fn swap_remove(&mut self, i: usize) -> T {
        assert!(i < self.len, "swap_remove index {i} out of bounds");
        let last = self.pop().expect("non-empty");
        if i == self.len {
            return last;
        }
        std::mem::replace(self.get_mut(i).expect("in bounds"), last)
    }
}

impl<T> std::ops::Index<usize> for ChunkedVec<T> {
    type Output = T;

    #[inline]
    fn index(&self, i: usize) -> &T {
        self.get(i).expect("ChunkedVec index out of bounds")
    }
}

impl<T: Clone> std::ops::IndexMut<usize> for ChunkedVec<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        self.get_mut(i).expect("ChunkedVec index out of bounds")
    }
}

impl<T: Clone> FromIterator<T> for ChunkedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut out = Self::new();
        for value in iter {
            out.push(value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl<T> ChunkedVec<T> {
        fn node(&self, n: usize) -> &Interior<T> {
            &self.root[n]
        }

        fn leaf(&self, i: usize) -> &Leaf<T> {
            &self.root[i >> SPAN_SHIFT][(i >> SHIFT) & FAN_MASK]
        }
    }

    #[test]
    fn behaves_like_a_vec_across_chunk_boundaries() {
        let mut v: ChunkedVec<usize> = ChunkedVec::new();
        let mut reference = Vec::new();
        for i in 0..(CHUNK * 3 + 7) {
            v.push(i);
            reference.push(i);
        }
        assert_eq!(v.len(), reference.len());
        assert!(v.iter().eq(reference.iter()));
        assert_eq!(v.swap_remove(3), reference.swap_remove(3));
        assert_eq!(v.swap_remove(CHUNK + 1), reference.swap_remove(CHUNK + 1));
        assert_eq!(
            v.swap_remove(v.len() - 1),
            reference.swap_remove(reference.len() - 1)
        );
        assert!(v.iter().eq(reference.iter()));
        assert_eq!(v.to_vec(), reference);
        v.resize_with(10, || 0);
        reference.truncate(10);
        assert!(v.iter().eq(reference.iter()));
        v.resize_with(CHUNK + 2, || 9);
        reference.resize(CHUNK + 2, 9);
        assert!(v.iter().eq(reference.iter()));
        assert_eq!(v.get(v.len() - 1), reference.last());
        assert_eq!(v.to_vec(), reference);
        assert_eq!(ChunkedVec::<usize>::new().to_vec(), Vec::<usize>::new());
    }

    #[test]
    fn behaves_like_a_vec_across_interior_boundaries() {
        let n = SPAN * 2 + CHUNK + 3;
        let mut v: ChunkedVec<u32> = (0..n as u32).collect();
        let mut reference: Vec<u32> = (0..n as u32).collect();
        assert_eq!(v.len(), n);
        assert_eq!(v.chunk_count(), n.div_ceil(CHUNK));
        assert!(v.iter().eq(reference.iter()));
        assert!(v.iter().rev().eq(reference.iter().rev()));
        for i in [
            0,
            CHUNK - 1,
            CHUNK,
            SPAN - 1,
            SPAN,
            SPAN + 1,
            2 * SPAN,
            n - 1,
        ] {
            assert_eq!(v.get(i), reference.get(i), "index {i}");
            assert_eq!(v[i], i as u32);
        }
        assert_eq!(v.get(n), None);
        // Shrink across an interior boundary and grow back.
        v.resize_with(SPAN - 1, || 0);
        reference.truncate(SPAN - 1);
        assert_eq!(v.root.len(), 1, "the emptied interior nodes are dropped");
        assert!(v.iter().eq(reference.iter()));
        v.resize_with(SPAN + 5, || 7);
        reference.resize(SPAN + 5, 7);
        assert!(v.iter().eq(reference.iter()));
        assert_eq!(v.to_vec(), reference);
        assert_eq!(v.swap_remove(1), reference.swap_remove(1));
        assert_eq!(v.swap_remove(SPAN), reference.swap_remove(SPAN));
        assert!(v.iter().eq(reference.iter()));
        while let Some(x) = v.pop() {
            assert_eq!(Some(x), reference.pop());
        }
        assert!(reference.is_empty());
        assert!(v.is_empty());
        assert!(v.root.is_empty());
        v.push(1);
        assert_eq!(v.to_vec(), vec![1]);
    }

    #[test]
    fn clones_share_chunks_until_written() {
        let mut a: ChunkedVec<u64> = (0..(CHUNK as u64 * 4)).collect();
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.root, &b.root), "a clone shares the root");
        a[5] = 999;
        assert_eq!(b[5], 5, "the clone is unaffected by writes");
        assert_eq!(a[5], 999);
        // Only the written path was copied.
        assert!(Arc::ptr_eq(a.leaf(CHUNK), b.leaf(CHUNK)));
        assert!(!Arc::ptr_eq(a.leaf(0), b.leaf(0)));
        // A second write to the same leaf copies nothing more.
        let leaf = Arc::as_ptr(a.leaf(0));
        a[6] = 1000;
        assert_eq!(Arc::as_ptr(a.leaf(0)), leaf);
    }

    #[test]
    fn clones_are_independent_across_interior_boundaries() {
        let n = SPAN * 3 + 17;
        let original: ChunkedVec<u64> = (0..n as u64).collect();
        let reference: Vec<u64> = (0..n as u64).collect();
        let points = [0, CHUNK - 1, CHUNK, SPAN - 1, SPAN, 2 * SPAN + CHUNK, n - 1];

        // Writes to the clone leave the original alone.
        let mut copy = original.clone();
        for &i in &points {
            copy[i] = u64::MAX - i as u64;
        }
        assert!(original.iter().eq(reference.iter()));
        for &i in &points {
            assert_eq!(copy[i], u64::MAX - i as u64);
        }
        // Untouched interior nodes and leaves stay shared.
        assert!(Arc::ptr_eq(
            original.leaf(SPAN + CHUNK),
            copy.leaf(SPAN + CHUNK)
        ));
        assert!(!Arc::ptr_eq(original.node(1), copy.node(1)));
        assert!(Arc::ptr_eq(
            original.leaf(SPAN + 2 * CHUNK),
            copy.leaf(SPAN + 2 * CHUNK)
        ));

        // And writes to the original leave the clone alone.
        let mut original = original;
        let snapshot = original.clone();
        for &i in &points {
            original[i] = 42;
        }
        assert!(snapshot.iter().eq(reference.iter()));

        // Growth and shrinkage of a clone across leaf and interior
        // boundaries.
        let mut grown = snapshot.clone();
        for i in 0..(SPAN + CHUNK) as u64 {
            grown.push(n as u64 + i);
        }
        assert_eq!(snapshot.len(), n);
        assert!(snapshot.iter().eq(reference.iter()));
        assert!(grown.iter().take(n).eq(reference.iter()));
        assert_eq!(grown[n + SPAN], (n + SPAN) as u64);

        let mut shrunk = snapshot.clone();
        shrunk.resize_with(SPAN - 3, || 0);
        assert_eq!(shrunk.len(), SPAN - 3);
        assert!(snapshot.iter().eq(reference.iter()));
        assert_eq!(shrunk.swap_remove(0), 0);
        assert_eq!(shrunk[0], (SPAN - 4) as u64);
        assert_eq!(snapshot[0], 0);
        assert_eq!(snapshot[SPAN - 4], (SPAN - 4) as u64);
    }

    #[test]
    fn small_vectors_stay_small() {
        let v: ChunkedVec<u64> = (0..3).collect();
        assert!(v.capacity() < CHUNK);
        assert_eq!(v.root.len(), 1);
        assert_eq!(v.node(0).len(), 1);
        assert_eq!(v.chunk_count(), 1);
    }

    #[test]
    fn unrepresentable_reservations_fail() {
        let mut v: ChunkedVec<u64> = ChunkedVec::new();
        assert!(v.try_reserve_exact(1 << 60).is_err());
        assert!(v.try_reserve_exact(usize::MAX).is_err());
        assert!(v.try_reserve_exact(SPAN * 3).is_ok());
        assert!(v.is_empty());
    }
}
