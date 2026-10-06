//! Per-node adjacency lists that carry what a hop needs.
//!
//! An entry is `(relationship type, neighbour, relationship id)`, so
//! expanding a node filters by type and yields the far endpoint without
//! touching a single relationship record. The list is a byte string of
//! variable-width entries: ids are stored in as many bytes as they need,
//! which keeps an entry at 8 bytes while ids fit in three bytes (16M
//! nodes and relationships) and at 10 bytes up to 4 billion.
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
//! Relationship types are numbered by the graph's [`TypeDict`]. The ids
//! are private to one process: snapshots and the WAL keep writing names.

use std::collections::HashMap;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::{Name, NodeId, RelationshipId};

/// Bytes kept inline before a list spills to the heap: two entries of a
/// graph with up to 16M nodes and relationships.
const INLINE_BYTES: usize = 16;

/// Dictionaries up to this size are searched linearly: comparing a
/// handful of short names beats hashing one.
const LINEAR_LOOKUP_MAX: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AdjEntry {
    pub type_id: u32,
    pub neighbour: NodeId,
    pub rel: RelationshipId,
}

#[derive(Clone, Default)]
pub(super) struct AdjList {
    bytes: SmallVec<u8, INLINE_BYTES>,
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

impl AdjList {
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.bytes.clear();
    }

    pub(super) fn push(&mut self, entry: AdjEntry) {
        let nb = width(entry.neighbour);
        let rb = width(entry.rel);
        let (tcode, tb) = match entry.type_id {
            0..=0xFF => (0u8, 1),
            0x100..=0xFFFF => (1, 2),
            _ => (2, 4),
        };
        let header = (nb as u8 - 1) | ((rb as u8 - 1) << 3) | (tcode << 6);
        self.bytes.reserve(1 + tb + nb + rb);
        self.bytes.push(header);
        self.bytes
            .extend_from_slice(&entry.type_id.to_le_bytes()[..tb]);
        self.bytes
            .extend_from_slice(&entry.neighbour.to_le_bytes()[..nb]);
        self.bytes.extend_from_slice(&entry.rel.to_le_bytes()[..rb]);
    }

    /// Remove the entry for `rel`. Returns whether it was present.
    pub(super) fn remove(&mut self, rel: RelationshipId) -> bool {
        let mut iter = self.iter();
        loop {
            let start = iter.pos;
            let Some(entry) = iter.next() else {
                return false;
            };
            if entry.rel == rel {
                let end = iter.pos;
                self.bytes.drain(start..end);
                return true;
            }
        }
    }

    #[inline]
    pub(super) fn iter(&self) -> AdjIter<'_> {
        AdjIter {
            bytes: &self.bytes,
            pos: 0,
        }
    }

    /// Number of entries. O(entries): the list stores no count, and only
    /// the header byte of each entry is read.
    pub(super) fn count(&self) -> usize {
        let bytes = self.bytes.as_slice();
        let mut pos = 0;
        let mut n = 0;
        while pos < bytes.len() {
            let h = bytes[pos];
            pos += 1 + type_width(h >> 6) + (h & 7) as usize + 1 + ((h >> 3) & 7) as usize + 1;
            n += 1;
        }
        n
    }

    /// Heap bytes owned beyond the inline storage.
    pub(super) fn heap_bytes(&self) -> usize {
        if self.bytes.spilled() {
            self.bytes.capacity()
        } else {
            0
        }
    }
}

impl std::fmt::Debug for AdjList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

pub(super) struct AdjIter<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Iterator for AdjIter<'_> {
    type Item = AdjEntry;

    #[inline]
    fn next(&mut self) -> Option<AdjEntry> {
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

/// The graph's relationship types, numbered in first-use order. Ids are
/// never reused: a type whose last relationship was deleted keeps its
/// number.
#[derive(Clone, Default)]
pub(super) struct TypeDict {
    inner: Arc<TypeDictInner>,
}

#[derive(Clone, Default)]
struct TypeDictInner {
    names: Vec<Name>,
    ids: HashMap<Name, u32>,
}

impl TypeDict {
    #[inline]
    pub(super) fn id_of(&self, name: &str) -> Option<u32> {
        let inner = &*self.inner;
        if inner.names.len() <= LINEAR_LOOKUP_MAX {
            inner
                .names
                .iter()
                .position(|n| n.as_str() == name)
                .map(|i| i as u32)
        } else {
            inner.ids.get(name).copied()
        }
    }

    /// The id for `name`, assigning the next one on first use.
    pub(super) fn id_or_insert(&mut self, name: &Name) -> u32 {
        if let Some(id) = self.id_of(name) {
            return id;
        }
        let inner = Arc::make_mut(&mut self.inner);
        let id = inner.names.len() as u32;
        inner.names.push(name.clone());
        inner.ids.insert(name.clone(), id);
        id
    }

    /// Resolve a traversal's type list once, before walking entries.
    #[inline]
    pub(super) fn filter(&self, types: &[String]) -> TypeFilter {
        match types {
            [] => TypeFilter::Any,
            [single] => match self.id_of(single) {
                Some(id) => TypeFilter::One(id),
                None => TypeFilter::Nothing,
            },
            many => {
                let ids: SmallVec<u32, 4> = many.iter().filter_map(|t| self.id_of(t)).collect();
                match ids.as_slice() {
                    [] => TypeFilter::Nothing,
                    [one] => TypeFilter::One(*one),
                    _ => TypeFilter::Many(ids),
                }
            }
        }
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
    fn every_width_round_trips() {
        let entries = entries();
        let mut list = AdjList::new();
        for &e in &entries {
            list.push(e);
        }
        assert_eq!(list.iter().collect::<Vec<_>>(), entries);
        assert_eq!(list.count(), entries.len());
    }

    #[test]
    fn a_single_short_entry_reads_without_eight_bytes_behind_it() {
        let mut list = AdjList::new();
        let e = AdjEntry {
            type_id: 3,
            neighbour: 7,
            rel: 9,
        };
        list.push(e);
        assert_eq!(list.iter().collect::<Vec<_>>(), vec![e]);
        assert_eq!(list.heap_bytes(), 0);
    }

    #[test]
    fn remove_takes_exactly_one_entry_and_keeps_order() {
        let entries = entries();
        let mut list = AdjList::new();
        for &e in &entries {
            list.push(e);
        }
        let mut expected = entries.clone();
        // Remove from the middle, the front and the back.
        for index in [expected.len() / 2, 0, expected.len() - 3] {
            let gone = expected.remove(index);
            assert!(list.remove(gone.rel));
            assert!(!list.remove(gone.rel));
            assert_eq!(list.iter().collect::<Vec<_>>(), expected);
            assert_eq!(list.count(), expected.len());
        }
        for e in expected {
            assert!(list.remove(e.rel));
        }
        assert!(list.is_empty());
    }

    #[test]
    fn two_entries_of_a_16m_graph_stay_inline() {
        let mut list = AdjList::new();
        for rel in [16_000_000u64, 16_000_001] {
            list.push(AdjEntry {
                type_id: 200,
                neighbour: 15_999_999,
                rel,
            });
        }
        assert_eq!(list.heap_bytes(), 0);
        assert_eq!(std::mem::size_of::<AdjList>(), 24);
    }

    #[test]
    fn dictionary_numbers_types_once_and_filters() {
        let mut dict = TypeDict::default();
        let names: Vec<Name> = (0..20).map(|i| Name::new(&format!("T{i}"))).collect();
        for (i, name) in names.iter().enumerate() {
            assert_eq!(dict.id_or_insert(name), i as u32);
            // Both the linear and the hashed lookup see every earlier name.
            for (j, earlier) in names[..=i].iter().enumerate() {
                assert_eq!(dict.id_of(earlier), Some(j as u32));
            }
        }
        assert_eq!(dict.id_or_insert(&names[3]), 3);
        assert_eq!(dict.id_of("missing"), None);

        assert!(dict.filter(&[]).matches(19));
        let one = dict.filter(&["T4".to_string()]);
        assert!(one.matches(4) && !one.matches(5));
        let many = dict.filter(&["T4".to_string(), "nope".to_string(), "T9".to_string()]);
        assert!(many.matches(4) && many.matches(9) && !many.matches(5));
        assert!(!dict.filter(&["nope".to_string()]).matches(0));

        // A clone shares the dictionary until one side adds a type.
        let mut copy = dict.clone();
        let added = copy.id_or_insert(&Name::new("New"));
        assert_eq!(added, 20);
        assert_eq!(dict.id_of("New"), None);
    }
}
