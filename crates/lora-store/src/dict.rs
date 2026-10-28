//! Per-graph name dictionaries.
//!
//! A graph numbers its labels, relationship types and property keys in
//! first-use order, so stored records and adjacency entries hold a small
//! integer where they would repeat a name. Numbers are never reused: a
//! name whose last user was deleted keeps its number.
//!
//! The numbers are private to one graph and its clones, and are not
//! persisted: snapshots and the WAL write names.
//!
//! A fourth dictionary numbers short string *values* of the keys that
//! repeat a handful of them (a source, a kind, a run id), so a record
//! holds a number in their place (see [`ValueDict`]).

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

/// The dictionaries of a graph.
#[derive(Clone, Default)]
pub(crate) struct Dicts {
    pub(crate) labels: NameDict,
    pub(crate) types: NameDict,
    pub(crate) keys: NameDict,
    pub(crate) values: ValueDict,
}

impl Dicts {
    pub(crate) fn heap_bytes(&self) -> usize {
        self.labels.heap_bytes()
            + self.types.heap_bytes()
            + self.keys.heap_bytes()
            + self.values.heap_bytes()
    }
}

/// Longest string value a record refers to by number. Longer ones are
/// rarely repeated labels of a category, and a number saves little on
/// them next to their length.
const VALUE_MAX_LEN: usize = 32;
/// String values of a key a record stores in full before the key is
/// judged: [`ValueDict`] numbers a key's values once this many show at
/// most [`TRIAL_DISTINCT`] distinct ones.
const TRIAL_VALUES: u16 = 64;
const TRIAL_DISTINCT: usize = 8;
/// New values a numbered key may add before it stops adding them: a key
/// that kept a few values during its trial and then took many more is
/// not one whose values repeat.
const KEY_MISSES: u16 = 64;
/// Values the dictionary holds at most. Numbers are never reused, so this
/// bounds what it can keep.
const VALUES_MAX: usize = 4096;

/// Short string values of the keys that repeat a few of them, numbered
/// like names so a record holds the number (one or two bytes) instead of
/// the string. `source: 'musicbrainz'` in every record of a sync tracker
/// costs a byte where it cost twelve.
///
/// Which keys: each key's first [`TRIAL_VALUES`] short string values are
/// stored in full while their hashes are counted. A key that showed at
/// most [`TRIAL_DISTINCT`] distinct values has its short values numbered
/// from then on, up to [`KEY_MISSES`] new ones; every other key is never
/// numbered. The judgement only changes how a value is stored: a record
/// reads the same string either way.
#[derive(Clone, Default)]
pub(crate) struct ValueDict {
    strings: NameDict,
    /// By key id.
    keys: Arc<Vec<KeyValues>>,
}

#[derive(Clone)]
enum KeyValues {
    /// Hashes of the distinct short values seen, and how many were seen.
    Trial {
        seen: u16,
        distinct: Vec<u64>,
    },
    /// Values are numbered; `misses` counts the new ones added.
    Numbered {
        misses: u16,
    },
    Stored,
}

/// A graph's value dictionary as a snapshot carries it, so a restarted
/// graph stores the values it numbered by number again. Only a hint: a
/// graph without it reads every value the same, and stores more bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueDictionary {
    /// The numbered strings, in number order.
    pub strings: Vec<Arc<str>>,
    /// The keys judged after their trial.
    pub keys: Vec<(Name, ValueKeyState)>,
}

/// How a key's short string values are stored, once judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKeyState {
    /// By number; `misses` new values have been added since the trial.
    Numbered { misses: u16 },
    /// In full.
    Stored,
}

impl Default for KeyValues {
    fn default() -> Self {
        KeyValues::Trial {
            seen: 0,
            distinct: Vec::new(),
        }
    }
}

impl ValueDict {
    /// The number to store for `value` under key `key`, if it is to be
    /// stored by number; counts it towards the key's trial otherwise.
    #[inline]
    pub(crate) fn number(&mut self, key: u32, value: &str) -> Option<u32> {
        if value.len() > VALUE_MAX_LEN {
            return None;
        }
        match self.keys.get(key as usize) {
            Some(KeyValues::Stored) => None,
            Some(KeyValues::Numbered { misses }) => {
                if let Some(id) = self.strings.id_of(value) {
                    return Some(id);
                }
                if *misses >= KEY_MISSES || self.strings.len() >= VALUES_MAX {
                    return None;
                }
                let keys = Arc::make_mut(&mut self.keys);
                if let KeyValues::Numbered { misses } = &mut keys[key as usize] {
                    *misses += 1;
                }
                Some(
                    self.strings
                        .id_or_insert_with(value, || crate::Name::from_arc(Arc::from(value))),
                )
            }
            _ => {
                self.observe(key, value);
                None
            }
        }
    }

    /// Count `value` towards key `key`'s trial, and judge the key once
    /// the trial is over.
    fn observe(&mut self, key: u32, value: &str) {
        let keys = Arc::make_mut(&mut self.keys);
        if keys.len() <= key as usize {
            keys.resize_with(key as usize + 1, KeyValues::default);
        }
        let state = &mut keys[key as usize];
        let KeyValues::Trial { seen, distinct } = state else {
            return;
        };
        let hash = NameState::default().hash_one(value);
        if !distinct.contains(&hash) {
            if distinct.len() == TRIAL_DISTINCT {
                *state = KeyValues::Stored;
                return;
            }
            distinct.push(hash);
        }
        *seen += 1;
        if *seen == TRIAL_VALUES {
            *state = KeyValues::Numbered { misses: 0 };
        }
    }

    /// [`Self::number`] without changing the dictionary: the number of a
    /// value already numbered under a numbered key, `None` for anything
    /// else. Encoders on other threads share one dictionary this way; a
    /// value they store in full reads the same as a numbered one.
    #[inline]
    pub(crate) fn number_existing(&self, key: u32, value: &str) -> Option<u32> {
        if value.len() > VALUE_MAX_LEN {
            return None;
        }
        match self.keys.get(key as usize) {
            Some(KeyValues::Numbered { .. }) => self.strings.id_of(value),
            _ => None,
        }
    }

    /// The numbered strings in number order, and the keys that finished
    /// their trial with the judgement each got. A key still on trial is
    /// left out: its trial starts over.
    pub(crate) fn export(&self, key_name: impl Fn(u32) -> Name) -> ValueDictionary {
        let strings = (0..self.strings.len() as u32)
            .map(|id| Arc::from(self.string(id)))
            .collect();
        let keys = self
            .keys
            .iter()
            .enumerate()
            .filter_map(|(key, state)| {
                let state = match state {
                    KeyValues::Trial { .. } => return None,
                    KeyValues::Numbered { misses } => ValueKeyState::Numbered { misses: *misses },
                    KeyValues::Stored => ValueKeyState::Stored,
                };
                Some((key_name(key as u32), state))
            })
            .collect();
        ValueDictionary { strings, keys }
    }

    /// Restore what [`Self::export`] wrote into an empty dictionary; `key_id`
    /// numbers a key name. Strings past [`VALUES_MAX`], too long to be
    /// numbered, or repeated are refused, so a restored dictionary is one
    /// [`Self::number`] could have built.
    pub(crate) fn import(
        &mut self,
        dictionary: &ValueDictionary,
        mut key_id: impl FnMut(&Name) -> u32,
    ) -> Result<(), String> {
        if dictionary.strings.len() > VALUES_MAX {
            return Err("value dictionary holds too many strings".into());
        }
        for value in &dictionary.strings {
            if value.len() > VALUE_MAX_LEN {
                return Err("value dictionary holds a string too long to number".into());
            }
            let before = self.strings.len();
            self.strings
                .id_or_insert_with(value, || crate::Name::from_arc(Arc::clone(value)));
            if self.strings.len() == before {
                return Err("value dictionary repeats a string".into());
            }
        }
        let keys = Arc::make_mut(&mut self.keys);
        for (name, state) in &dictionary.keys {
            let key = key_id(name) as usize;
            if keys.len() <= key {
                keys.resize_with(key + 1, KeyValues::default);
            }
            keys[key] = match *state {
                ValueKeyState::Numbered { misses } => KeyValues::Numbered {
                    misses: misses.min(KEY_MISSES),
                },
                ValueKeyState::Stored => KeyValues::Stored,
            };
        }
        Ok(())
    }

    /// The string numbered `id`.
    #[inline]
    pub(crate) fn string(&self, id: u32) -> &str {
        self.strings.name(id).as_str()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        // Values are not interned process-wide: count their bytes too.
        let strings: usize = (0..self.strings.len() as u32)
            .map(|id| 2 * std::mem::size_of::<usize>() + self.string(id).len())
            .sum();
        self.strings.heap_bytes()
            + strings
            + self.keys.capacity() * std::mem::size_of::<KeyValues>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key that repeats a few values has them numbered after its trial;
    /// one with many distinct values never does.
    #[test]
    fn values_are_numbered_only_for_keys_that_repeat_a_few() {
        let mut values = ValueDict::default();
        for i in 0..TRIAL_VALUES {
            assert_eq!(values.number(0, ["a", "b", "c"][i as usize % 3]), None);
            assert_eq!(values.number(1, &format!("id-{i}")), None);
        }
        let a = values.number(0, "a").expect("key 0 is numbered");
        assert_eq!(values.string(a), "a");
        assert_eq!(values.number(0, "a"), Some(a));
        assert_eq!(values.number(1, "id-1"), None);
        assert_eq!(values.number(0, &"x".repeat(VALUE_MAX_LEN + 1)), None);
        // New values stop being added after `KEY_MISSES` of them, "a"
        // the first.
        let added = (0..2 * KEY_MISSES)
            .filter(|i| values.number(0, &format!("new-{i}")).is_some())
            .count();
        assert_eq!(added, KEY_MISSES as usize - 1);
        assert_eq!(values.number(0, "a"), Some(a));
    }

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
