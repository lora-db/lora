//! Slot-indexed storage for encoded records, a leaf of records per
//! allocation.
//!
//! Nodes and relationships are stored as encoded byte strings (see
//! [`crate::encoded`]), indexed by id. Each record used to be its own
//! allocation behind an 8-byte slot: an 8-byte header, the bytes, and the
//! allocator's rounding to its 16-byte size step, which on a
//! relationship of 11 bytes is two thirds overhead. Here the records of
//! [`CHUNK`] consecutive ids share one leaf: a table of `u32` offsets and
//! [`SEGMENTS`] buffers, one per [`SEG_SLOTS`] slots, each holding its
//! records behind a varint length.
//!
//! ```text
//! root: Arc<[node]>  →  node: Arc<[leaf]>  →  leaf: Arc<Leaf>
//!                                              offsets:  [u32; CHUNK]
//!                                              segments: [Arc<[u8]>; 8]
//!                                                        len, bytes, len, bytes, ...
//! ```
//!
//! The tree above the leaves is [`super::chunked_vec`]'s: cloning bumps
//! the root's refcount, and the first write after a clone copies the
//! root table, one interior node and the one leaf it touches. Copying a
//! leaf copies its offsets and shares its segments; the write then copies
//! the one segment it lands in. With one buffer per leaf, that write
//! copied all 512 records, which on wide nodes made a staged property
//! write several times slower.
//!
//! A record is never changed in place. Storing a new version appends it
//! to its segment and leaves the old bytes as garbage until the segment
//! is rebuilt: when it is full, shared with a clone, or (on a remove)
//! mostly garbage. An update that does not fit its segment is kept loose,
//! in an allocation of its own, rather than copying the segment's
//! records (up to [`LOOSE_MAX`] per leaf); the segment's next rebuild
//! folds it back in. Records of [`LARGE_MIN`] bytes or more are always
//! loose, so a segment stays small enough for `u32` offsets.

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
/// Set on the offset of a record held outside the segments; the other
/// bits index [`Leaf::loose`].
const LOOSE: u32 = 1 << 31;
/// Records at least this long are always loose. With them out, a segment
/// holds at most `SEG_SLOTS * LARGE_MIN` bytes of live records (4 MiB)
/// and its spare room, well within the 31 bits of an offset.
const LARGE_MIN: usize = 1 << 16;
/// Updates a leaf keeps loose, at most, before an update that does not
/// fit its segment rebuilds the segment instead.
const LOOSE_MAX: u32 = 32;

#[inline]
fn varint_len(value: usize) -> usize {
    (64 - (value as u64 | 1).leading_zeros() as usize).div_ceil(7)
}

#[inline]
fn put_varint(out: &mut [u8], pos: &mut usize, mut value: usize) {
    while value >= 0x80 {
        out[*pos] = value as u8 | 0x80;
        *pos += 1;
        value >>= 7;
    }
    out[*pos] = value as u8;
    *pos += 1;
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

/// Slots per segment of a leaf's buffer.
const SEG_SLOTS: usize = 64;
const SEG_SHIFT: u32 = SEG_SLOTS.trailing_zeros();
const SEGMENTS: usize = CHUNK / SEG_SLOTS;

/// The records of [`SEG_SLOTS`] consecutive slots: a fixed-size buffer,
/// filled from the front and shared between leaf copies until one of
/// them writes to it.
#[derive(Clone, Default)]
struct Segment {
    buf: Option<Arc<[u8]>>,
    /// Bytes of `buf` in use.
    len: u32,
    /// Bytes in use that no slot refers to.
    garbage: u32,
}

impl Segment {
    #[inline]
    fn bytes(&self) -> &[u8] {
        self.buf.as_deref().unwrap_or(&[])
    }
}

#[derive(Clone)]
struct Leaf {
    /// Each slot's offset into its segment, or `LOOSE | index`.
    offsets: [u32; CHUNK],
    segments: [Segment; SEGMENTS],
    /// Records held in allocations of their own: those of [`LARGE_MIN`]
    /// bytes or more, and updates that did not fit their segment. `None`
    /// once released.
    loose: Vec<Option<Arc<[u8]>>>,
    /// Entries of `loose` that no slot refers to.
    loose_garbage: u32,
    /// Live loose records shorter than [`LARGE_MIN`]: the updates.
    loose_small: u32,
    /// Records stored.
    live: u32,
}

impl Leaf {
    fn new() -> Self {
        Leaf {
            offsets: [EMPTY; CHUNK],
            segments: Default::default(),
            loose: Vec::new(),
            loose_garbage: 0,
            loose_small: 0,
            live: 0,
        }
    }

    #[inline]
    fn get(&self, slot: usize) -> Option<&[u8]> {
        let offset = self.offsets[slot];
        if offset == EMPTY {
            return None;
        }
        if offset & LOOSE != 0 {
            return self.loose[(offset & !LOOSE) as usize].as_deref();
        }
        let buf = self.segments[slot >> SEG_SHIFT].bytes();
        let mut pos = offset as usize;
        let len = get_varint(buf, &mut pos);
        Some(&buf[pos..pos + len])
    }

    /// Account for the record in `slot` becoming unreachable.
    fn release(&mut self, slot: usize) {
        let offset = self.offsets[slot];
        if offset == EMPTY {
            return;
        }
        if offset & LOOSE != 0 {
            let record = self.loose[(offset & !LOOSE) as usize].take();
            if record.is_some_and(|record| record.len() < LARGE_MIN) {
                self.loose_small -= 1;
            }
            self.loose_garbage += 1;
        } else {
            let segment = &mut self.segments[slot >> SEG_SHIFT];
            let mut pos = offset as usize;
            let len = get_varint(segment.bytes(), &mut pos);
            segment.garbage += (pos - offset as usize + len) as u32;
        }
        self.offsets[slot] = EMPTY;
        self.live -= 1;
    }

    fn set(&mut self, slot: usize, bytes: &[u8]) {
        let update = self.offsets[slot] != EMPTY;
        self.release(slot);
        self.live += 1;
        if bytes.len() >= LARGE_MIN {
            self.push_loose(slot, bytes);
            return;
        }
        let seg = slot >> SEG_SHIFT;
        let need = varint_len(bytes.len()) + bytes.len();
        // `Arc::get_mut` is an atomic read-modify-write: one per record.
        if self.append(slot, bytes, need) {
            return;
        }
        // An update that does not fit its segment (a full one, or one a
        // clone shares) is kept loose instead of copying the segment's 64
        // records: one allocation, as when every record was its own. The
        // segment's next rebuild folds it back in.
        if update && self.loose_small < LOOSE_MAX {
            self.loose_small += 1;
            self.push_loose(slot, bytes);
            return;
        }
        self.rebuild(seg, need);
        let appended = self.append(slot, bytes, need);
        assert!(appended, "rebuild leaves the segment unshared, with room");
    }

    /// Append `bytes` (`need` bytes with their length) to `slot`'s
    /// segment, if the segment is unshared and has room.
    #[inline]
    fn append(&mut self, slot: usize, bytes: &[u8], need: usize) -> bool {
        let segment = &mut self.segments[slot >> SEG_SHIFT];
        let start = segment.len as usize;
        let Some(buf) = segment
            .buf
            .as_mut()
            .and_then(Arc::get_mut)
            .filter(|buf| buf.len() - start >= need)
        else {
            return false;
        };
        let mut pos = start;
        put_varint(buf, &mut pos, bytes.len());
        buf[pos..pos + bytes.len()].copy_from_slice(bytes);
        segment.len = (pos + bytes.len()) as u32;
        self.offsets[slot] = start as u32;
        true
    }

    fn push_loose(&mut self, slot: usize, bytes: &[u8]) {
        if self.loose_garbage as usize > self.loose.len() / 2 {
            self.compact_loose();
        }
        self.offsets[slot] = LOOSE | self.loose.len() as u32;
        self.loose.push(Some(bytes.into()));
    }

    /// The loose record at `offset`, if it is an update that a rebuild
    /// folds back into its segment.
    #[inline]
    fn foldable(&self, offset: u32) -> Option<&[u8]> {
        self.loose[(offset & !LOOSE) as usize]
            .as_deref()
            .filter(|record| record.len() < LARGE_MIN)
    }

    fn remove(&mut self, slot: usize) -> bool {
        let offset = self.offsets[slot];
        if offset == EMPTY {
            return false;
        }
        self.release(slot);
        if self.live == 0 {
            *self = Leaf::new();
        } else if offset & LOOSE == 0 {
            let seg = slot >> SEG_SHIFT;
            let segment = &self.segments[seg];
            if segment.garbage == segment.len {
                self.segments[seg] = Segment::default();
            } else if segment.garbage > 4096 && segment.garbage > segment.len / 2 {
                self.rebuild(seg, 0);
            }
        }
        true
    }

    /// Copy segment `seg`'s reachable records, and its slots' loose
    /// updates, into a buffer of its own in slot order, with room for
    /// `need` more bytes. Run when the segment is full, shared with a
    /// clone, or mostly garbage.
    fn rebuild(&mut self, seg: usize, need: usize) {
        let slots = seg * SEG_SLOTS..(seg + 1) * SEG_SLOTS;
        let segment = &self.segments[seg];
        let mut live_bytes = (segment.len - segment.garbage) as usize;
        let mut live = 0;
        for &offset in &self.offsets[slots.clone()] {
            if offset == EMPTY {
                continue;
            }
            if offset & LOOSE == 0 {
                live += 1;
            } else if let Some(record) = self.foldable(offset) {
                live += 1;
                live_bytes += varint_len(record.len()) + record.len();
            }
        }
        let spare = if need == 0 {
            0
        } else {
            // Expect the empty slots to fill with records like the present
            // ones (ids are handed out in order, so a segment fills front
            // to back), but reserve for at most as many again as are
            // stored, so a mostly empty segment grows geometrically. A
            // full segment that is being rewritten grows by an eighth.
            let empty = SEG_SLOTS.saturating_sub(live + 1);
            if live == 0 && self.live > 1 {
                // A fresh segment of a leaf that already holds records: the
                // segments before it filled the same way, so size it for
                // all its slots at once from their average.
                let leaf_bytes: usize = self
                    .segments
                    .iter()
                    .map(|segment| (segment.len - segment.garbage) as usize)
                    .sum();
                let leaf_live = (self.live - 1) as usize;
                empty * (leaf_bytes / leaf_live).max(1)
            } else {
                let avg = live_bytes.checked_div(live).unwrap_or(need);
                (empty.min(live.max(1)) * avg).max(live_bytes / 8)
            }
        };
        let mut buf: Arc<[u8]> = std::iter::repeat_n(0, live_bytes + need + spare).collect();
        let out = Arc::get_mut(&mut buf).expect("a new buffer is unshared");
        let old = self.segments[seg].bytes();
        let mut end = 0;
        for slot in slots {
            let offset = self.offsets[slot];
            if offset == EMPTY {
                continue;
            }
            if offset & LOOSE != 0 {
                let index = (offset & !LOOSE) as usize;
                let Some(record) = self.loose[index]
                    .as_deref()
                    .filter(|record| record.len() < LARGE_MIN)
                else {
                    continue;
                };
                let mut pos = end;
                put_varint(out, &mut pos, record.len());
                out[pos..pos + record.len()].copy_from_slice(record);
                self.offsets[slot] = end as u32;
                end = pos + record.len();
                self.loose[index] = None;
                self.loose_garbage += 1;
                self.loose_small -= 1;
                continue;
            }
            let start = offset as usize;
            let mut pos = start;
            let len = get_varint(old, &mut pos);
            let n = pos + len - start;
            out[end..end + n].copy_from_slice(&old[start..start + n]);
            self.offsets[slot] = end as u32;
            end += n;
        }
        self.segments[seg] = Segment {
            buf: Some(buf),
            len: end as u32,
            garbage: 0,
        };
    }

    /// Drop the released entries of `loose`.
    fn compact_loose(&mut self) {
        let mut loose = Vec::with_capacity(self.loose.len() - self.loose_garbage as usize);
        for offset in &mut self.offsets {
            if *offset != EMPTY && *offset & LOOSE != 0 {
                let record = self.loose[(*offset & !LOOSE) as usize].take();
                *offset = LOOSE | loose.len() as u32;
                loose.push(record);
            }
        }
        self.loose = loose;
        self.loose_garbage = 0;
    }

    fn heap_bytes(&self) -> usize {
        const ARC_HEADER: usize = 2 * size_of::<usize>();
        self.segments
            .iter()
            .filter_map(|segment| segment.buf.as_ref())
            .map(|buf| ARC_HEADER + buf.len())
            .sum::<usize>()
            + self.loose.capacity() * size_of::<Option<Arc<[u8]>>>()
            + self
                .loose
                .iter()
                .flatten()
                .map(|record| ARC_HEADER + record.len())
                .sum::<usize>()
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

    /// The slots from `from` on that hold a record, in order. Cheaper than
    /// [`Self::iter_from`] when the bytes are not needed.
    pub(super) fn slots_from(&self, from: usize) -> impl Iterator<Item = usize> + '_ {
        let leaves = self.len.div_ceil(CHUNK);
        let first = from / CHUNK;
        (first..leaves).flat_map(move |l| {
            let leaf = self.leaf(l * CHUNK);
            let start = if l == first { from & MASK } else { 0 };
            let end = (self.len - l * CHUNK).min(CHUNK);
            (start..end)
                .filter(move |&s| leaf.offsets[s] != EMPTY)
                .map(move |s| l * CHUNK + s)
        })
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
            assert!(slab
                .slots_from(from)
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

    /// Whether segment `seg` of leaf 0 is the same buffer in both slabs.
    fn shares_segment(a: &RecordSlab, b: &RecordSlab, seg: usize) -> bool {
        match (
            &a.root[0][0].segments[seg].buf,
            &b.root[0][0].segments[seg].buf,
        ) {
            (Some(x), Some(y)) => Arc::ptr_eq(x, y),
            _ => false,
        }
    }

    fn full_leaf() -> RecordSlab {
        let mut slab = RecordSlab::default();
        slab.grow_to(CHUNK);
        for i in 0..CHUNK {
            slab.set(i, &[i as u8; 40]);
        }
        slab
    }

    #[test]
    fn a_write_after_a_clone_copies_one_segment_at_most() {
        let mut slab = full_leaf();
        let snapshot = slab.clone();
        // An update is kept loose: no segment is copied.
        slab.set(SEG_SLOTS + 3, b"new");
        assert!(!Arc::ptr_eq(&slab.root[0][0], &snapshot.root[0][0]));
        assert!((0..SEGMENTS).all(|seg| shares_segment(&slab, &snapshot, seg)));
        assert_eq!(slab.get(SEG_SLOTS + 3), Some(&b"new"[..]));
        assert_eq!(
            snapshot.get(SEG_SLOTS + 3),
            Some(&[(SEG_SLOTS + 3) as u8; 40][..])
        );
        // Emptying a slot and filling it again rebuilds its segment, which
        // takes the loose update back in; the other segments stay shared.
        slab.remove(SEG_SLOTS + 5);
        slab.set(SEG_SLOTS + 5, b"again");
        for seg in 0..SEGMENTS {
            assert_eq!(
                shares_segment(&slab, &snapshot, seg),
                seg != 1,
                "segment {seg}"
            );
        }
        assert_eq!(slab.root[0][0].loose_small, 0);
        assert_eq!(slab.get(SEG_SLOTS + 3), Some(&b"new"[..]));
        assert_eq!(slab.get(SEG_SLOTS + 5), Some(&b"again"[..]));
    }

    #[test]
    fn updates_past_the_loose_cap_rebuild_their_segment() {
        let mut slab = full_leaf();
        // The leaf is full and sized to fit: every update overflows.
        for i in 0..LOOSE_MAX as usize {
            slab.set(i * 7, &[0xAB; 41]);
        }
        assert_eq!(slab.root[0][0].loose_small, LOOSE_MAX);
        // One more rebuilds segment 0, folding back its loose records.
        let folded = (0..LOOSE_MAX as usize)
            .filter(|i| i * 7 < SEG_SLOTS)
            .count() as u32;
        slab.set(1, &[0xCD; 41]);
        assert_eq!(slab.root[0][0].loose_small, LOOSE_MAX - folded);
        for i in 0..CHUNK {
            let expected = if i == 1 {
                vec![0xCD; 41]
            } else if i % 7 == 0 && i / 7 < LOOSE_MAX as usize {
                vec![0xAB; 41]
            } else {
                vec![i as u8; 40]
            };
            assert_eq!(slab.get(i), Some(expected.as_slice()), "slot {i}");
        }
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
