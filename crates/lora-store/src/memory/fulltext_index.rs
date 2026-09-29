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
//!   * ASCII-fold: strip diacritics and map letters without a
//!     decomposition (`ø` → `o`, `æ` → `ae`, `ß` → `ss`), so `Sonar`
//!     matches `Sónar` and `Oya*` matches `Øyafestivalen`. Indexing and
//!     querying share the analyzer, so folded and accented queries match
//!     the same documents,
//!   * split on Unicode non-alphanumeric characters (punctuation,
//!     whitespace, control chars),
//!   * drop empty fragments.
//!
//! Queries AND their terms; a term ending in `*` matches every indexed
//! term with that prefix.
//!
//! Maintenance is synchronous: every property set / unset on a covered
//! `(entity, property)` triggers a re-index call through the secondary
//! index maintenance path (`secondary_index_maintenance.rs`). The
//! `fulltext.eventually_consistent` OPTION parses but is currently a
//! no-op — we always apply changes inline.

use std::collections::{BTreeMap, HashMap};

use std::sync::Arc;

use super::cow::{CowIdMap, CowMap, CowOrdMap};
use crate::Properties;

use super::StoredIndexEntity;

pub(super) type TermCounts = BTreeMap<String, u32>;
pub(super) type PropertyTermCounts = BTreeMap<String, TermCounts>;

/// Split `text` into the lowercase, ASCII-folded tokens used by both
/// indexing and query parsing. Mirrors Lucene's "standard" analyzer plus
/// an ASCII-folding filter: alphanumeric runs are tokens; everything
/// else is a separator.
pub fn standard_analyzer(text: &str) -> Vec<String> {
    analyze(text, false).into_iter().map(|(t, _)| t).collect()
}

/// Tokenise, optionally keeping a trailing `*` on a token as a prefix
/// marker (query side). Returns `(token, is_prefix)`.
fn analyze(text: &str, keep_prefix_marker: bool) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for low in ch.to_lowercase() {
                push_folded(&mut buf, low);
            }
        } else if !buf.is_empty() {
            let prefix = keep_prefix_marker && ch == '*';
            out.push((std::mem::take(&mut buf), prefix));
        }
    }
    if !buf.is_empty() {
        out.push((buf, false));
    }
    out
}

/// Append `ch` to `buf` with diacritics removed. Canonical decomposition
/// splits `ó` into `o` + a combining accent, which is dropped; letters
/// that have no decomposition but a conventional ASCII spelling are
/// mapped explicitly. Anything else (CJK, Cyrillic, ...) passes through.
fn push_folded(buf: &mut String, ch: char) {
    if ch.is_ascii() {
        buf.push(ch);
        return;
    }
    let mapped = match ch {
        'ø' => Some("o"),
        'æ' => Some("ae"),
        'œ' => Some("oe"),
        'ß' => Some("ss"),
        'đ' | 'ð' => Some("d"),
        'ł' => Some("l"),
        'þ' => Some("th"),
        'ı' => Some("i"),
        'ħ' => Some("h"),
        _ => None,
    };
    if let Some(s) = mapped {
        buf.push_str(s);
        return;
    }
    unicode_normalization::char::decompose_canonical(ch, |c| {
        // Combining diacritical marks.
        if !('\u{0300}'..='\u{036f}').contains(&c) {
            buf.push(c);
        }
    });
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

    pub(super) fn by_name(&self) -> impl Iterator<Item = (&String, &FulltextIndex)> {
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

    /// Whether any index in this registry holds `entity_id`.
    pub(super) fn indexes_entity(&self, entity_id: u64) -> bool {
        self.by_name
            .values()
            .any(|index| index.entity_terms.contains_key(&entity_id))
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
    /// Keys are `Arc<str>` shared with `entity_terms`, so copying a shard
    /// for a write bumps refcounts instead of reallocating term strings.
    pub(super) postings: CowOrdMap<Arc<str>, CowIdMap<u32>>,
    /// `entity → set<term>` reverse map so re-indexing can remove the
    /// stale contribution before adding the new one.
    pub(super) entity_terms: CowMap<u64, Arc<[Arc<str>]>>,
}

impl FulltextIndex {
    fn new(labels: Vec<String>, properties: Vec<String>) -> Self {
        Self {
            labels,
            properties,
            postings: CowOrdMap::default(),
            entity_terms: CowMap::default(),
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
            for term in old_terms.iter() {
                let emptied = self.postings.get_mut(&**term).is_some_and(|bucket| {
                    bucket.remove(&entity_id);
                    bucket.is_empty()
                });
                if emptied {
                    self.postings.remove(&**term);
                }
            }
        }
        if terms.is_empty() {
            return;
        }
        let mut new_terms: Vec<Arc<str>> = Vec::with_capacity(terms.len());
        for (term, tf) in terms {
            let term: Arc<str> = Arc::from(term);
            self.postings
                .get_or_insert_with(term.clone(), CowIdMap::default)
                .insert(entity_id, tf);
            new_terms.push(term);
        }
        self.entity_terms.insert(entity_id, new_terms.into());
    }

    pub(super) fn remove_entity(&mut self, entity_id: u64) {
        if let Some(terms) = self.entity_terms.remove(&entity_id) {
            for term in terms.iter() {
                let emptied = self.postings.get_mut(&**term).is_some_and(|bucket| {
                    bucket.remove(&entity_id);
                    bucket.is_empty()
                });
                if emptied {
                    self.postings.remove(&**term);
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
        let tokens = analyze(query_text, true);
        if tokens.is_empty() {
            return Vec::new();
        }
        // Resolve each query term to a posting list. A prefix term merges
        // the postings of every indexed term starting with it.
        let mut merged: Vec<BTreeMap<u64, u32>> = Vec::new();
        let mut exact: Vec<&CowIdMap<u32>> = Vec::with_capacity(tokens.len());
        for (token, prefix) in &tokens {
            if *prefix {
                let mut union: BTreeMap<u64, u32> = BTreeMap::new();
                let prefix = token.clone();
                for (_, posting) in self
                    .postings
                    .range(
                        std::ops::Bound::Included(Arc::from(token.as_str())),
                        std::ops::Bound::Unbounded,
                    )
                    .take_while(|(term, _)| term.starts_with(prefix.as_str()))
                {
                    for (id, tf) in posting.iter() {
                        let slot = union.entry(*id).or_insert(0);
                        *slot = slot.saturating_add(*tf);
                    }
                }
                if union.is_empty() {
                    return Vec::new();
                }
                merged.push(union);
                continue;
            }
            match self.postings.get(token.as_str()) {
                Some(p) => exact.push(p),
                None => return Vec::new(), // term not present → AND fails
            }
        }
        // Find the smallest posting list to seed the intersection.
        let mut posting_iter: Vec<Posting<'_>> = exact.into_iter().map(Posting::Index).collect();
        posting_iter.extend(merged.iter().map(Posting::Merged));
        posting_iter.sort_by_key(|p| p.len());

        let mut results: BTreeMap<u64, u32> = BTreeMap::new();
        // Seed with the smallest list.
        let Some(seed) = posting_iter.first() else {
            return Vec::new();
        };
        for (id, tf) in seed.iter() {
            results.insert(id, tf);
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

/// A query term's posting list: straight from the index, or the union
/// built for a prefix term.
enum Posting<'a> {
    Index(&'a CowIdMap<u32>),
    Merged(&'a BTreeMap<u64, u32>),
}

impl Posting<'_> {
    fn len(&self) -> usize {
        match self {
            Posting::Index(p) => p.len(),
            Posting::Merged(p) => p.len(),
        }
    }

    fn get(&self, id: &u64) -> Option<&u32> {
        match self {
            Posting::Index(p) => p.get(id),
            Posting::Merged(p) => p.get(id),
        }
    }

    fn iter(&self) -> Box<dyn Iterator<Item = (u64, u32)> + '_> {
        match self {
            Posting::Index(p) => Box::new(p.iter().map(|(id, tf)| (*id, *tf))),
            Posting::Merged(p) => Box::new(p.iter().map(|(id, tf)| (*id, *tf))),
        }
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
            out.insert(key.to_string(), tokenize_to_term_counts(value));
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
