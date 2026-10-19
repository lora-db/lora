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

use smallvec::SmallVec;

use crate::dict::NameDict;
use crate::{NodeId, RelationshipId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AdjEntry {
    pub type_id: u32,
    pub neighbour: NodeId,
    pub rel: RelationshipId,
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
    /// Bytes of outgoing entries, at the front of the data.
    out_len: u32,
    /// Bytes of incoming entries, at the end of the data.
    in_len: u32,
    /// Bytes of data after the header.
    cap: u32,
}

const HEADER: usize = std::mem::size_of::<Header>();
const ALIGN: usize = std::mem::align_of::<Header>();
const _: () = assert!(HEADER == 12);

/// Data capacity for at least `min` bytes, so that the allocation (header
/// included) is a multiple of 16, the allocator's size step.
#[inline]
fn capacity_for(min: usize) -> usize {
    ((HEADER + min + 15) & !15) - HEADER
}

/// Both adjacency lists of one node. Empty is a null pointer.
#[derive(Default)]
pub(super) struct Adjacency {
    ptr: Option<NonNull<Header>>,
}

// SAFETY: `Adjacency` owns its allocation exclusively, like a `Box`.
unsafe impl Send for Adjacency {}
unsafe impl Sync for Adjacency {}

impl Adjacency {
    #[inline]
    fn header(&self) -> Option<&Header> {
        // SAFETY: a non-null `ptr` points at a live, initialized header.
        self.ptr.map(|ptr| unsafe { &*ptr.as_ptr() })
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
            // and `&self` keeps the allocation alive and unchanged.
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
                let h = &*ptr.as_ptr();
                let len = h.in_len as usize;
                std::slice::from_raw_parts(Self::data(ptr).add(h.cap as usize - len), len)
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

    /// A new allocation of `cap` data bytes and empty lists.
    fn allocate(cap: usize) -> NonNull<Header> {
        let cap32 = u32::try_from(cap).expect("adjacency list under 4 GiB");
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
                cap: cap32,
            })
        };
        ptr
    }

    /// Enlarge the data area to `cap` bytes (in place where the allocator
    /// can) and move the incoming entries to its new end.
    fn grow(&mut self, cap: usize) {
        let Some(ptr) = self.ptr else {
            self.ptr = Some(Self::allocate(cap));
            return;
        };
        let cap32 = u32::try_from(cap).expect("adjacency list under 4 GiB");
        let new_layout = Self::layout(cap);
        // SAFETY: `ptr` was allocated with `layout(old_cap)`; `cap > old_cap`.
        // `realloc` keeps the first `HEADER + old_cap` bytes, so the incoming
        // entries are still at the old end, from where they are moved.
        unsafe {
            let old_cap = (*ptr.as_ptr()).cap as usize;
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
            (*new.as_ptr()).cap = cap32;
            self.ptr = Some(new);
        }
    }

    fn free(&mut self) {
        if let Some(ptr) = self.ptr.take() {
            // SAFETY: allocated with `layout(cap)`.
            unsafe {
                let cap = (*ptr.as_ptr()).cap as usize;
                alloc::dealloc(ptr.as_ptr().cast(), Self::layout(cap));
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.free();
    }

    #[inline]
    fn used(&self) -> usize {
        self.header()
            .map_or(0, |h| h.out_len as usize + h.in_len as usize)
    }

    /// Add `entry` to the outgoing list, or the incoming one.
    pub(super) fn push(&mut self, outgoing: bool, entry: AdjEntry) {
        let mut buf = [0u8; 21];
        let len = encode(entry, !outgoing, &mut buf);
        let (used, cap) = self.header().map_or((0, 0), |h| {
            (h.out_len as usize + h.in_len as usize, h.cap as usize)
        });
        if used + len > cap {
            // Growing by half: a quarter saved 2 bytes per element on the
            // benchmark graph but made bulk relationship creation 7% slower.
            self.grow(capacity_for((used + len).max(cap + cap / 2)));
        }
        let ptr = self.ptr.expect("allocated above");
        // SAFETY: `used + len <= cap`, so the entry lands in free space,
        // and `&mut self` makes the write exclusive.
        unsafe {
            let header = &mut *ptr.as_ptr();
            let at = if outgoing {
                let at = header.out_len as usize;
                header.out_len += len as u32;
                at
            } else {
                header.in_len += len as u32;
                header.cap as usize - header.in_len as usize
            };
            std::ptr::copy_nonoverlapping(buf.as_ptr(), Self::data(ptr).add(at), len);
        }
    }

    /// Remove the entry for `rel` from the outgoing list, or the incoming
    /// one. Returns whether it was present.
    pub(super) fn remove(&mut self, outgoing: bool, rel: RelationshipId) -> bool {
        let side = if outgoing {
            self.outgoing()
        } else {
            self.incoming()
        };
        let mut iter = side.iter();
        let (before, after) = loop {
            let before = iter.pos();
            let Some(entry) = iter.next() else {
                return false;
            };
            if entry.rel == rel {
                break (before, iter.pos());
            }
        };
        let Some(ptr) = self.ptr else {
            return false;
        };
        // SAFETY: the moves stay within the side's written bytes, and
        // `&mut self` makes them exclusive.
        unsafe {
            let header = &mut *ptr.as_ptr();
            let data = Self::data(ptr);
            if outgoing {
                // The entry is `before..after`: move the later ones down.
                let len = header.out_len as usize;
                std::ptr::copy(data.add(after), data.add(before), len - after);
                header.out_len -= (after - before) as u32;
            } else {
                // Offsets are within the incoming slice and the entry is
                // `after..before`. Newer entries sit below it: move them up.
                let base = data.add(header.cap as usize - header.in_len as usize);
                std::ptr::copy(base, base.add(before - after), after);
                header.in_len -= (before - after) as u32;
            }
        }
        if self.used() == 0 {
            self.free();
        }
        true
    }

    /// Heap bytes owned.
    pub(super) fn heap_bytes(&self) -> usize {
        self.header().map_or(0, |h| HEADER + h.cap as usize)
    }

    /// [`Self::heap_bytes`] split between the directions: each side's
    /// entries, plus half of the header and free space.
    pub(super) fn heap_bytes_by_side(&self) -> (usize, usize) {
        let Some(h) = self.header() else {
            return (0, 0);
        };
        let shared = HEADER + h.cap as usize - h.out_len as usize - h.in_len as usize;
        let out = h.out_len as usize + shared / 2;
        (out, self.heap_bytes() - out)
    }
}

impl Clone for Adjacency {
    /// A copy sized to the entries, without the original's free space.
    fn clone(&self) -> Self {
        let (out, inc) = (self.outgoing().bytes, self.incoming().bytes);
        if out.is_empty() && inc.is_empty() {
            return Adjacency { ptr: None };
        }
        let cap = capacity_for(out.len() + inc.len());
        let ptr = Self::allocate(cap);
        // SAFETY: the new data area holds `cap >= out + inc` bytes.
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

    fn side(adj: &Adjacency, outgoing: bool) -> AdjSide<'_> {
        if outgoing {
            adj.outgoing()
        } else {
            adj.incoming()
        }
    }

    #[test]
    fn every_width_round_trips_in_both_directions() {
        let entries = entries();
        let mut adj = Adjacency::default();
        // Interleave the directions so each push grows past the other side.
        for &e in &entries {
            adj.push(true, e);
            adj.push(false, AdjEntry { rel: !e.rel, ..e });
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
        for outgoing in [true, false] {
            let mut adj = Adjacency::default();
            let e = AdjEntry {
                type_id: 3,
                neighbour: 7,
                rel: 9,
            };
            adj.push(outgoing, e);
            assert_eq!(side(&adj, outgoing).iter().collect::<Vec<_>>(), vec![e]);
            assert!(side(&adj, !outgoing).is_empty());
            // A 12-byte header and the 4-byte entry.
            assert_eq!(adj.heap_bytes(), 16);
        }
    }

    #[test]
    fn remove_takes_exactly_one_entry_and_keeps_order() {
        for outgoing in [true, false] {
            let entries = entries();
            let mut adj = Adjacency::default();
            for &e in &entries {
                adj.push(outgoing, e);
                adj.push(
                    !outgoing,
                    AdjEntry {
                        rel: e.rel ^ 1,
                        ..e
                    },
                );
            }
            let other: Vec<_> = side(&adj, !outgoing).iter().collect();
            let mut expected = entries.clone();
            // Remove from the middle, the oldest end and the newest end.
            for index in [expected.len() / 2, 0, expected.len() - 3] {
                let gone = expected.remove(index);
                assert!(adj.remove(outgoing, gone.rel));
                assert!(!adj.remove(outgoing, gone.rel));
                assert_eq!(side(&adj, outgoing).iter().collect::<Vec<_>>(), expected);
                assert_eq!(side(&adj, outgoing).count(), expected.len());
                assert_eq!(side(&adj, !outgoing).iter().collect::<Vec<_>>(), other);
            }
            for e in expected {
                assert!(adj.remove(outgoing, e.rel));
            }
            assert!(side(&adj, outgoing).is_empty());
            for e in other {
                assert!(adj.remove(!outgoing, e.rel));
            }
            assert_eq!(adj.heap_bytes(), 0);
        }
    }

    #[test]
    fn eight_entries_of_a_16m_graph_take_one_80_byte_allocation() {
        let mut adj = Adjacency::default();
        for rel in 0..8u64 {
            adj.push(
                rel % 2 == 0,
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
