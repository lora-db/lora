//! In-memory inverted index for `CREATE FULLTEXT INDEX`.
//!
//! Each catalog-registered fulltext index owns a [`FulltextIndex`]
//! holding:
//!
//!   * the label / rel-type set it covers (`labels`, any-of semantics),
//!   * the property set it covers (`properties`, any-of semantics),
//!   * a posting list `term → entity_id → term_frequency`,
//!   * a per-entity reverse map `entity_id → set<term>` so re-indexing
//!     on update can drop the old contribution without rescanning the
//!     full posting list.
//!
//! Tokenisation is delegated to [`standard_analyzer`] which is a tiny
//! Lucene-style "standard" analyzer:
//!   * lowercase,
//!   * split on Unicode non-alphanumeric characters (punctuation,
//!     whitespace, control chars),
//!   * drop empty fragments.
//!
//! Maintenance is synchronous: every property set / unset on a covered
//! `(entity, property)` triggers a re-index call through the secondary
//! index maintenance path (`secondary_index_maintenance.rs`). The
//! `fulltext.eventually_consistent` OPTION parses but is currently a
//! no-op — we always apply changes inline.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::Properties;

use super::StoredIndexEntity;

pub(super) type TermCounts = BTreeMap<String, u32>;
pub(super) type PropertyTermCounts = BTreeMap<String, TermCounts>;

/// Split `text` into the lowercase tokens used by both indexing and
/// query parsing. Mirrors Lucene's "standard" analyzer for ASCII text:
/// alphanumeric runs are tokens; everything else is a separator.
pub fn standard_analyzer(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for low in ch.to_lowercase() {
                buf.push(low);
            }
        } else if !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Registry of fulltext indexes for either nodes or relationships.
#[derive(Debug, Default, Clone)]
pub(super) struct FulltextRegistry {
    by_name: HashMap<String, FulltextIndex>,
}

impl FulltextRegistry {
    pub(super) fn register(&mut self, name: String, labels: Vec<String>, properties: Vec<String>) {
        let entry = FulltextIndex::new(labels, properties);
        self.by_name.insert(name, entry);
    }

    pub(super) fn deregister(&mut self, name: &str) {
        self.by_name.remove(name);
    }

    pub(super) fn get(&self, name: &str) -> Option<&FulltextIndex> {
        self.by_name.get(name)
    }

    pub(super) fn get_mut(&mut self, name: &str) -> Option<&mut FulltextIndex> {
        self.by_name.get_mut(name)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &FulltextIndex)> {
        self.by_name.iter()
    }

    pub(super) fn by_name_mut(&mut self) -> impl Iterator<Item = (&String, &mut FulltextIndex)> {
        self.by_name.iter_mut()
    }

    /// Indexes covering at least one of the supplied labels. Callers
    /// pass the labels of the entity being mutated to find every index
    /// that needs to see the update.
    pub(super) fn indexes_for_labels<'a, I>(
        &'a self,
        labels: I,
    ) -> impl Iterator<Item = (&'a String, &'a FulltextIndex)>
    where
        I: IntoIterator<Item = &'a str>,
        I::IntoIter: Clone,
    {
        let labels = labels.into_iter();
        self.by_name
            .iter()
            .filter(move |(_, idx)| idx.covers_any_label(labels.clone()))
    }

    /// Mutable iterator for maintenance writes. Same matching rule as
    /// [`Self::indexes_for_labels`].
    pub(super) fn indexes_for_labels_mut<'a, I>(
        &'a mut self,
        labels: I,
    ) -> impl Iterator<Item = (&'a String, &'a mut FulltextIndex)>
    where
        I: IntoIterator<Item = &'a str>,
        I::IntoIter: Clone,
    {
        let labels = labels.into_iter();
        self.by_name
            .iter_mut()
            .filter(move |(_, idx)| idx.covers_any_label(labels.clone()))
    }

    pub(super) fn remove_entity_everywhere(&mut self, entity_id: u64) {
        for index in self.by_name.values_mut() {
            index.remove_entity(entity_id);
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct FulltextIndex {
    pub labels: Vec<String>,
    pub properties: Vec<String>,
    /// `term → entity → term_frequency`. Term frequency is the count of
    /// tokens for the entity across all covered properties.
    postings: BTreeMap<String, BTreeMap<u64, u32>>,
    /// `entity → set<term>` reverse map so re-indexing can remove the
    /// stale contribution before adding the new one.
    entity_terms: BTreeMap<u64, BTreeSet<String>>,
}

impl FulltextIndex {
    fn new(labels: Vec<String>, properties: Vec<String>) -> Self {
        Self {
            labels,
            properties,
            postings: BTreeMap::new(),
            entity_terms: BTreeMap::new(),
        }
    }

    pub(super) fn property_is_covered(&self, property: &str) -> bool {
        self.properties.iter().any(|p| p == property)
    }

    pub(super) fn covers_any_label<'a>(&self, labels: impl IntoIterator<Item = &'a str>) -> bool {
        labels
            .into_iter()
            .any(|label| self.labels.iter().any(|wanted| wanted == label))
    }

    /// Replace this entity's contribution with `terms`. `terms` is the
    /// full set of (term, count) pairs derived from the union of all
    /// covered properties of the entity at its current state. Pass an
    /// empty iterator to drop the entity entirely.
    pub(super) fn reindex_entity(&mut self, entity_id: u64, terms: TermCounts) {
        // Drop old contribution.
        if let Some(old_terms) = self.entity_terms.remove(&entity_id) {
            for term in old_terms {
                if let Some(bucket) = self.postings.get_mut(&term) {
                    bucket.remove(&entity_id);
                    if bucket.is_empty() {
                        self.postings.remove(&term);
                    }
                }
            }
        }
        if terms.is_empty() {
            return;
        }
        let mut new_terms = BTreeSet::new();
        for (term, tf) in terms {
            self.postings
                .entry(term.clone())
                .or_default()
                .insert(entity_id, tf);
            new_terms.insert(term);
        }
        self.entity_terms.insert(entity_id, new_terms);
    }

    pub(super) fn remove_entity(&mut self, entity_id: u64) {
        if let Some(terms) = self.entity_terms.remove(&entity_id) {
            for term in terms {
                if let Some(bucket) = self.postings.get_mut(&term) {
                    bucket.remove(&entity_id);
                    if bucket.is_empty() {
                        self.postings.remove(&term);
                    }
                }
            }
        }
    }

    /// Run a query against the index. Tokenises with the standard
    /// analyzer and returns `(entity_id, score)` for entities that
    /// contain *all* query terms (AND semantics). Score is the sum of
    /// term frequencies across the matched terms; ties broken by
    /// entity id ascending.
    pub(super) fn query(&self, query_text: &str) -> Vec<(u64, f64)> {
        let tokens = standard_analyzer(query_text);
        if tokens.is_empty() {
            return Vec::new();
        }
        // Find the smallest posting list to seed the intersection.
        let mut posting_iter: Vec<&BTreeMap<u64, u32>> = Vec::with_capacity(tokens.len());
        for token in &tokens {
            match self.postings.get(token) {
                Some(p) => posting_iter.push(p),
                None => return Vec::new(), // term not present → AND fails
            }
        }
        posting_iter.sort_by_key(|p| p.len());

        let mut results: BTreeMap<u64, u32> = BTreeMap::new();
        // Seed with the smallest list.
        let Some(seed) = posting_iter.first() else {
            return Vec::new();
        };
        for (id, tf) in *seed {
            results.insert(*id, *tf);
        }
        // Intersect with the rest, summing TF as we go.
        for posting in posting_iter.iter().skip(1) {
            let mut next: BTreeMap<u64, u32> = BTreeMap::new();
            for (id, acc) in &results {
                if let Some(tf) = posting.get(id) {
                    next.insert(*id, acc.saturating_add(*tf));
                }
            }
            results = next;
            if results.is_empty() {
                return Vec::new();
            }
        }
        let mut out: Vec<(u64, f64)> = results
            .into_iter()
            .map(|(id, tf)| (id, tf as f64))
            .collect();
        // Descending score, ascending id for ties.
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out
    }
}

/// Tokenise `value` and produce per-term frequencies for indexing.
pub(super) fn tokenize_to_term_counts(value: &str) -> TermCounts {
    let mut out = TermCounts::new();
    for tok in standard_analyzer(value) {
        *out.entry(tok).or_insert(0) += 1;
    }
    out
}

pub(super) fn string_property_term_counts(properties: &Properties) -> PropertyTermCounts {
    let mut out = PropertyTermCounts::new();
    for (key, value) in properties {
        if let crate::PropertyValue::String(value) = value {
            out.insert(key.clone(), tokenize_to_term_counts(value));
        }
    }
    out
}

pub(super) fn term_counts_for_properties(
    properties: &Properties,
    selected_properties: &[String],
) -> TermCounts {
    let by_property = string_property_term_counts(properties);
    term_counts_for_selected_properties(&by_property, selected_properties)
}

pub(super) fn term_counts_for_selected_properties(
    by_property: &PropertyTermCounts,
    selected_properties: &[String],
) -> TermCounts {
    let mut out = TermCounts::new();
    for property in selected_properties {
        if let Some(counts) = by_property.get(property) {
            merge_term_counts(&mut out, counts.clone());
        }
    }
    out
}

/// Merge `more` into `into`, summing counts.
pub(super) fn merge_term_counts(into: &mut TermCounts, more: TermCounts) {
    for (k, v) in more {
        *into.entry(k).or_insert(0) += v;
    }
}

/// Identifier for a fulltext registry, by entity scope.
#[allow(dead_code)]
pub(super) fn registry_for_entity_kind(entity: StoredIndexEntity) -> &'static str {
    match entity {
        StoredIndexEntity::Node => "node",
        StoredIndexEntity::Relationship => "relationship",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyzer_lowercases_and_splits_on_punct() {
        assert_eq!(
            standard_analyzer("Hello, World! 42"),
            vec!["hello", "world", "42"]
        );
    }

    #[test]
    fn analyzer_collapses_runs() {
        assert_eq!(standard_analyzer("a   b\tc\n d"), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn reindex_replaces_old_contribution() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("foo bar"));
        idx.reindex_entity(1, tokenize_to_term_counts("baz"));
        // The old terms should not return entity 1 anymore.
        let r = idx.query("foo");
        assert!(r.is_empty(), "expected empty after reindex, got {r:?}");
        let r = idx.query("baz");
        assert_eq!(r, vec![(1, 1.0)]);
    }

    #[test]
    fn query_intersects_terms() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("alpha beta gamma"));
        idx.reindex_entity(2, tokenize_to_term_counts("alpha gamma delta"));
        let r = idx.query("alpha beta");
        assert_eq!(r, vec![(1, 2.0)], "only entity 1 has both terms");
    }

    #[test]
    fn query_returns_empty_for_unknown_term() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("alpha"));
        assert!(idx.query("zeta").is_empty());
    }
}
