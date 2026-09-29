//! A `Vec` split into fixed-size chunks shared by `Arc`.
//!
//! Every write that cannot run in place works on a clone of the graph
//! (so a failure can discard it), and every reader snapshot is a clone
//! too. With plain `Vec`s that clone copied every node slot, adjacency
//! list and label index entry: O(graph) per write. Here cloning copies one
//! pointer per [`CHUNK`] entries, and a write copies only the chunks it
//! touches (`Arc::make_mut`), so a single-row write on a large graph costs
//! a few kilobytes of copying instead of the whole store.
//!
//! The API mirrors the subset of `Vec` the store uses. Reads pay one extra
//! indexed load (chunk, then slot).

use std::sync::Arc;

/// Entries per chunk. A power of two so indexing is a shift and a mask.
pub(super) const CHUNK: usize = 512;
const SHIFT: u32 = CHUNK.trailing_zeros();
const MASK: usize = CHUNK - 1;

pub(super) struct ChunkedVec<T> {
    chunks: Vec<Arc<Vec<T>>>,
    len: usize,
}

impl<T> Default for ChunkedVec<T> {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }
}

impl<T> Clone for ChunkedVec<T> {
    /// O(len / CHUNK): chunk contents are shared, not copied.
    fn clone(&self) -> Self {
        Self {
            chunks: self.chunks.clone(),
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
        // SAFETY-free: `i < len` guarantees the chunk and slot exist.
        Some(&self.chunks[i >> SHIFT][i & MASK])
    }

    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + '_ {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
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
        for chunk in &self.chunks {
            out.extend_from_slice(chunk);
        }
        out
    }

    /// Allocated slots, for the memory estimator.
    pub(super) fn capacity(&self) -> usize {
        self.chunks.iter().map(|c| c.capacity()).sum()
    }

    /// Number of chunks, for the memory estimator.
    pub(super) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

impl<T: Clone> ChunkedVec<T> {
    /// Reserve the chunk table for `additional` more entries, failing
    /// (instead of looping or aborting) when the size is unrepresentable,
    /// e.g. a corrupt id asking for 2^60 slots.
    pub(super) fn try_reserve_exact(
        &mut self,
        additional: usize,
    ) -> Result<(), std::collections::TryReserveError> {
        self.chunks.try_reserve_exact(additional / CHUNK + 1)
    }

    /// Mutable access; copies the containing chunk first if a clone of
    /// this vector still shares it.
    #[inline]
    pub(super) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i >= self.len {
            return None;
        }
        Some(&mut Arc::make_mut(&mut self.chunks[i >> SHIFT])[i & MASK])
    }

    pub(super) fn push(&mut self, value: T) {
        if self.len & MASK == 0 {
            // The first chunk grows like a Vec so small vectors (a label
            // carried by three nodes) stay small; later chunks are
            // allocated at full size once.
            let capacity = if self.chunks.is_empty() { 0 } else { CHUNK };
            self.chunks.push(Arc::new(Vec::with_capacity(capacity)));
        }
        let last = self.chunks.last_mut().expect("chunk pushed above");
        Arc::make_mut(last).push(value);
        self.len += 1;
    }

    pub(super) fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let last = self.chunks.last_mut().expect("len > 0");
        let value = Arc::make_mut(last).pop();
        if last.is_empty() {
            self.chunks.pop();
        }
        self.len -= 1;
        value
    }

    /// Grow to `new_len` with `fill`, or shrink to it.
    pub(super) fn resize_with(&mut self, new_len: usize, mut fill: impl FnMut() -> T) {
        while self.len > new_len {
            self.pop();
        }
        if self.len < new_len {
            self.chunks.reserve((new_len - self.len) / CHUNK + 1);
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
    fn clones_share_chunks_until_written() {
        let mut a: ChunkedVec<u64> = (0..(CHUNK as u64 * 4)).collect();
        let b = a.clone();
        a[5] = 999;
        assert_eq!(b[5], 5, "the clone is unaffected by writes");
        assert_eq!(a[5], 999);
        // Only the written chunk was copied.
        assert!(Arc::ptr_eq(&a.chunks[1], &b.chunks[1]));
        assert!(!Arc::ptr_eq(&a.chunks[0], &b.chunks[0]));
    }
}
