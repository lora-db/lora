//! Trigram inverted index for `TEXT` Cypher indexes.
//!
//! Each indexed (label-or-type, property) pair owns a
//! [`TrigramScope`]. The scope maps every overlapping 3-byte window of
//! the string's UTF-8 encoding to the set of entity ids whose value
//! contains that window. This lets `CONTAINS`, `STARTS WITH`, and
//! `ENDS WITH` predicates skip the full property scan: probe by the
//! query's own trigrams, intersect, and refilter the candidate set.
//!
//! ## Why byte-level trigrams
//!
//! Some references describe *Unicode code-point* trigrams.
//! Byte-level trigrams over UTF-8 yield the same set of valid
//! 3-byte aligned windows for pure-ASCII text and a strict
//! superset for multibyte text — the index *over*-collects, the
//! refilter step (mandatory anyway, since any trigram set can match
//! a substring not actually present) trims it back. In return we
//! avoid the cost of decoding every char to look up a single key.
//!
//! ## Maintenance contract
//!
//! All mutations go through the existing `MutationEvent` stream;
//! `InMemoryGraph::on_node_property_set` etc. delegate updates here
//! when the registry has at least one matching scope. When the last
//! scope referencing a (label, property) pair is dropped, the bucket
//! itself is removed so an empty TEXT catalog pays zero memory.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::entity_index_store::ScopedPropertyKey;

/// Registry of trigram scopes for either nodes or relationships.
#[derive(Debug, Default, Clone)]
pub(super) struct TrigramRegistry {
    by_scope: HashMap<ScopedPropertyKey, TrigramScope>,
}

#[derive(Debug, Default, Clone)]
pub(super) struct TrigramScope {
    /// Trigram → entity ids whose property value contains it.
    grams: BTreeMap<[u8; 3], BTreeSet<u64>>,
    /// Reference count: how many catalog entries point at this scope.
    /// We allow multiple TEXT indexes on the same `(label, property)`
    /// (different names, redundant); the scope is freed only when the
    /// last reference is dropped.
    refcount: u32,
}

impl TrigramRegistry {
    /// Mark a scope as in-use, allocating it if missing. Returns
    /// `true` if the scope was freshly created (caller should
    /// backfill it from existing data).
    pub(super) fn add_scope(&mut self, label: &str, property: &str) -> bool {
        let key = ScopedPropertyKey::new(label, property);
        let entry = self.by_scope.entry(key).or_default();
        let was_empty = entry.refcount == 0;
        entry.refcount = entry.refcount.saturating_add(1);
        was_empty
    }

    /// Decrement the refcount on a scope. The scope is removed once
    /// the last reference is gone.
    pub(super) fn remove_scope(&mut self, label: &str, property: &str) {
        let key = ScopedPropertyKey::new(label, property);
        if let Some(scope) = self.by_scope.get_mut(&key) {
            scope.refcount = scope.refcount.saturating_sub(1);
            if scope.refcount == 0 {
                self.by_scope.remove(&key);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn has_scope(&self, label: &str, property: &str) -> bool {
        self.by_scope
            .contains_key(&ScopedPropertyKey::new(label, property))
    }

    pub(super) fn insert(&mut self, label: &str, property: &str, id: u64, value: &str) {
        if let Some(scope) = self
            .by_scope
            .get_mut(&ScopedPropertyKey::new(label, property))
        {
            scope.insert(id, value);
        }
    }

    #[cfg(test)]
    pub(super) fn remove(&mut self, label: &str, property: &str, id: u64, value: &str) {
        if let Some(scope) = self
            .by_scope
            .get_mut(&ScopedPropertyKey::new(label, property))
        {
            scope.remove(id, value);
        }
    }

    pub(super) fn update(
        &mut self,
        label: &str,
        property: &str,
        id: u64,
        old: Option<&str>,
        new: Option<&str>,
    ) {
        let Some(scope) = self
            .by_scope
            .get_mut(&ScopedPropertyKey::new(label, property))
        else {
            return;
        };
        if let Some(old) = old {
            scope.remove(id, old);
        }
        if let Some(new) = new {
            scope.insert(id, new);
        }
    }

    /// Candidate ids whose property value *might* contain `query`.
    /// `None` when the query is too short for trigram matching (callers
    /// should fall back to scan).
    pub(super) fn candidates(
        &self,
        label: &str,
        property: &str,
        query: &str,
    ) -> Option<BTreeSet<u64>> {
        let scope = self
            .by_scope
            .get(&ScopedPropertyKey::new(label, property))?;
        scope.candidates(query)
    }
}

impl TrigramScope {
    fn insert(&mut self, id: u64, value: &str) {
        for tri in trigrams(value) {
            self.grams.entry(tri).or_default().insert(id);
        }
    }

    fn remove(&mut self, id: u64, value: &str) {
        for tri in trigrams(value) {
            if let Some(set) = self.grams.get_mut(&tri) {
                set.remove(&id);
                if set.is_empty() {
                    self.grams.remove(&tri);
                }
            }
        }
    }

    fn candidates(&self, query: &str) -> Option<BTreeSet<u64>> {
        let mut grams: Vec<[u8; 3]> = trigrams(query).collect();
        if grams.is_empty() {
            // Query shorter than 3 bytes — every entity is a potential
            // match. Returning `None` signals the caller to scan.
            return None;
        }
        // Probe lowest-cardinality trigram first — once that bucket is
        // narrowed, intersection on subsequent probes runs over a
        // smaller working set.
        grams.sort_by_key(|tri| self.grams.get(tri).map(BTreeSet::len).unwrap_or(usize::MAX));
        let first = self.grams.get(&grams[0])?;
        let mut out = first.clone();
        for tri in &grams[1..] {
            let next = self.grams.get(tri)?;
            out.retain(|id| next.contains(id));
            if out.is_empty() {
                return Some(out);
            }
        }
        Some(out)
    }
}

fn trigrams(value: &str) -> impl Iterator<Item = [u8; 3]> + '_ {
    let bytes = value.as_bytes();
    (0..bytes.len().saturating_sub(2)).map(move |i| {
        let window = &bytes[i..i + 3];
        [window[0], window[1], window[2]]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_none() {
        let mut reg = TrigramRegistry::default();
        reg.add_scope("Person", "name");
        reg.insert("Person", "name", 1, "Alice");
        assert!(reg.candidates("Person", "name", "ab").is_none());
    }

    #[test]
    fn substring_match_intersects_trigrams() {
        let mut reg = TrigramRegistry::default();
        reg.add_scope("Person", "name");
        reg.insert("Person", "name", 1, "Alexandra");
        reg.insert("Person", "name", 2, "Alexander");
        reg.insert("Person", "name", 3, "Bob");

        let candidates = reg.candidates("Person", "name", "Alex").unwrap();
        assert!(candidates.contains(&1));
        assert!(candidates.contains(&2));
        assert!(!candidates.contains(&3));
    }

    #[test]
    fn remove_after_insert_clears_buckets() {
        let mut reg = TrigramRegistry::default();
        reg.add_scope("Person", "name");
        reg.insert("Person", "name", 1, "Alice");
        reg.remove("Person", "name", 1, "Alice");
        assert!(reg
            .candidates("Person", "name", "Alic")
            .map(|s| s.is_empty())
            .unwrap_or(true));
    }

    #[test]
    fn refcount_keeps_scope_until_last_remove() {
        let mut reg = TrigramRegistry::default();
        assert!(reg.add_scope("Person", "name"));
        assert!(!reg.add_scope("Person", "name")); // already exists
        reg.insert("Person", "name", 1, "Alice");
        reg.remove_scope("Person", "name");
        assert!(reg.has_scope("Person", "name"));
        reg.remove_scope("Person", "name");
        assert!(!reg.has_scope("Person", "name"));
    }
}
