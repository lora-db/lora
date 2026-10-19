//! Slot-indexed storage for encoded records, a leaf of records per
//! allocation.
//!
//! Nodes and relationships are stored as encoded byte strings (see
//! [`crate::encoded`]), indexed by id. Each record used to be its own
//! allocation behind an 8-byte slot: an 8-byte header, the bytes, and the
//! allocator's rounding to its 16-byte size step, which on a
//! relationship of 11 bytes is two thirds overhead. Here the records of
//! [`CHUNK`] consecutive ids share one leaf: a table of `u32` offsets and
//! one byte buffer holding each record behind a varint length.
//!
//! ```text
//! root: Arc<[node]>  →  node: Arc<[leaf]>  →  leaf: Arc<Leaf>
//!                                              offsets: [u32; CHUNK]
//!                                              data:    len, bytes, len, bytes, ...
//! ```
//!
//! The tree above the leaves is [`super::chunked_vec`]'s: cloning bumps
//! the root's refcount, and the first write after a clone copies the
//! root table, one interior node and the one leaf it touches. Copying a
//! leaf copies its buffer, where a leaf of separate records bumped one
//! count per record.
//!
//! A record is never changed in place. Storing a new version appends it
//! to the buffer and leaves the old bytes as garbage until the leaf is
//! compacted, which happens once garbage is half the buffer, and on every
//! copy. Records of [`LARGE_MIN`] bytes or more are kept in their own
//! allocations, so a leaf's buffer stays small enough for `u32` offsets.

use std::sync::Arc;

use super::chunked_vec::{CHUNK, FANOUT};

const SHIFT: u32 = CHUNK.trailing_zeros();
const MASK: usize = CHUNK - 1;
const FAN_SHIFT: u32 = FANOUT.trailing_zeros();
const FAN_MASK: usize = FANOUT - 1;
const SPAN_SHIFT: u32 = SHIFT + FAN_SHIFT;
const SPAN_MASK: usize = (CHUNK * FANOUT) - 1;

/// Offset of an empty slot.
const EMPTY: u32 = u32::MAX;
/// Set on the offset of a record held outside the buffer; the other bits
/// index [`Leaf::large`].
const LARGE: u32 = 1 << 31;
/// Records at least this long are held outside the buffer. With them out,
/// a buffer holds at most `CHUNK * LARGE_MIN` bytes of live records (32
/// MiB) and as much garbage, well within the 31 bits of an offset.
const LARGE_MIN: usize = 1 << 16;

#[inline]
fn varint_len(value: usize) -> usize {
    (64 - (value as u64 | 1).leading_zeros() as usize).div_ceil(7)
}

#[inline]
fn put_varint(out: &mut Vec<u8>, mut value: usize) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[inline]
fn get_varint(bytes: &[u8], pos: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as usize) << shift;
        if byte < 0x80 {
            return value;
        }
        shift += 7;
    }
}

struct Leaf {
    offsets: [u32; CHUNK],
    data: Vec<u8>,
    large: Vec<Box<[u8]>>,
    /// Records stored.
    live: u32,
    /// Bytes of `data` that no slot refers to.
    garbage: usize,
    /// Entries of `large` that no slot refers to (emptied).
    large_garbage: usize,
}

impl Leaf {
    fn new() -> Self {
        Leaf {
            offsets: [EMPTY; CHUNK],
            data: Vec::new(),
            large: Vec::new(),
            live: 0,
            garbage: 0,
            large_garbage: 0,
        }
    }

    #[inline]
    fn get(&self, slot: usize) -> Option<&[u8]> {
        let offset = self.offsets[slot];
        if offset == EMPTY {
            return None;
        }
        if offset & LARGE != 0 {
            return Some(&self.large[(offset & !LARGE) as usize]);
        }
        let mut pos = offset as usize;
        let len = get_varint(&self.data, &mut pos);
        Some(&self.data[pos..pos + len])
    }

    /// Account for the record in `slot` becoming unreachable.
    fn release(&mut self, slot: usize) {
        let offset = self.offsets[slot];
        if offset == EMPTY {
            return;
        }
        if offset & LARGE != 0 {
            self.large[(offset & !LARGE) as usize] = Box::default();
            self.large_garbage += 1;
        } else {
            let mut pos = offset as usize;
            let len = get_varint(&self.data, &mut pos);
            self.garbage += pos - offset as usize + len;
        }
        self.offsets[slot] = EMPTY;
        self.live -= 1;
    }

    fn set(&mut self, slot: usize, bytes: &[u8]) {
        self.release(slot);
        self.live += 1;
        if bytes.len() >= LARGE_MIN {
            if self.large_garbage > self.large.len() / 2 {
                self.compact(0);
            }
            self.offsets[slot] = LARGE | self.large.len() as u32;
            self.large.push(bytes.into());
            return;
        }
        let need = varint_len(bytes.len()) + bytes.len();
        if self.data.capacity() - self.data.len() < need {
            self.make_room(need);
        }
        self.offsets[slot] = self.data.len() as u32;
        put_varint(&mut self.data, bytes.len());
        self.data.extend_from_slice(bytes);
    }

    fn remove(&mut self, slot: usize) -> bool {
        if self.offsets[slot] == EMPTY {
            return false;
        }
        self.release(slot);
        if self.live == 0 {
            *self = Leaf::new();
        } else if self.garbage > 4096 && self.garbage > self.data.len() / 2 {
            self.compact(0);
        }
        true
    }

    /// Grow the buffer for `need` more bytes, or compact it if it is
    /// mostly garbage.
    fn make_room(&mut self, need: usize) {
        if self.garbage > self.data.len() / 2 {
            self.compact(need);
            if self.data.capacity() - self.data.len() >= need {
                return;
            }
        }
        // Expect the empty slots to fill with records like the present
        // ones (ids are handed out in order, so a leaf fills front to
        // back), but reserve for at most as many again as are stored, so
        // a mostly empty leaf grows geometrically. A full leaf that is
        // being rewritten grows by an eighth.
        let live = self.live.saturating_sub(1) as usize;
        let avg = (self.data.len() - self.garbage)
            .checked_div(live)
            .unwrap_or(need);
        let empty = CHUNK - live - 1;
        let expected = empty.min(live.max(1)) * avg;
        let extra = need + expected.max(self.data.len() / 8);
        self.data.reserve_exact(extra);
    }

    /// Rewrite the buffer with only the reachable records, in slot order,
    /// leaving room for `extra` more bytes.
    fn compact(&mut self, extra: usize) {
        let live_bytes = self.data.len() - self.garbage;
        let mut data = Vec::with_capacity(live_bytes + extra);
        let mut large = Vec::with_capacity(self.large.len() - self.large_garbage);
        for slot in 0..CHUNK {
            let offset = self.offsets[slot];
            if offset == EMPTY {
                continue;
            }
            if offset & LARGE != 0 {
                let record = std::mem::take(&mut self.large[(offset & !LARGE) as usize]);
                self.offsets[slot] = LARGE | large.len() as u32;
                large.push(record);
            } else {
                let start = offset as usize;
                let mut pos = start;
                let len = get_varint(&self.data, &mut pos);
                self.offsets[slot] = data.len() as u32;
                data.extend_from_slice(&self.data[start..pos + len]);
            }
        }
        self.data = data;
        self.large = large;
        self.garbage = 0;
        self.large_garbage = 0;
    }

    fn heap_bytes(&self) -> usize {
        self.data.capacity()
            + self.large.capacity() * size_of::<Box<[u8]>>()
            + self.large.iter().map(|r| r.len()).sum::<usize>()
    }
}

impl Clone for Leaf {
    /// A compacted copy, sized to its records.
    fn clone(&self) -> Self {
        let mut copy = Leaf {
            offsets: self.offsets,
            data: Vec::new(),
            large: Vec::new(),
            live: self.live,
            garbage: self.garbage,
            large_garbage: self.large_garbage,
        };
        if self.garbage == 0 && self.large_garbage == 0 {
            copy.data = self.data.clone();
            copy.large = self.large.clone();
        } else {
            // Compact while copying: walk the slots of `self`, writing into
            // `copy`'s offsets and buffers.
            copy.data.reserve_exact(self.data.len() - self.garbage);
            copy.large
                .reserve_exact(self.large.len() - self.large_garbage);
            for slot in 0..CHUNK {
                let offset = self.offsets[slot];
                if offset == EMPTY {
                    continue;
                }
                if offset & LARGE != 0 {
                    copy.offsets[slot] = LARGE | copy.large.len() as u32;
                    copy.large
                        .push(self.large[(offset & !LARGE) as usize].clone());
                } else {
                    let start = offset as usize;
                    let mut pos = start;
                    let len = get_varint(&self.data, &mut pos);
                    copy.offsets[slot] = copy.data.len() as u32;
                    copy.data.extend_from_slice(&self.data[start..pos + len]);
                }
            }
            copy.garbage = 0;
            copy.large_garbage = 0;
        }
        copy
    }
}

type Interior = Arc<[Arc<Leaf>]>;

/// Encoded records by slot index. Empty slots read as `None`.
pub(super) struct RecordSlab {
    root: Arc<[Interior]>,
    /// Slots, empty or not. Leaves exist for all of them.
    len: usize,
}

impl Default for RecordSlab {
    fn default() -> Self {
        Self {
            root: Arc::new([]),
            len: 0,
        }
    }
}

impl Clone for RecordSlab {
    /// O(1): one refcount bump on the root; every node and leaf is shared.
    fn clone(&self) -> Self {
        Self {
            root: Arc::clone(&self.root),
            len: self.len,
        }
    }
}

impl std::fmt::Debug for RecordSlab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|(i, bytes)| (i, bytes.len())))
            .finish()
    }
}

impl RecordSlab {
    /// Number of slots, empty or not.
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    #[inline]
    fn leaf(&self, i: usize) -> &Leaf {
        debug_assert!(i < self.len);
        // SAFETY: leaves exist for every slot below `len`, packed from the
        // front (every interior node but the last is full), so `i < len`
        // puts the node and leaf in bounds. The callers check `i < len`.
        unsafe {
            let node = self.root.get_unchecked(i >> SPAN_SHIFT);
            node.get_unchecked((i >> SHIFT) & FAN_MASK)
        }
    }

    #[inline]
    pub(super) fn get(&self, i: usize) -> Option<&[u8]> {
        if i >= self.len {
            return None;
        }
        self.leaf(i).get(i & MASK)
    }

    #[inline]
    pub(super) fn contains(&self, i: usize) -> bool {
        i < self.len && self.leaf(i).offsets[i & MASK] != EMPTY
    }

    /// The leaf holding slot `i`, copied first if a clone shares it.
    fn leaf_mut(&mut self, i: usize) -> &mut Leaf {
        assert!(i < self.len, "record slot {i} out of bounds");
        let node = &mut Arc::make_mut(&mut self.root)[i >> SPAN_SHIFT];
        Arc::make_mut(&mut Arc::make_mut(node)[(i >> SHIFT) & FAN_MASK])
    }

    /// Store `bytes` in slot `i` (below [`Self::len`]), replacing what it
    /// held. Returns whether the slot was empty.
    pub(super) fn set(&mut self, i: usize, bytes: &[u8]) -> bool {
        let leaf = self.leaf_mut(i);
        let was_empty = leaf.offsets[i & MASK] == EMPTY;
        leaf.set(i & MASK, bytes);
        was_empty
    }

    /// Empty slot `i`. Returns whether it held a record.
    pub(super) fn remove(&mut self, i: usize) -> bool {
        if !self.contains(i) {
            return false;
        }
        self.leaf_mut(i).remove(i & MASK)
    }

    /// Check that `additional` more slots are representable, failing
    /// (instead of looping or aborting) when they are not, e.g. a corrupt
    /// id asking for 2^60 slots.
    pub(super) fn try_reserve(
        &self,
        additional: usize,
    ) -> Result<(), std::collections::TryReserveError> {
        let leaves = self
            .len
            .checked_add(additional)
            .map(|total| total.div_ceil(CHUNK))
            .and_then(|leaves| leaves.checked_mul(size_of::<Leaf>()));
        if leaves.is_none_or(|bytes| bytes > isize::MAX as usize) {
            return Err(Vec::<u8>::new()
                .try_reserve_exact(usize::MAX)
                .expect_err("usize::MAX bytes exceeds isize::MAX"));
        }
        Ok(())
    }

    /// Grow to `new_len` empty-filled slots. Never shrinks.
    pub(super) fn grow_to(&mut self, new_len: usize) {
        while self.len.div_ceil(CHUNK) < new_len.div_ceil(CHUNK) {
            // Leaves start at slot multiples of CHUNK; `leaves * CHUNK` is
            // the first slot of the new one.
            let first = self.len.div_ceil(CHUNK) * CHUNK;
            let leaf = Arc::new(Leaf::new());
            if first & SPAN_MASK == 0 {
                self.root = self
                    .root
                    .iter()
                    .cloned()
                    .chain(std::iter::once(Arc::from([leaf])))
                    .collect();
            } else {
                let root = Arc::make_mut(&mut self.root);
                let node = root.last_mut().expect("first slot is not a span start");
                *node = node.iter().cloned().chain(std::iter::once(leaf)).collect();
            }
            self.len = first + 1;
        }
        self.len = self.len.max(new_len);
    }

    /// `(slot, bytes)` of every record from slot `from` on, in slot order.
    pub(super) fn iter_from(&self, from: usize) -> impl Iterator<Item = (usize, &[u8])> + '_ {
        let leaves = self.len.div_ceil(CHUNK);
        let first = from / CHUNK;
        (first..leaves).flat_map(move |l| {
            let leaf = self.leaf(l * CHUNK);
            let start = if l == first { from & MASK } else { 0 };
            let end = (self.len - l * CHUNK).min(CHUNK);
            (start..end).filter_map(move |s| leaf.get(s).map(|bytes| (l * CHUNK + s, bytes)))
        })
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &[u8])> + '_ {
        self.iter_from(0)
    }

    /// Heap bytes: the tree, every leaf and its buffers.
    pub(super) fn heap_bytes(&self) -> usize {
        const ARC_HEADER: usize = 2 * size_of::<usize>();
        let tree = ARC_HEADER
            + self
                .root
                .iter()
                .map(|node| {
                    size_of::<Interior>() + ARC_HEADER + node.len() * size_of::<Arc<Leaf>>()
                })
                .sum::<usize>();
        let leaves: usize = self
            .root
            .iter()
            .flat_map(|node| node.iter())
            .map(|leaf| ARC_HEADER + size_of::<Leaf>() + leaf.heap_bytes())
            .sum();
        tree + leaves
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn splitmix(mut x: u64) -> u64 {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    fn record(seed: u64) -> Vec<u8> {
        let len = match seed % 50 {
            0 => LARGE_MIN + (seed % 7) as usize,
            1..=5 => 200 + (seed % 300) as usize,
            _ => (seed % 40) as usize,
        };
        (0..len).map(|i| (seed as usize + i) as u8).collect()
    }

    fn check(slab: &RecordSlab, model: &BTreeMap<usize, Vec<u8>>, len: usize) {
        assert_eq!(slab.len(), len);
        for i in 0..len + 3 {
            assert_eq!(slab.get(i), model.get(&i).map(|v| v.as_slice()), "slot {i}");
            assert_eq!(slab.contains(i), model.contains_key(&i));
        }
        let all: Vec<(usize, Vec<u8>)> = slab.iter().map(|(i, b)| (i, b.to_vec())).collect();
        let expected: Vec<(usize, Vec<u8>)> = model.iter().map(|(&i, v)| (i, v.clone())).collect();
        assert_eq!(all, expected);
        for from in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, len / 2, len] {
            assert!(slab
                .iter_from(from)
                .map(|(i, _)| i)
                .eq(model.range(from..).map(|(&i, _)| i)));
        }
    }

    #[test]
    fn matches_a_map_through_sets_removes_and_clones() {
        let mut slab = RecordSlab::default();
        let mut model = BTreeMap::new();
        let len = CHUNK * 3 + 17;
        // Fill in id order, as creates do.
        for i in 0..len {
            slab.grow_to(i + 1);
            if i % 11 != 3 {
                let r = record(i as u64);
                assert!(slab.set(i, &r));
                model.insert(i, r);
            }
        }
        check(&slab, &model, len);
        let snapshot = slab.clone();
        let snapshot_model = model.clone();
        // Rewrite and remove at random: garbage builds up and leaves compact.
        for step in 0..20_000u64 {
            let i = (splitmix(step) % len as u64) as usize;
            if step % 4 == 0 {
                assert_eq!(slab.remove(i), model.remove(&i).is_some());
            } else {
                let r = record(splitmix(step ^ 0xFEED));
                assert_eq!(slab.set(i, &r), model.insert(i, r).is_none());
            }
            if step % 997 == 0 {
                // A clone in the middle; writes after it copy leaves again.
                let copy = slab.clone();
                check(&copy, &model, len);
            }
        }
        check(&slab, &model, len);
        check(&snapshot, &snapshot_model, len);
        for i in 0..len {
            slab.remove(i);
        }
        assert!(slab.iter().next().is_none());
    }

    #[test]
    fn grows_across_interior_boundaries() {
        let mut slab = RecordSlab::default();
        let span = CHUNK * FANOUT;
        slab.grow_to(span + 5);
        assert!(slab.set(span + 4, b"far"));
        assert!(slab.set(0, b"near"));
        assert_eq!(slab.get(span + 4), Some(&b"far"[..]));
        assert_eq!(slab.get(0), Some(&b"near"[..]));
        assert_eq!(slab.get(span + 5), None);
        assert_eq!(slab.iter().count(), 2);
        slab.grow_to(3);
        assert_eq!(slab.len(), span + 5);
    }

    #[test]
    fn a_full_leaf_of_small_records_costs_little_beyond_its_bytes() {
        let mut slab = RecordSlab::default();
        let n = CHUNK * 8;
        for i in 0..n {
            slab.grow_to(i + 1);
            slab.set(i, &[7u8; 11]);
        }
        // 11 bytes and a length byte per record, plus a 4-byte offset.
        let per_record = slab.heap_bytes() as f64 / n as f64;
        assert!(per_record < 17.5, "{per_record}");
    }
}
