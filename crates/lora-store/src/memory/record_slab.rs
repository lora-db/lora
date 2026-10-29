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

use super::ARC_HEADER;
use std::sync::Arc;

use super::chunked_vec::{CHUNK, FANOUT};
use crate::varint;

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
/// Released entries `loose` may hold before a push compacts it (when
/// they are also half of it), so a leaf cycling one loose record does not
/// rescan its offsets on every update.
const LOOSE_SLACK: u32 = 8;

/// Slots per segment of a leaf's buffer.
const SEG_SLOTS: usize = 64;
const SEG_SHIFT: u32 = SEG_SLOTS.trailing_zeros();
const SEGMENTS: usize = CHUNK / SEG_SLOTS;

/// The most bytes a segment buffer is ever sized for: live records and
/// the new one (each under `LARGE_MIN` plus a 3-byte length), and spare
/// room for as much again. Offsets into it must stay below [`LOOSE`].
const SEG_MAX: usize = 2 * (SEG_SLOTS + 1) * (LARGE_MIN + 3);
const _: () = assert!(SEG_MAX < LOOSE as usize);
const _: () = assert!(CHUNK.is_multiple_of(SEG_SLOTS) && SEG_SLOTS.is_power_of_two());
/// A leaf's loose entries (at most one live per slot, released ones at
/// most as many again plus the slack) must be indexable below [`LOOSE`].
const _: () = assert!(2 * CHUNK + LOOSE_SLACK as usize + 1 < LOOSE as usize);

/// Where a slot's record is, decoded from its offset.
#[derive(Clone, Copy)]
enum Place {
    Empty,
    /// Byte offset into the slot's segment.
    Segment(usize),
    /// Index into [`Leaf::loose`].
    Loose(usize),
}

impl Place {
    #[inline]
    fn of(offset: u32) -> Place {
        if offset == EMPTY {
            Place::Empty
        } else if offset & LOOSE != 0 {
            Place::Loose((offset & !LOOSE) as usize)
        } else {
            Place::Segment(offset as usize)
        }
    }
}

#[inline]
fn segment_of(slot: usize) -> usize {
    slot >> SEG_SHIFT
}

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

    #[inline]
    fn live_bytes(&self) -> usize {
        (self.len - self.garbage) as usize
    }

    /// The record at byte `offset`: its length prefix and bytes.
    #[inline]
    fn record(&self, offset: usize) -> &[u8] {
        let bytes = self.bytes();
        let mut pos = offset;
        let len = varint::read_len(bytes, &mut pos);
        &bytes[pos..pos + len]
    }

    /// Bytes the record at `offset` takes, its length prefix included.
    #[inline]
    fn stored_len(&self, offset: usize) -> usize {
        let mut pos = offset;
        let len = varint::read_len(self.bytes(), &mut pos);
        pos - offset + len
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
        match Place::of(self.offsets[slot]) {
            Place::Empty => None,
            Place::Segment(offset) => Some(self.segments[segment_of(slot)].record(offset)),
            Place::Loose(index) => self.loose[index].as_deref(),
        }
    }

    #[inline]
    fn contains(&self, slot: usize) -> bool {
        self.offsets[slot] != EMPTY
    }

    /// Account for the record in `slot` becoming unreachable.
    fn release(&mut self, slot: usize) {
        match Place::of(self.offsets[slot]) {
            Place::Empty => return,
            Place::Segment(offset) => {
                let segment = &mut self.segments[segment_of(slot)];
                segment.garbage += segment.stored_len(offset) as u32;
            }
            Place::Loose(index) => {
                let record = self.loose[index].take();
                if record.is_some_and(|record| record.len() < LARGE_MIN) {
                    self.loose_small -= 1;
                }
                self.loose_garbage += 1;
            }
        }
        self.offsets[slot] = EMPTY;
        self.live -= 1;
    }

    fn set(&mut self, slot: usize, bytes: &[u8]) {
        let update = self.contains(slot);
        self.release(slot);
        self.live += 1;
        if bytes.len() >= LARGE_MIN {
            self.push_loose(slot, bytes);
            return;
        }
        let need = varint::len(bytes.len() as u64) + bytes.len();
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
        self.rebuild(segment_of(slot), need);
        let appended = self.append(slot, bytes, need);
        assert!(appended, "rebuild leaves the segment unshared, with room");
    }

    /// Append `bytes` (`need` bytes with their length) to `slot`'s
    /// segment, if the segment is unshared and has room.
    #[inline]
    fn append(&mut self, slot: usize, bytes: &[u8], need: usize) -> bool {
        let segment = &mut self.segments[segment_of(slot)];
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
        varint::write(buf, &mut pos, bytes.len() as u64);
        buf[pos..pos + bytes.len()].copy_from_slice(bytes);
        segment.len = (pos + bytes.len()) as u32;
        self.offsets[slot] = start as u32;
        true
    }

    fn push_loose(&mut self, slot: usize, bytes: &[u8]) {
        if self.loose_garbage >= LOOSE_SLACK && self.loose_garbage as usize > self.loose.len() / 2 {
            self.compact_loose();
        }
        self.offsets[slot] = LOOSE | self.loose.len() as u32;
        self.loose.push(Some(bytes.into()));
    }

    /// The loose record at `index`, if it is an update that a rebuild
    /// folds back into its segment.
    #[inline]
    fn foldable(&self, index: usize) -> Option<&[u8]> {
        self.loose[index]
            .as_deref()
            .filter(|record| record.len() < LARGE_MIN)
    }

    fn remove(&mut self, slot: usize) -> bool {
        let place = Place::of(self.offsets[slot]);
        if matches!(place, Place::Empty) {
            return false;
        }
        self.release(slot);
        if self.live == 0 {
            *self = Leaf::new();
        } else if let Place::Segment(_) = place {
            let seg = segment_of(slot);
            let segment = &self.segments[seg];
            if segment.garbage == segment.len {
                self.segments[seg] = Segment::default();
            } else if segment.garbage > 4096 && segment.garbage > segment.len / 2 {
                self.rebuild(seg, 0);
            }
        }
        true
    }

    /// Slots of segment `seg`.
    #[inline]
    fn slots(seg: usize) -> std::ops::Range<usize> {
        seg * SEG_SLOTS..(seg + 1) * SEG_SLOTS
    }

    /// Spare room for a rebuilt segment `seg` that will hold `live`
    /// records in `live_bytes` and then one more of `need` bytes.
    fn spare(&self, seg: usize, live: usize, live_bytes: usize, need: usize) -> usize {
        if need == 0 {
            // A compaction after removes: nothing is about to be added.
            return 0;
        }
        // Expect the empty slots to fill with records like the present
        // ones (ids are handed out in order, so a segment fills front to
        // back), but reserve for at most as many again as are stored, so
        // a mostly empty segment grows geometrically. A full segment that
        // is being rewritten grows by an eighth.
        let empty = SEG_SLOTS.saturating_sub(live + 1);
        let others = (self.live - 1) as usize;
        if live == 0 && others > 0 {
            // A fresh segment of a leaf that already holds records: the
            // segments before it filled the same way, so size it for all
            // its slots at once from their average.
            let leaf_bytes: usize = self.segments.iter().map(Segment::live_bytes).sum();
            debug_assert_eq!(self.segments[seg].live_bytes(), 0);
            empty * (leaf_bytes / others).max(1)
        } else {
            let avg = live_bytes.checked_div(live).unwrap_or(need);
            (empty.min(live.max(1)) * avg).max(live_bytes / 8)
        }
    }

    /// Copy segment `seg`'s reachable records, and its slots' loose
    /// updates, into a buffer of its own in slot order, with room for
    /// `need` more bytes. Run when the segment is full, shared with a
    /// clone, or mostly garbage.
    fn rebuild(&mut self, seg: usize, need: usize) {
        let mut live_bytes = self.segments[seg].live_bytes();
        let mut live = 0;
        for &offset in &self.offsets[Self::slots(seg)] {
            match Place::of(offset) {
                Place::Empty => {}
                Place::Segment(_) => live += 1,
                Place::Loose(index) => {
                    if let Some(record) = self.foldable(index) {
                        live += 1;
                        live_bytes += varint::len(record.len() as u64) + record.len();
                    }
                }
            }
        }
        let size = live_bytes + need + self.spare(seg, live, live_bytes, need);
        debug_assert!(size <= SEG_MAX, "segment of {size} bytes");
        let mut buf: Arc<[u8]> = std::iter::repeat_n(0, size).collect();
        let out = Arc::get_mut(&mut buf).expect("a new buffer is unshared");
        let old = &self.segments[seg];
        let mut end = 0;
        for slot in Self::slots(seg) {
            let (record, folded) = match Place::of(self.offsets[slot]) {
                Place::Empty => continue,
                Place::Segment(offset) => (old.record(offset), None),
                Place::Loose(index) => match self.loose[index].as_deref() {
                    Some(record) if record.len() < LARGE_MIN => (record, Some(index)),
                    _ => continue,
                },
            };
            self.offsets[slot] = end as u32;
            varint::write(out, &mut end, record.len() as u64);
            out[end..end + record.len()].copy_from_slice(record);
            end += record.len();
            if let Some(index) = folded {
                self.loose[index] = None;
                self.loose_small -= 1;
                self.loose_garbage += 1;
            }
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
            if let Place::Loose(index) = Place::of(*offset) {
                *offset = LOOSE | loose.len() as u32;
                loose.push(self.loose[index].take());
            }
        }
        self.loose = loose;
        self.loose_garbage = 0;
    }

    fn heap_bytes(&self) -> usize {
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

    /// Panic unless the leaf's counters and offsets agree with each other.
    #[cfg(test)]
    fn check_invariants(&self) {
        let mut live = 0;
        let mut loose_small = 0;
        let mut referenced = vec![false; self.loose.len()];
        let mut used = [0usize; SEGMENTS];
        for (slot, &offset) in self.offsets.iter().enumerate() {
            match Place::of(offset) {
                Place::Empty => continue,
                Place::Segment(offset) => {
                    let segment = &self.segments[segment_of(slot)];
                    let stored = segment.stored_len(offset);
                    assert!(
                        offset + stored <= segment.len as usize,
                        "slot {slot} past len"
                    );
                    used[segment_of(slot)] += stored;
                }
                Place::Loose(index) => {
                    let record = self.loose[index].as_deref().expect("live loose entry");
                    assert!(
                        !referenced[index],
                        "loose entry {index} shared by two slots"
                    );
                    referenced[index] = true;
                    loose_small += usize::from(record.len() < LARGE_MIN);
                }
            }
            live += 1;
        }
        assert_eq!(live, self.live as usize, "live");
        assert_eq!(loose_small, self.loose_small as usize, "loose_small");
        assert!(self.loose_small <= LOOSE_MAX);
        let released = self.loose.iter().filter(|r| r.is_none()).count();
        assert_eq!(released, self.loose_garbage as usize, "loose_garbage");
        assert!(
            self.loose
                .iter()
                .zip(&referenced)
                .all(|(r, &seen)| r.is_none() || seen),
            "an unreferenced live loose entry"
        );
        for (seg, segment) in self.segments.iter().enumerate() {
            assert!(segment.len as usize <= segment.bytes().len());
            assert_eq!(used[seg], segment.live_bytes(), "segment {seg} live bytes");
        }
    }
}

type Interior = Arc<[Arc<Leaf>]>;

/// The one leaf every empty leaf position points at. A leaf whose last
/// record is removed is swapped for it, so the ids a delete-heavy workload
/// leaves behind cost a pointer per leaf rather than a 2 KiB leaf, and
/// scans skip them without reading their offsets. The first write to such
/// a position copies it, as any write to a shared leaf does.
fn empty_leaf() -> &'static Arc<Leaf> {
    static EMPTY_LEAF: std::sync::OnceLock<Arc<Leaf>> = std::sync::OnceLock::new();
    EMPTY_LEAF.get_or_init(|| Arc::new(Leaf::new()))
}

/// Records in whole leaves, built apart from any slab (on another thread)
/// for [`RecordSlab::put_leaves`]. Slots must be pushed in ascending order.
///
/// A segment's records are staged in a buffer the builder reuses and get
/// one allocation of their exact size once the segment is complete: filed
/// through [`Leaf::set`], a leaf's first segment grew by rebuilds (300
/// bytes, 600, ... 10 KiB), each copying the records so far.
#[derive(Default)]
pub(super) struct BuiltLeaves {
    leaves: Vec<(usize, Arc<Leaf>)>,
    last: Option<usize>,
    records: usize,
    /// The segment being staged: `(leaf position, segment)`.
    staging: Option<(usize, usize)>,
    stage: Vec<u8>,
}

impl BuiltLeaves {
    /// Add `bytes` at slot `i`, above every slot pushed before.
    pub(super) fn push(&mut self, i: usize, bytes: &[u8]) {
        debug_assert!(self.last.is_none_or(|last| last < i));
        let at = i >> SHIFT;
        let slot = i & MASK;
        let seg = segment_of(slot);
        if self.staging != Some((at, seg)) {
            self.flush();
            self.staging = Some((at, seg));
        }
        if self.leaves.last().is_none_or(|(l, _)| *l != at) {
            self.leaves.push((at, Arc::new(Leaf::new())));
        }
        let (_, leaf) = self.leaves.last_mut().expect("pushed above");
        let leaf = Arc::get_mut(leaf).expect("a built leaf is unshared");
        if bytes.len() >= LARGE_MIN {
            leaf.live += 1;
            leaf.push_loose(slot, bytes);
        } else {
            let mut pos = self.stage.len();
            self.stage
                .resize(pos + varint::len(bytes.len() as u64) + bytes.len(), 0);
            leaf.offsets[slot] = pos as u32;
            varint::write(&mut self.stage, &mut pos, bytes.len() as u64);
            self.stage[pos..].copy_from_slice(bytes);
            leaf.live += 1;
        }
        self.last = Some(i);
        self.records += 1;
    }

    /// Give the staged segment its buffer.
    fn flush(&mut self) {
        let Some((at, seg)) = self.staging.take() else {
            return;
        };
        if self.stage.is_empty() {
            return;
        }
        let (l, leaf) = self
            .leaves
            .last_mut()
            .expect("a staged segment has its leaf");
        debug_assert_eq!(*l, at);
        let leaf = Arc::get_mut(leaf).expect("a built leaf is unshared");
        leaf.segments[seg] = Segment {
            buf: Some(Arc::from(self.stage.as_slice())),
            len: self.stage.len() as u32,
            garbage: 0,
        };
        self.stage.clear();
    }

    /// The leaves, every segment given its buffer.
    fn into_leaves(mut self) -> Vec<(usize, Arc<Leaf>)> {
        self.flush();
        self.leaves
    }

    /// The highest slot pushed.
    pub(super) fn last_slot(&self) -> Option<usize> {
        self.last
    }

    pub(super) fn len(&self) -> usize {
        self.records
    }
}

/// Encoded records by slot index. Empty slots read as `None`.
pub(super) struct RecordSlab {
    root: Arc<[Interior]>,
    /// Slots, empty or not. Leaf positions exist for all of them; one
    /// without records may hold the shared [`empty_leaf`].
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

    /// The leaf holding slot `i`. Leaves exist for every slot below
    /// `len`, packed from the front; callers check `i < len`.
    #[inline]
    fn leaf(&self, i: usize) -> &Leaf {
        debug_assert!(i < self.len);
        &self.root[i >> SPAN_SHIFT][(i >> SHIFT) & FAN_MASK]
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
        i < self.len && self.leaf(i).contains(i & MASK)
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
        let was_empty = !leaf.contains(i & MASK);
        leaf.set(i & MASK, bytes);
        was_empty
    }

    /// Store the records of `built` (every slot below [`Self::len`]). A
    /// leaf position that holds no records takes its built leaf as is;
    /// one that does takes the records one by one. Returns how many
    /// records were stored, or the first slot that already held one.
    pub(super) fn put_leaves(&mut self, built: BuiltLeaves) -> Result<usize, usize> {
        let empty = empty_leaf();
        let mut stored = 0;
        for (at, leaf) in built.into_leaves() {
            let i = at * CHUNK;
            assert!(i < self.len, "record leaf {at} out of bounds");
            let node = &mut Arc::make_mut(&mut self.root)[i >> SPAN_SHIFT];
            let place = &mut Arc::make_mut(node)[at & FAN_MASK];
            if Arc::ptr_eq(place, empty) || place.live == 0 {
                stored += leaf.live as usize;
                *place = leaf;
                continue;
            }
            let target = Arc::make_mut(place);
            for slot in 0..CHUNK {
                if let Some(bytes) = leaf.get(slot) {
                    if target.contains(slot) {
                        return Err(i + slot);
                    }
                    target.set(slot, bytes);
                    stored += 1;
                }
            }
        }
        Ok(stored)
    }

    /// Empty slot `i`. Returns whether it held a record.
    pub(super) fn remove(&mut self, i: usize) -> bool {
        if !self.contains(i) {
            return false;
        }
        let leaf = self.leaf_mut(i);
        let removed = leaf.remove(i & MASK);
        if leaf.live == 0 {
            // `leaf_mut` left the path to this leaf unshared.
            let node = &mut Arc::make_mut(&mut self.root)[i >> SPAN_SHIFT];
            Arc::make_mut(node)[(i >> SHIFT) & FAN_MASK] = Arc::clone(empty_leaf());
        }
        removed
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
            let leaf = Arc::clone(empty_leaf());
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

    /// Each leaf from the one holding slot `from` on: its first slot and
    /// the range of its slots at or after `from` and below `len`.
    fn leaves_from(
        &self,
        from: usize,
    ) -> impl Iterator<Item = (usize, &Leaf, std::ops::Range<usize>)> + '_ {
        let first = from / CHUNK;
        // A pointer test, not `leaf.live`: that field sits past the offsets,
        // on a cache line a scan of a full leaf would not otherwise touch.
        let empty: *const Leaf = &**empty_leaf();
        (first..self.len.div_ceil(CHUNK))
            .map(move |l| (l, self.leaf(l * CHUNK)))
            .filter(move |&(_, leaf)| !std::ptr::eq(leaf, empty))
            .map(move |(l, leaf)| {
                let base = l * CHUNK;
                let start = if l == first { from & MASK } else { 0 };
                let end = (self.len - base).min(CHUNK);
                (base, leaf, start..end)
            })
    }

    /// `(slot, bytes)` of every record from slot `from` on, in slot order.
    pub(super) fn iter_from(&self, from: usize) -> impl Iterator<Item = (usize, &[u8])> + '_ {
        self.leaves_from(from).flat_map(|(base, leaf, slots)| {
            slots.filter_map(move |s| leaf.get(s).map(|bytes| (base + s, bytes)))
        })
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &[u8])> + '_ {
        self.iter_from(0)
    }

    /// The slots from `from` on that hold a record, in order. Cheaper than
    /// [`Self::iter_from`] when the bytes are not needed.
    pub(super) fn slots_from(&self, from: usize) -> impl Iterator<Item = usize> + '_ {
        self.leaves_from(from).flat_map(|(base, leaf, slots)| {
            slots
                .filter(move |&s| leaf.contains(s))
                .map(move |s| base + s)
        })
    }

    /// Heap bytes: the tree, every leaf and its buffers.
    pub(super) fn heap_bytes(&self) -> usize {
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
            .filter(|leaf| !Arc::ptr_eq(leaf, empty_leaf()))
            .map(|leaf| ARC_HEADER + size_of::<Leaf>() + leaf.heap_bytes())
            .sum();
        tree + leaves
    }

    /// Panic unless the tree's shape matches `len` and every leaf is
    /// consistent.
    #[cfg(test)]
    fn check_invariants(&self) {
        let leaves = self.len.div_ceil(CHUNK);
        assert_eq!(self.root.len(), leaves.div_ceil(FANOUT), "interior nodes");
        for (n, node) in self.root.iter().enumerate() {
            let expected = if n + 1 < self.root.len() {
                FANOUT
            } else {
                leaves - n * FANOUT
            };
            assert_eq!(node.len(), expected, "leaves of node {n}");
        }
        for (base, leaf, slots) in self.leaves_from(0) {
            leaf.check_invariants();
            assert!(
                leaf.live > 0 || std::ptr::eq(leaf, &**empty_leaf()),
                "an empty leaf of its own at {base}"
            );
            assert!(
                (slots.end..CHUNK).all(|s| !leaf.contains(s)),
                "a record past len in the leaf at {base}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::test_util::splitmix;
    use std::collections::BTreeMap;

    fn record(seed: u64) -> Vec<u8> {
        let len = match seed % 50 {
            0 => LARGE_MIN + (seed % 7) as usize,
            1..=5 => 200 + (seed % 300) as usize,
            _ => (seed % 40) as usize,
        };
        (0..len).map(|i| (seed as usize + i) as u8).collect()
    }

    fn check(slab: &RecordSlab, model: &BTreeMap<usize, Vec<u8>>, len: usize) {
        slab.check_invariants();
        assert_eq!(slab.len(), len);
        for i in 0..len + 3 {
            assert_eq!(slab.get(i), model.get(&i).map(|v| v.as_slice()), "slot {i}");
            assert_eq!(slab.contains(i), model.contains_key(&i));
        }
        let all: Vec<(usize, Vec<u8>)> = slab.iter().map(|(i, b)| (i, b.to_vec())).collect();
        let expected: Vec<(usize, Vec<u8>)> = model.iter().map(|(&i, v)| (i, v.clone())).collect();
        assert_eq!(all, expected);
        for from in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, len / 2, len, len + 1] {
            assert!(slab
                .iter_from(from)
                .map(|(i, _)| i)
                .eq(model.range(from..).map(|(&i, _)| i)));
            assert!(slab
                .slots_from(from)
                .eq(model.range(from..).map(|(&i, _)| i)));
        }
    }

    /// One random set or remove on `slab` and its model.
    fn step(
        slab: &mut RecordSlab,
        model: &mut BTreeMap<usize, Vec<u8>>,
        len: usize,
        x: u64,
        removes: u64,
    ) {
        let i = (splitmix(x) % len as u64) as usize;
        if splitmix(x ^ 0xA5A5) % 100 < removes {
            assert_eq!(slab.remove(i), model.remove(&i).is_some());
        } else {
            let r = record(splitmix(x ^ 0xFEED));
            assert_eq!(slab.set(i, &r), model.insert(i, r).is_none());
        }
        slab.check_invariants();
    }

    /// Fill in id order (as creates do), then rewrite and remove at random
    /// while clones are taken, written to and checked on both sides.
    fn run_model(seed: u64, steps: u64, removes: u64) {
        let mut slab = RecordSlab::default();
        let mut model = BTreeMap::new();
        let len = CHUNK * 3 + 17;
        for i in 0..len {
            slab.grow_to(i + 1);
            if splitmix(seed ^ i as u64) % 11 != 3 {
                let r = record(seed ^ i as u64);
                assert!(slab.set(i, &r));
                model.insert(i, r);
            }
        }
        check(&slab, &model, len);
        let snapshot = (slab.clone(), model.clone());
        let mut fork: Option<(RecordSlab, BTreeMap<usize, Vec<u8>>)> = None;
        for n in 0..steps {
            let x = seed.wrapping_mul(0x9E37) ^ n;
            step(&mut slab, &mut model, len, x, removes);
            if n % 997 == 0 {
                // Fork: both copies are written from here on, each against
                // its own model, so copy-on-write is tested in both
                // directions.
                if let Some((f, fm)) = &fork {
                    check(f, fm, len);
                }
                fork = Some((slab.clone(), model.clone()));
            }
            if let Some((f, fm)) = &mut fork {
                step(f, fm, len, !x, removes);
            }
        }
        check(&slab, &model, len);
        check(&snapshot.0, &snapshot.1, len);
        if let Some((f, fm)) = &fork {
            check(f, fm, len);
        }
        for i in 0..len {
            assert_eq!(slab.remove(i), model.remove(&i).is_some());
        }
        slab.check_invariants();
        assert!(slab.iter().next().is_none());
    }

    #[test]
    fn matches_a_map_through_sets_removes_and_clones() {
        for seed in 0..4 {
            run_model(seed, 6_000, 25);
        }
    }

    #[test]
    fn matches_a_map_when_removes_dominate() {
        // Segments empty out, compact after removes and are dropped.
        run_model(17, 8_000, 70);
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

    /// A leaf emptied by removes is released: a slab whose records are all
    /// deleted keeps only its tree, scans skip it, and its slots fill again.
    #[test]
    fn emptied_leaves_are_released_and_refill() {
        let mut slab = RecordSlab::default();
        let len = 3 * CHUNK + 7;
        slab.grow_to(len);
        let empty_bytes = slab.heap_bytes();
        let mut model = BTreeMap::new();
        for i in 0..len {
            slab.set(i, &record(i as u64));
            model.insert(i, record(i as u64));
        }
        let full_bytes = slab.heap_bytes();
        let shared = slab.clone();
        for i in 0..len {
            if i != CHUNK + 3 {
                assert!(slab.remove(i));
                model.remove(&i);
            }
        }
        check(&slab, &model, len);
        // One leaf left: its record's segment, not the others' leaves.
        assert!(slab.heap_bytes() < full_bytes / 2);
        assert!(slab.remove(CHUNK + 3));
        model.clear();
        check(&slab, &model, len);
        assert_eq!(slab.heap_bytes(), empty_bytes);
        assert_eq!(shared.iter().count(), len);

        slab.set(2 * CHUNK + 1, b"again");
        model.insert(2 * CHUNK + 1, b"again".to_vec());
        check(&slab, &model, len);
    }
}
