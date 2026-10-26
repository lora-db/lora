//! Per-node adjacency lists that carry what a hop needs.
//!
//! An entry is `(relationship type, neighbour, relationship id)`, so
//! expanding a node filters by type and yields the far endpoint without
//! touching a single relationship record. Entries are variable-width:
//! ids are stored in as many bytes as they need, which keeps an entry at
//! 8 bytes while ids fit in three bytes (16M nodes and relationships) and
//! at 10 bytes up to 4 billion.
//!
//! Entry layout:
//!
//! ```text
//! header: bits 0-2  neighbour width - 1   (1..=8 bytes)
//!         bits 3-5  relationship id width - 1
//!         bits 6-7  type id width code    (0 => 1, 1 => 2, 2 => 4 bytes)
//! type id, neighbour, relationship id: little-endian, widths as above
//! ```
//!
//! A node's outgoing and incoming lists share one allocation behind one
//! pointer ([`Adjacency`]): a 12-byte header, the outgoing entries
//! growing up from the front and the incoming ones growing down from the
//! end, with the free space between them.
//!
//! ```text
//! [out_len, in_len, cap | out entries →   …free…   ← in entries ]
//! ```
//!
//! Either list grows without moving the other, so a node with a million
//! incoming relationships still gains an outgoing one in O(1). Incoming
//! entries are stored mirrored (header byte last, fields in reverse
//! order), so they read from the end, oldest first, just as outgoing ones
//! read from the front. Before, each direction was a 24-byte `SmallVec`
//! that spilled to its own allocation past two entries.
//!
//! Relationship types are numbered by the graph's type dictionary
//! ([`crate::dict`]). The numbers are private to one process: snapshots
//! and the WAL keep writing names.

use std::alloc::{self, Layout};
use std::ptr::NonNull;
use std::sync::atomic::{fence, AtomicU32, Ordering};

use smallvec::SmallVec;

use crate::dict::NameDict;
use crate::{NodeId, RelationshipId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AdjEntry {
    pub type_id: u32,
    pub neighbour: NodeId,
    pub rel: RelationshipId,
}

/// One of a node's two adjacency lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    /// Relationships leaving the node, neighbour = target.
    Out,
    /// Relationships entering it, neighbour = source.
    In,
}

impl Side {
    #[cfg(test)]
    fn opposite(self) -> Side {
        match self {
            Side::Out => Side::In,
            Side::In => Side::Out,
        }
    }
}

#[inline]
fn width(value: u64) -> usize {
    ((64 - value.leading_zeros() as usize).div_ceil(8)).max(1)
}

#[inline]
fn type_width(code: u8) -> usize {
    match code {
        0 => 1,
        1 => 2,
        _ => 4,
    }
}

/// Bytes of the entry whose header byte is `h`, header included.
#[inline]
fn entry_len(h: u8) -> usize {
    1 + type_width(h >> 6) + (h & 7) as usize + 1 + ((h >> 3) & 7) as usize + 1
}

/// Little-endian read of `n` (1..=8) bytes at `pos`.
#[inline]
fn read(bytes: &[u8], pos: usize, n: usize) -> u64 {
    if let Some(word) = bytes.get(pos..pos + 8) {
        let word = u64::from_le_bytes(word.try_into().unwrap());
        word & (u64::MAX >> (64 - 8 * n))
    } else {
        let mut buf = [0u8; 8];
        buf[..n].copy_from_slice(&bytes[pos..pos + n]);
        u64::from_le_bytes(buf)
    }
}

/// `entry` encoded into `buf`, returning its length. `mirrored` writes
/// the incoming form: fields in reverse order, header byte last.
#[inline]
fn encode(entry: AdjEntry, mirrored: bool, buf: &mut [u8; 21]) -> usize {
    let nb = width(entry.neighbour);
    let rb = width(entry.rel);
    let (tcode, tb) = match entry.type_id {
        0..=0xFF => (0u8, 1),
        0x100..=0xFFFF => (1, 2),
        _ => (2, 4),
    };
    let header = (nb as u8 - 1) | ((rb as u8 - 1) << 3) | (tcode << 6);
    let len = 1 + tb + nb + rb;
    let ty = &entry.type_id.to_le_bytes()[..tb];
    let neighbour = &entry.neighbour.to_le_bytes()[..nb];
    let rel = &entry.rel.to_le_bytes()[..rb];
    if mirrored {
        buf[..rb].copy_from_slice(rel);
        buf[rb..rb + nb].copy_from_slice(neighbour);
        buf[rb + nb..rb + nb + tb].copy_from_slice(ty);
        buf[len - 1] = header;
    } else {
        buf[0] = header;
        buf[1..1 + tb].copy_from_slice(ty);
        buf[1 + tb..1 + tb + nb].copy_from_slice(neighbour);
        buf[1 + tb + nb..len].copy_from_slice(rel);
    }
    len
}

#[repr(C)]
struct Header {
    /// Bytes of outgoing entries, at the front of the data. Written only
    /// while the allocation is unshared.
    out_len: u32,
    /// Bytes of incoming entries, at the end of the data. Written only
    /// while the allocation is unshared.
    in_len: u32,
    /// Data capacity and reference count in one word: see [`CapRc`].
    cap_rc: AtomicU32,
}

const HEADER: usize = std::mem::size_of::<Header>();
const ALIGN: usize = std::mem::align_of::<Header>();
const _: () = assert!(HEADER == 12);

/// The largest data capacity: lengths are `u32`, and the allocation stays a
/// multiple of 16.
const MAX_CAP: usize = (u32::MAX as usize & !15) - HEADER;

/// Data capacity for at least `min` bytes, so that the allocation (header
/// included) is a multiple of 16, the allocator's size step.
#[inline]
const fn capacity_for(min: usize) -> usize {
    ((HEADER + min + 15) & !15) - HEADER
}
const _: () = assert!(capacity_for(MAX_CAP) == MAX_CAP && MAX_CAP <= u32::MAX as usize);

/// Every capacity is [`capacity_for`] something, so `HEADER + cap` is a
/// multiple of 16 and `cap % 16` is always this. The low four bits of the
/// capacity carry no information, and hold the reference count instead.
const CAP_LOW: usize = (16 - HEADER % 16) % 16;
const _: () = assert!(capacity_for(1) % 16 == CAP_LOW && MAX_CAP % 16 == CAP_LOW);

/// Count bits of [`Header::cap_rc`].
const RC_MASK: u32 = 15;
/// The most handles one allocation can have. Cloning a list held this
/// many times copies it instead, as every clone did before lists were
/// shared.
const RC_MAX: u32 = RC_MASK;

/// Decoding of [`Header::cap_rc`]: `cap - CAP_LOW` (a multiple of 16)
/// with the count of [`Adjacency`] handles (1..=[`RC_MAX`]) in the low
/// bits. Packing the count there keeps the header at 12 bytes: a separate
/// counter would make it 16, and cost every node with a list ~4 bytes.
struct CapRc;

impl CapRc {
    #[inline]
    const fn pack(cap: usize, rc: u32) -> u32 {
        (cap - CAP_LOW) as u32 | rc
    }

    #[inline]
    const fn cap(word: u32) -> usize {
        (word & !RC_MASK) as usize + CAP_LOW
    }

    #[inline]
    const fn rc(word: u32) -> u32 {
        word & RC_MASK
    }
}

/// Both adjacency lists of one node. Empty is a null pointer.
///
/// The allocation is shared, like an `Arc`: cloning bumps the count in
/// its header, and a change to a shared allocation first copies it. A
/// staged write copies a whole leaf of [`super::chunked_vec::ChunkedVec`]
/// (512 nodes), which used to mean 512 list copies (and every edge of a
/// hub in that leaf) to change one node's list; now it is 512 count
/// bumps, and only the list that changes is copied.
#[derive(Default)]
pub(super) struct Adjacency {
    ptr: Option<NonNull<Header>>,
}

// SAFETY: an allocation is changed only through the one handle that holds
// it (`make_unique` checks the count with acquire ordering, as
// `Arc::get_mut` does), the count is atomic, and the entries are plain
// bytes, so handles can move and be shared between threads like `Arc<[u8]>`.
unsafe impl Send for Adjacency {}
unsafe impl Sync for Adjacency {}

impl Adjacency {
    #[inline]
    fn header(&self) -> Option<&Header> {
        // SAFETY: a non-null `ptr` points at a live, initialized header,
        // kept alive by this handle's share of the count.
        self.ptr.map(|ptr| unsafe { &*ptr.as_ptr() })
    }

    /// Data capacity of the allocation at `ptr`.
    #[inline]
    fn cap_of(ptr: NonNull<Header>) -> usize {
        // SAFETY: as for `header`. Relaxed suffices: the capacity bits
        // only change while the allocation is unshared.
        CapRc::cap(unsafe { (*ptr.as_ptr()).cap_rc.load(Ordering::Relaxed) })
    }

    /// Start of the data area. Only the outgoing bytes at its front and
    /// the incoming bytes at its end are initialized; nothing reads the
    /// free space between them.
    #[inline]
    fn data(ptr: NonNull<Header>) -> *mut u8 {
        // SAFETY: the data area follows the header in the same allocation.
        unsafe { ptr.as_ptr().cast::<u8>().add(HEADER) }
    }

    #[inline]
    pub(super) fn outgoing(&self) -> AdjSide<'_> {
        let bytes = match self.ptr {
            // SAFETY: the first `out_len` data bytes are written entries,
            // and `&self` keeps the allocation alive and unchanged (it is
            // changed only through a sole handle, under `&mut`).
            Some(ptr) => unsafe {
                let len = (*ptr.as_ptr()).out_len as usize;
                std::slice::from_raw_parts(Self::data(ptr), len)
            },
            None => &[],
        };
        AdjSide {
            bytes,
            mirrored: false,
        }
    }

    #[inline]
    pub(super) fn incoming(&self) -> AdjSide<'_> {
        let bytes = match self.ptr {
            // SAFETY: the last `in_len` of the `cap` data bytes are written
            // entries.
            Some(ptr) => unsafe {
                let len = (*ptr.as_ptr()).in_len as usize;
                let cap = Self::cap_of(ptr);
                std::slice::from_raw_parts(Self::data(ptr).add(cap - len), len)
            },
            None => &[],
        };
        AdjSide {
            bytes,
            mirrored: true,
        }
    }

    #[inline]
    fn layout(cap: usize) -> Layout {
        Layout::from_size_align(HEADER + cap, ALIGN).expect("adjacency list fits the address space")
    }

    /// A new, unshared allocation of `cap` data bytes and empty lists.
    fn allocate(cap: usize) -> NonNull<Header> {
        debug_assert_eq!(cap % 16, CAP_LOW);
        assert!(cap <= MAX_CAP, "adjacency list under 4 GiB");
        let layout = Self::layout(cap);
        // SAFETY: the layout has a non-zero size (it includes the header).
        let raw = unsafe { alloc::alloc(layout) };
        let Some(ptr) = NonNull::new(raw.cast::<Header>()) else {
            alloc::handle_alloc_error(layout)
        };
        // SAFETY: `ptr` is valid for writes of a `Header` and aligned for it.
        unsafe {
            ptr.as_ptr().write(Header {
                out_len: 0,
                in_len: 0,
                cap_rc: AtomicU32::new(CapRc::pack(cap, 1)),
            })
        };
        ptr
    }

    /// An unshared copy of both lists in a new allocation of
    /// `capacity_for(min)` data bytes (at least the bytes in use).
    fn copy_with_capacity(&self, min: usize) -> Adjacency {
        let (out, inc) = (self.outgoing().bytes, self.incoming().bytes);
        let cap = capacity_for(min.max(out.len() + inc.len()));
        let ptr = Self::allocate(cap);
        // SAFETY: the new data area holds `cap >= out + inc` bytes, and no
        // other handle sees it yet.
        unsafe {
            let data = Self::data(ptr);
            std::ptr::copy_nonoverlapping(out.as_ptr(), data, out.len());
            std::ptr::copy_nonoverlapping(inc.as_ptr(), data.add(cap - inc.len()), inc.len());
            let header = &mut *ptr.as_ptr();
            header.out_len = out.len() as u32;
            header.in_len = inc.len() as u32;
        }
        Adjacency { ptr: Some(ptr) }
    }

    /// Whether this is the only handle on a non-empty allocation, so it
    /// may change it in place.
    #[inline]
    fn is_unique(&self) -> bool {
        // SAFETY: as for `header`. Acquire pairs with the release in
        // `free`: whatever another handle did before letting go happens
        // before our writes.
        self.ptr.is_some_and(|ptr| unsafe {
            CapRc::rc((*ptr.as_ptr()).cap_rc.load(Ordering::Acquire)) == 1
        })
    }

    /// Make this handle the sole owner of its allocation, copying it
    /// (with room for `min` data bytes) if another handle shares it.
    #[inline]
    fn make_unique(&mut self, min: usize) {
        if self.ptr.is_some() && !self.is_unique() {
            *self = self.copy_with_capacity(min);
        }
    }

    /// Enlarge the data area to `cap` bytes (in place where the allocator
    /// can) and move the incoming entries to its new end. The allocation
    /// must be unshared.
    fn grow(&mut self, cap: usize) {
        let Some(ptr) = self.ptr else {
            self.ptr = Some(Self::allocate(cap));
            return;
        };
        debug_assert!(self.is_unique());
        debug_assert_eq!(cap % 16, CAP_LOW);
        let new_layout = Self::layout(cap);
        // SAFETY: `ptr` was allocated with `layout(old_cap)`; `cap > old_cap`.
        // `realloc` keeps the first `HEADER + old_cap` bytes, so the incoming
        // entries are still at the old end, from where they are moved. The
        // allocation is unshared, so nothing else reads it meanwhile.
        unsafe {
            let old_cap = Self::cap_of(ptr);
            let in_len = (*ptr.as_ptr()).in_len as usize;
            let raw = alloc::realloc(
                ptr.as_ptr().cast(),
                Self::layout(old_cap),
                new_layout.size(),
            );
            let Some(new) = NonNull::new(raw.cast::<Header>()) else {
                alloc::handle_alloc_error(new_layout)
            };
            let data = Self::data(new);
            std::ptr::copy(data.add(old_cap - in_len), data.add(cap - in_len), in_len);
            (*new.as_ptr())
                .cap_rc
                .store(CapRc::pack(cap, 1), Ordering::Relaxed);
            self.ptr = Some(new);
        }
    }

    /// Let go of the allocation: drop this handle's share, and free it if
    /// that was the last.
    fn free(&mut self) {
        if let Some(ptr) = self.ptr.take() {
            // SAFETY: this handle held a share, so the allocation is live
            // until the decrement; the last handle frees it with the layout
            // it was allocated with. Release/acquire as in `Arc::drop`, so
            // every other handle's use happens before the free.
            unsafe {
                let word = (*ptr.as_ptr()).cap_rc.fetch_sub(1, Ordering::Release);
                if CapRc::rc(word) == 1 {
                    fence(Ordering::Acquire);
                    alloc::dealloc(ptr.as_ptr().cast(), Self::layout(CapRc::cap(word)));
                }
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.free();
    }

    /// Whether both lists are empty.
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.ptr.is_none()
    }

    /// One of the two lists.
    #[inline]
    pub(super) fn side(&self, side: Side) -> AdjSide<'_> {
        match side {
            Side::Out => self.outgoing(),
            Side::In => self.incoming(),
        }
    }

    /// Add `entry` to the list on `side`.
    pub(super) fn push(&mut self, side: Side, entry: AdjEntry) {
        let mut buf = [0u8; 21];
        let len = encode(entry, side == Side::In, &mut buf);
        let (used, cap) = self.ptr.map_or((0, 0), |ptr| {
            // SAFETY: as for `header`.
            let h = unsafe { &*ptr.as_ptr() };
            (h.out_len as usize + h.in_len as usize, Self::cap_of(ptr))
        });
        let want = if used + len > cap {
            assert!(used + len <= MAX_CAP, "adjacency list over 4 GiB");
            // Growing by half: a quarter saved 2 bytes per element on the
            // benchmark graph but made bulk relationship creation 7% slower.
            (used + len).max(cap + cap / 2).min(MAX_CAP)
        } else {
            cap
        };
        // A shared list is copied once, straight into the size it needs.
        self.make_unique(want);
        if used + len > self.ptr.map_or(0, Self::cap_of) {
            self.grow(capacity_for(want));
        }
        let ptr = self.ptr.expect("allocated above");
        // SAFETY: `used + len <= cap`, so the entry lands in free space,
        // and the allocation is unshared and `&mut self` makes the write
        // exclusive.
        unsafe {
            let cap = Self::cap_of(ptr);
            let header = &mut *ptr.as_ptr();
            let at = if side == Side::Out {
                let at = header.out_len as usize;
                header.out_len += len as u32;
                at
            } else {
                header.in_len += len as u32;
                cap - header.in_len as usize
            };
            std::ptr::copy_nonoverlapping(buf.as_ptr(), Self::data(ptr).add(at), len);
        }
    }

    /// Remove the entry for `rel` from the list on `side`. Returns whether
    /// it was present.
    pub(super) fn remove(&mut self, side: Side, rel: RelationshipId) -> bool {
        let mut iter = self.side(side).iter();
        let (before, after) = loop {
            let before = iter.pos();
            let Some(entry) = iter.next() else {
                return false;
            };
            if entry.rel == rel {
                break (before, iter.pos());
            }
        };
        // Offsets are relative to the side's bytes, which a copy keeps.
        self.make_unique(0);
        let Some(ptr) = self.ptr else {
            return false;
        };
        // SAFETY: the moves stay within the side's written bytes, and the
        // allocation is unshared and `&mut self` makes them exclusive.
        unsafe {
            let cap = Self::cap_of(ptr);
            let header = &mut *ptr.as_ptr();
            let data = Self::data(ptr);
            if side == Side::Out {
                // The entry is `before..after`: move the later ones down.
                let len = header.out_len as usize;
                std::ptr::copy(data.add(after), data.add(before), len - after);
                header.out_len -= (after - before) as u32;
            } else {
                // Offsets are within the incoming slice and the entry is
                // `after..before`. Newer entries sit below it: move them up.
                let base = data.add(cap - header.in_len as usize);
                std::ptr::copy(base, base.add(before - after), after);
                header.in_len -= (before - after) as u32;
            }
        }
        if self.outgoing().is_empty() && self.incoming().is_empty() {
            self.free();
        }
        true
    }

    /// Heap bytes of the allocation, counted in full by every handle that
    /// shares it.
    pub(super) fn heap_bytes(&self) -> usize {
        self.ptr.map_or(0, |ptr| HEADER + Self::cap_of(ptr))
    }

    /// [`Self::heap_bytes`] split between the directions: each side's
    /// entries, plus half of the header and free space.
    pub(super) fn heap_bytes_by_side(&self) -> (usize, usize) {
        let Some(h) = self.header() else {
            return (0, 0);
        };
        let total = self.heap_bytes();
        let shared = total - h.out_len as usize - h.in_len as usize;
        let out = h.out_len as usize + shared / 2;
        (out, total - out)
    }
}

impl Clone for Adjacency {
    /// Another handle on the same allocation. A list already held
    /// [`RC_MAX`] times is copied instead, sized to its entries.
    fn clone(&self) -> Self {
        let Some(ptr) = self.ptr else {
            return Adjacency { ptr: None };
        };
        // SAFETY: as for `header`.
        let count = unsafe { &(*ptr.as_ptr()).cap_rc };
        // Relaxed as in `Arc::clone`: the new handle comes from an existing
        // one, which keeps the allocation alive meanwhile.
        let mut word = count.load(Ordering::Relaxed);
        loop {
            if CapRc::rc(word) == RC_MAX {
                return self.copy_with_capacity(0);
            }
            match count.compare_exchange_weak(word, word + 1, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Adjacency { ptr: Some(ptr) },
                Err(now) => word = now,
            }
        }
    }
}

impl Drop for Adjacency {
    fn drop(&mut self) {
        self.free();
    }
}

impl std::fmt::Debug for Adjacency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adjacency")
            .field("outgoing", &self.outgoing())
            .field("incoming", &self.incoming())
            .finish()
    }
}

/// One direction of a node's adjacency.
#[derive(Clone, Copy)]
pub(super) struct AdjSide<'a> {
    bytes: &'a [u8],
    mirrored: bool,
}

impl<'a> AdjSide<'a> {
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    #[inline]
    pub(super) fn iter(&self) -> AdjIter<'a> {
        AdjIter {
            bytes: self.bytes,
            pos: if self.mirrored { self.bytes.len() } else { 0 },
            mirrored: self.mirrored,
        }
    }

    /// Number of entries. O(entries): the list stores no count, and only
    /// the header byte of each entry is read.
    pub(super) fn count(&self) -> usize {
        let bytes = self.bytes;
        let mut n = 0;
        if self.mirrored {
            let mut pos = bytes.len();
            while pos > 0 {
                pos -= entry_len(bytes[pos - 1]);
                n += 1;
            }
        } else {
            let mut pos = 0;
            while pos < bytes.len() {
                pos += entry_len(bytes[pos]);
                n += 1;
            }
        }
        n
    }
}

impl std::fmt::Debug for AdjSide<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// Entries oldest first: forward through an outgoing list, backward
/// through a mirrored incoming one.
pub(super) struct AdjIter<'a> {
    bytes: &'a [u8],
    pos: usize,
    mirrored: bool,
}

impl AdjIter<'_> {
    /// Offset of the next entry's edge: its start going forward, its end
    /// going backward.
    #[inline]
    fn pos(&self) -> usize {
        self.pos
    }
}

impl Iterator for AdjIter<'_> {
    type Item = AdjEntry;

    #[inline]
    fn next(&mut self) -> Option<AdjEntry> {
        if self.mirrored {
            if self.pos == 0 {
                return None;
            }
            let h = self.bytes[self.pos - 1];
            let tb = type_width(h >> 6);
            let nb = (h & 7) as usize + 1;
            let rb = ((h >> 3) & 7) as usize + 1;
            let ty = self.pos - 1 - tb;
            let neighbour = ty - nb;
            let rel = neighbour - rb;
            self.pos = rel;
            return Some(AdjEntry {
                type_id: read(self.bytes, ty, tb) as u32,
                neighbour: read(self.bytes, neighbour, nb),
                rel: read(self.bytes, rel, rb),
            });
        }
        let h = *self.bytes.get(self.pos)?;
        let tb = type_width(h >> 6);
        let nb = (h & 7) as usize + 1;
        let rb = ((h >> 3) & 7) as usize + 1;
        let p = self.pos + 1;
        let entry = AdjEntry {
            type_id: read(self.bytes, p, tb) as u32,
            neighbour: read(self.bytes, p + tb, nb),
            rel: read(self.bytes, p + tb + nb, rb),
        };
        self.pos = p + tb + nb + rb;
        Some(entry)
    }
}

/// Which relationship types a traversal accepts.
pub(super) enum TypeFilter {
    Any,
    One(u32),
    Many(SmallVec<u32, 4>),
    /// A type was asked for that no relationship has ever had.
    Nothing,
}

impl TypeFilter {
    /// Resolve a traversal's type list once, before walking entries.
    #[inline]
    pub(super) fn resolve(dict: &NameDict, types: &[String]) -> Self {
        match types {
            [] => TypeFilter::Any,
            [single] => match dict.id_of(single) {
                Some(id) => TypeFilter::One(id),
                None => TypeFilter::Nothing,
            },
            many => {
                let ids: SmallVec<u32, 4> = many.iter().filter_map(|t| dict.id_of(t)).collect();
                match ids.as_slice() {
                    [] => TypeFilter::Nothing,
                    [one] => TypeFilter::One(*one),
                    _ => TypeFilter::Many(ids),
                }
            }
        }
    }

    #[inline]
    pub(super) fn matches(&self, type_id: u32) -> bool {
        match self {
            TypeFilter::Any => true,
            TypeFilter::One(id) => *id == type_id,
            TypeFilter::Many(ids) => ids.contains(&type_id),
            TypeFilter::Nothing => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<AdjEntry> {
        let ids = [
            0u64,
            1,
            255,
            256,
            65_535,
            65_536,
            16_777_215,
            16_777_216,
            u32::MAX as u64,
            u32::MAX as u64 + 1,
            (1 << 48) - 1,
            1 << 56,
            u64::MAX,
        ];
        let types = [0u32, 1, 255, 256, 65_535, 65_536, u32::MAX];
        let mut out = Vec::new();
        for (i, &neighbour) in ids.iter().enumerate() {
            for (j, &type_id) in types.iter().enumerate() {
                let rel = ids[(i * 7 + j * 3) % ids.len()] ^ (out.len() as u64);
                out.push(AdjEntry {
                    type_id,
                    neighbour,
                    rel,
                });
            }
        }
        out
    }

    #[test]
    fn every_width_round_trips_in_both_directions() {
        let entries = entries();
        let mut adj = Adjacency::default();
        // Interleave the directions so each push grows past the other side.
        for &e in &entries {
            adj.push(Side::Out, e);
            adj.push(Side::In, AdjEntry { rel: !e.rel, ..e });
        }
        let incoming: Vec<_> = entries
            .iter()
            .map(|&e| AdjEntry { rel: !e.rel, ..e })
            .collect();
        assert_eq!(adj.outgoing().iter().collect::<Vec<_>>(), entries);
        assert_eq!(adj.incoming().iter().collect::<Vec<_>>(), incoming);
        assert_eq!(adj.outgoing().count(), entries.len());
        assert_eq!(adj.incoming().count(), entries.len());
        let copy = adj.clone();
        assert_eq!(copy.outgoing().iter().collect::<Vec<_>>(), entries);
        assert_eq!(copy.incoming().iter().collect::<Vec<_>>(), incoming);
        assert!(copy.heap_bytes() <= adj.heap_bytes());
    }

    #[test]
    fn a_single_short_entry_reads_without_eight_bytes_behind_it() {
        for dir in [Side::Out, Side::In] {
            let mut adj = Adjacency::default();
            let e = AdjEntry {
                type_id: 3,
                neighbour: 7,
                rel: 9,
            };
            adj.push(dir, e);
            assert_eq!(adj.side(dir).iter().collect::<Vec<_>>(), vec![e]);
            assert!(adj.side(dir.opposite()).is_empty());
            // A 12-byte header and the 4-byte entry.
            assert_eq!(adj.heap_bytes(), 16);
        }
    }

    #[test]
    fn remove_takes_exactly_one_entry_and_keeps_order() {
        for dir in [Side::Out, Side::In] {
            let entries = entries();
            let mut adj = Adjacency::default();
            for &e in &entries {
                adj.push(dir, e);
                adj.push(
                    dir.opposite(),
                    AdjEntry {
                        rel: e.rel ^ 1,
                        ..e
                    },
                );
            }
            let other: Vec<_> = adj.side(dir.opposite()).iter().collect();
            let mut expected = entries.clone();
            // Remove from the middle, the oldest end and the newest end.
            for index in [expected.len() / 2, 0, expected.len() - 3] {
                let gone = expected.remove(index);
                assert!(adj.remove(dir, gone.rel));
                assert!(!adj.remove(dir, gone.rel));
                assert_eq!(adj.side(dir).iter().collect::<Vec<_>>(), expected);
                assert_eq!(adj.side(dir).count(), expected.len());
                assert_eq!(adj.side(dir.opposite()).iter().collect::<Vec<_>>(), other);
            }
            for e in expected {
                assert!(adj.remove(dir, e.rel));
            }
            assert!(adj.side(dir).is_empty());
            for e in other {
                assert!(adj.remove(dir.opposite(), e.rel));
            }
            assert_eq!(adj.heap_bytes(), 0);
        }
    }

    #[test]
    fn eight_entries_of_a_16m_graph_take_one_80_byte_allocation() {
        let mut adj = Adjacency::default();
        for rel in 0..8u64 {
            adj.push(
                if rel % 2 == 0 { Side::Out } else { Side::In },
                AdjEntry {
                    type_id: 200,
                    neighbour: 15_999_999,
                    rel: 16_000_000 + rel,
                },
            );
        }
        assert_eq!(adj.clone().heap_bytes(), 80);
        assert_eq!(std::mem::size_of::<Adjacency>(), 8);
    }

    fn sample(n: u64) -> Vec<AdjEntry> {
        (0..n)
            .map(|i| AdjEntry {
                type_id: (i % 3) as u32,
                neighbour: 1000 + i,
                rel: i,
            })
            .collect()
    }

    fn filled(n: u64) -> Adjacency {
        let mut adj = Adjacency::default();
        for e in sample(n) {
            adj.push(if e.rel % 2 == 0 { Side::Out } else { Side::In }, e);
        }
        adj
    }

    fn lists(adj: &Adjacency) -> (Vec<AdjEntry>, Vec<AdjEntry>) {
        (
            adj.outgoing().iter().collect(),
            adj.incoming().iter().collect(),
        )
    }

    #[test]
    fn a_clone_shares_the_allocation_until_either_side_changes() {
        let mut a = filled(40);
        let before = lists(&a);
        let mut b = a.clone();
        assert_eq!(a.ptr, b.ptr, "a clone is a second handle, not a copy");
        let extra = AdjEntry {
            type_id: 9,
            neighbour: 5,
            rel: 500,
        };
        b.push(Side::Out, extra);
        assert_ne!(a.ptr, b.ptr, "pushing to a shared list copies it");
        assert_eq!(lists(&a), before);
        assert_eq!(b.outgoing().iter().last(), Some(extra));
        assert_eq!(b.incoming().iter().collect::<Vec<_>>(), before.1);
        // Both are sole owners now: further changes stay in place.
        let (pa, pb) = (a.ptr, b.ptr);
        assert!(a.remove(Side::In, 1));
        assert!(b.remove(Side::Out, 500));
        assert_eq!((a.ptr, b.ptr), (pa, pb));
        assert_eq!(lists(&b), before);
        assert_ne!(lists(&a), before);
    }

    #[test]
    fn removing_from_a_shared_list_leaves_the_other_handle_intact() {
        for side in [Side::Out, Side::In] {
            let a = filled(10);
            let before = lists(&a);
            let mut b = a.clone();
            let rel = if side == Side::Out { 4 } else { 5 };
            assert!(!b.remove(side, 999), "a miss copies nothing");
            assert_eq!(a.ptr, b.ptr);
            assert!(b.remove(side, rel));
            assert_ne!(a.ptr, b.ptr);
            assert_eq!(lists(&a), before);
            assert!(b.side(side).iter().all(|e| e.rel != rel));
            assert_eq!(b.side(side).count(), 4);
        }
        // Emptying a shared list frees only this handle's share.
        let mut one = Adjacency::default();
        one.push(Side::In, sample(1)[0]);
        let mut other = one.clone();
        assert!(other.remove(Side::In, 0));
        assert!(other.is_empty() && other.heap_bytes() == 0);
        assert_eq!(one.incoming().count(), 1);
    }

    #[test]
    fn clearing_and_dropping_handles_frees_once() {
        let a = filled(30);
        let mut handles: Vec<Adjacency> = (0..5).map(|_| a.clone()).collect();
        handles[2].clear();
        assert!(handles[2].is_empty());
        drop(a);
        let survivor = handles.pop().unwrap();
        drop(handles);
        assert_eq!(
            survivor.outgoing().count() + survivor.incoming().count(),
            30
        );
    }

    #[test]
    fn a_list_held_rc_max_times_is_copied_on_clone() {
        let a = filled(12);
        let held: Vec<Adjacency> = (1..RC_MAX).map(|_| a.clone()).collect();
        assert!(held.iter().all(|h| h.ptr == a.ptr));
        let copy = a.clone();
        assert_ne!(copy.ptr, a.ptr, "the count is saturated: copy instead");
        assert_eq!(lists(&copy), lists(&a));
        // The copy is sized to its entries, like every clone used to be.
        assert!(copy.heap_bytes() <= a.heap_bytes());
        drop(held);
        let shared = a.clone();
        assert_eq!(shared.ptr, a.ptr, "dropping handles frees count again");
    }

    #[test]
    fn growing_a_list_keeps_its_count_bits() {
        let mut a = Adjacency::default();
        for e in sample(200) {
            a.push(Side::In, e);
            let ptr = a.ptr.unwrap();
            // SAFETY: test-only read of the live header.
            let word = unsafe { (*ptr.as_ptr()).cap_rc.load(Ordering::Relaxed) };
            assert_eq!(CapRc::rc(word), 1);
            assert_eq!(CapRc::cap(word) % 16, CAP_LOW);
            assert_eq!(a.heap_bytes() % 16, 0);
        }
        assert_eq!(a.incoming().iter().collect::<Vec<_>>(), sample(200));
    }

    #[test]
    fn handles_cross_threads() {
        let a = filled(64);
        let expected = lists(&a);
        std::thread::scope(|scope| {
            for t in 0..4u64 {
                let mut mine = a.clone();
                let expected = &expected;
                scope.spawn(move || {
                    for i in 0..8 {
                        let copy = mine.clone();
                        assert_eq!(lists(&copy), lists(&mine));
                        if i <= 4 {
                            assert_eq!(&lists(&copy), expected);
                        }
                        drop(copy);
                        if i == 4 {
                            mine.push(
                                Side::Out,
                                AdjEntry {
                                    type_id: 1,
                                    neighbour: t,
                                    rel: 10_000 + t,
                                },
                            );
                        }
                    }
                    assert_eq!(mine.outgoing().count(), expected.0.len() + 1);
                });
            }
        });
        assert_eq!(lists(&a), expected);
    }

    #[test]
    fn a_type_filter_resolves_names_once() {
        let mut dict = NameDict::default();
        for i in 0..20 {
            dict.id_or_insert_str(&format!("T{i}"));
        }
        let filter = |types: &[&str]| {
            let types: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            TypeFilter::resolve(&dict, &types)
        };
        assert!(filter(&[]).matches(19));
        let one = filter(&["T4"]);
        assert!(one.matches(4) && !one.matches(5));
        let many = filter(&["T4", "nope", "T9"]);
        assert!(many.matches(4) && many.matches(9) && !many.matches(5));
        assert!(!filter(&["nope"]).matches(0));
    }
}
