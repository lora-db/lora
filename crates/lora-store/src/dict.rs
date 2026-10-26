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
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, OnceLock};

use crate::Name;

/// Dictionaries up to this size are searched linearly: comparing a
/// handful of short names beats hashing one.
const LINEAR_LOOKUP_MAX: usize = 8;

/// A random value per process, mixed into the starting state of the
/// store's fast hashers ([`NameHasher`] and the index `KeyHasher`). They
/// hash attacker-chosen strings (property keys, indexed values) with
/// cheap non-cryptographic functions; with a fixed start, colliding
/// inputs could be computed once and replayed against every process.
/// Nothing hashed with it is persisted or ordered by its hash.
#[inline]
pub(crate) fn process_hash_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        // `RandomState` draws its keys from the OS once per thread.
        std::collections::hash_map::RandomState::new().hash_one(0x5eed_u64) | 1
    })
}

/// FNV-1a from a per-process random basis. A property read hashes its
/// key, so the hash has to be cheap. It is not collision-resistant, but
/// the seed keeps collisions from being precomputed: someone who can
/// write arbitrary property keys would have to find them per process.
#[derive(Clone, Copy)]
pub(crate) struct NameHasher(u64);

/// Builds [`NameHasher`]s from the seeded basis, read once per table
/// rather than once per hash.
#[derive(Clone, Copy)]
pub(crate) struct NameState(u64);

impl Default for NameState {
    #[inline]
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325 ^ process_hash_seed())
    }
}

impl BuildHasher for NameState {
    type Hasher = NameHasher;

    #[inline]
    fn build_hasher(&self) -> NameHasher {
        NameHasher(self.0)
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
    ids: HashMap<Name, u32, NameState>,
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
        self.id_or_insert_with(name, || name.clone())
    }

    /// As [`Self::id_or_insert`] for a name held as a `&str`: `make` builds
    /// the shared name, and only when it is new.
    ///
    /// A new name copies the table when a graph clone still shares it, so
    /// the first write of each new name after a clone costs O(names).
    pub(crate) fn id_or_insert_with(&mut self, name: &str, make: impl FnOnce() -> Name) -> u32 {
        if let Some(id) = self.id_of(name) {
            return id;
        }
        let name = make();
        let inner = Arc::make_mut(&mut self.inner);
        let id = inner.names.len() as u32;
        inner.names.push(name.clone());
        inner.ids.insert(name, id);
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

    /// The name hasher starts from the per-process seed, not a
    /// fixed basis, so colliding names cannot be precomputed.
    #[test]
    fn hashers_are_seeded_per_process() {
        use std::hash::BuildHasher;
        let unseeded = {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for &b in b"name" {
                h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        };
        let mut hasher = NameState::default().build_hasher();
        hasher.write(b"name");
        assert_ne!(hasher.finish(), unseeded);
        assert_eq!(
            NameState::default().hash_one("name"),
            NameState::default().hash_one("name")
        );
    }
}
