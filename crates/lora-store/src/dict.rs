//! Per-graph name dictionaries.
//!
//! A graph numbers its labels, relationship types and property keys in
//! first-use order, so stored records and adjacency entries hold a small
//! integer where they would repeat a name. Numbers are never reused: a
//! name whose last user was deleted keeps its number.
//!
//! The numbers are private to one graph and its clones, and are not
//! persisted: snapshots and the WAL write names.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;

use crate::Name;

/// Dictionaries up to this size are searched linearly: comparing a
/// handful of short names beats hashing one.
const LINEAR_LOOKUP_MAX: usize = 8;

/// FNV-1a. Dictionary keys are short names chosen by the schema, and a
/// property read hashes one, so the hash has to be cheap; these maps
/// are not exposed to hostile key sets the way a general map is, since
/// a name must first be created as a label, type or key.
#[derive(Clone, Copy)]
pub(crate) struct NameHasher(u64);

impl Default for NameHasher {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for NameHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// One dictionary: names numbered from zero.
///
/// Cloning shares the table; the first `id_or_insert` of a new name on
/// either side copies it.
#[derive(Clone, Default)]
pub(crate) struct NameDict {
    inner: Arc<Inner>,
}

#[derive(Clone, Default)]
struct Inner {
    names: Vec<Name>,
    ids: HashMap<Name, u32, BuildHasherDefault<NameHasher>>,
}

impl NameDict {
    #[inline]
    pub(crate) fn id_of(&self, name: &str) -> Option<u32> {
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
    pub(crate) fn id_or_insert(&mut self, name: &Name) -> u32 {
        if let Some(id) = self.id_of(name) {
            return id;
        }
        let inner = Arc::make_mut(&mut self.inner);
        let id = inner.names.len() as u32;
        inner.names.push(name.clone());
        inner.ids.insert(name.clone(), id);
        id
    }

    /// As [`Self::id_or_insert`], for a name not yet interned.
    #[cfg(test)]
    pub(crate) fn id_or_insert_str(&mut self, name: &str) -> u32 {
        match self.id_of(name) {
            Some(id) => id,
            None => self.id_or_insert(&Name::new(name)),
        }
    }

    /// The name numbered `id`. Ids come from this dictionary or one it
    /// was cloned from, so the lookup cannot miss.
    #[inline]
    pub(crate) fn name(&self, id: u32) -> &Name {
        &self.inner.names[id as usize]
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner.names.len()
    }

    /// Heap bytes the table owns. Name buffers are interned and shared.
    pub(crate) fn heap_bytes(&self) -> usize {
        let inner = &*self.inner;
        inner.names.capacity() * std::mem::size_of::<Name>()
            + inner.ids.capacity() * (std::mem::size_of::<Name>() + std::mem::size_of::<u32>() + 1)
    }
}

/// The three dictionaries of a graph.
#[derive(Clone, Default)]
pub(crate) struct Dicts {
    pub(crate) labels: NameDict,
    pub(crate) types: NameDict,
    pub(crate) keys: NameDict,
}

impl Dicts {
    pub(crate) fn heap_bytes(&self) -> usize {
        self.labels.heap_bytes() + self.types.heap_bytes() + self.keys.heap_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_numbered_once_in_both_lookup_modes() {
        let mut dict = NameDict::default();
        let names: Vec<Name> = (0..20).map(|i| Name::new(&format!("N{i}"))).collect();
        for (i, name) in names.iter().enumerate() {
            assert_eq!(dict.id_or_insert(name), i as u32);
            // Both the linear and the hashed lookup see every earlier name.
            for (j, earlier) in names[..=i].iter().enumerate() {
                assert_eq!(dict.id_of(earlier), Some(j as u32));
                assert_eq!(dict.name(j as u32), earlier);
            }
        }
        assert_eq!(dict.id_or_insert(&names[3]), 3);
        assert_eq!(dict.id_or_insert_str("N7"), 7);
        assert_eq!(dict.id_or_insert_str("fresh"), 20);
        assert_eq!(dict.id_of("missing"), None);
        assert_eq!(dict.len(), 21);
    }

    #[test]
    fn a_clone_shares_the_table_until_one_side_adds_a_name() {
        let mut dict = NameDict::default();
        dict.id_or_insert_str("A");
        let mut copy = dict.clone();
        assert_eq!(copy.id_or_insert_str("B"), 1);
        assert_eq!(dict.id_of("B"), None);
        assert_eq!(dict.id_or_insert_str("C"), 1);
        assert_eq!(copy.name(1).as_str(), "B");
        assert_eq!(dict.name(1).as_str(), "C");
    }
}
