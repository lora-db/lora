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

use super::cow::{CowMap, CowOrdMap};
use super::id_map::CowIdMap;

use super::StoredIndexEntity;

/// Partition size of the term postings and leaf size of the per-entity
/// term lists. Reindexing an entity touches one partition per term and a
/// staged write copies each: a quarter of the defaults cut that copying
/// (and the old copies' drop) without slowing queries.
const TERM_PART: usize = 64;
const ENTITY_LEAF: usize = 112;

/// An entity's terms, in term order, sharing the posting lists' keys.
type EntityTerms = Arc<[Arc<str>]>;

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
    for_each_token(text, keep_prefix_marker, &mut buf, |token, prefix| {
        out.push((token.to_owned(), prefix))
    });
    out
}

/// The tokens of `text` in order, each with whether a `*` ended it (only
/// with `keep_prefix_marker`), spelled in `buf`: a caller tokenising many
/// texts keeps one buffer and allocates nothing per token.
fn for_each_token(
    text: &str,
    keep_prefix_marker: bool,
    buf: &mut String,
    mut emit: impl FnMut(&str, bool),
) {
    buf.clear();
    for ch in text.chars() {
        if ch.is_ascii() {
            // Most text: no case table, no decomposition.
            if ch.is_ascii_alphanumeric() {
                buf.push(ch.to_ascii_lowercase());
                continue;
            }
        } else if ch.is_alphanumeric() {
            for low in ch.to_lowercase() {
                push_folded(buf, low);
            }
            continue;
        }
        if !buf.is_empty() {
            emit(buf, keep_prefix_marker && ch == '*');
            buf.clear();
        }
    }
    if !buf.is_empty() {
        emit(buf, false);
        buf.clear();
    }
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
    /// Keys are `Arc<str>` shared with `entity_terms`, so copying a
    /// partition for a write bumps refcounts instead of reallocating term
    /// strings.
    pub(super) postings: CowOrdMap<Arc<str>, CowIdMap<u32>, TERM_PART>,
    /// `entity → set<term>` reverse map so re-indexing can remove the
    /// stale contribution before adding the new one.
    pub(super) entity_terms: CowMap<u64, Arc<[Arc<str>]>, ENTITY_LEAF>,
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

    /// An index of the entities `chunks` hold, which must come in
    /// ascending entity order with each entity once. The same index
    /// [`Self::reindex_entity`] builds entity by entity, without a walk of
    /// the term map per term of each entity: the distinct terms are sorted
    /// once, the postings are filed under their term's rank by counting,
    /// and both maps are filled in order, on every core when the build
    /// allows.
    pub(super) fn from_chunks(
        labels: Vec<String>,
        properties: Vec<String>,
        chunks: Vec<TokenisedChunk>,
    ) -> Self {
        let mut index = Self::new(labels, properties);
        debug_assert!(chunks
            .iter()
            .flat_map(|chunk| chunk.entities.iter().map(|(id, _)| *id))
            .is_sorted_by(|a, b| a < b));

        // Number the distinct terms across the chunks, then rank them.
        let mut numbers: HashMap<&str, u32> = HashMap::new();
        let mut distinct: Vec<&str> = Vec::new();
        let global: Vec<Vec<u32>> = chunks
            .iter()
            .map(|chunk| {
                chunk
                    .terms
                    .iter()
                    .map(|term| {
                        *numbers.entry(term.as_str()).or_insert_with(|| {
                            distinct.push(term.as_str());
                            (distinct.len() - 1) as u32
                        })
                    })
                    .collect()
            })
            .collect();
        drop(numbers);
        if distinct.is_empty() {
            return index;
        }
        let mut order: Vec<u32> = (0..distinct.len() as u32).collect();
        par_sort_unstable_by(&mut order, |a, b| {
            distinct[*a as usize].cmp(distinct[*b as usize])
        });
        let mut rank_of = vec![0u32; distinct.len()];
        for (rank, number) in order.iter().enumerate() {
            rank_of[*number as usize] = rank as u32;
        }
        let terms: Vec<Arc<str>> = order
            .iter()
            .map(|number| Arc::from(distinct[*number as usize]))
            .collect();
        drop(order);
        // Each chunk's term numbers as ranks.
        let ranks: Vec<Vec<u32>> = global
            .into_iter()
            .map(|numbers| {
                numbers
                    .into_iter()
                    .map(|number| rank_of[number as usize])
                    .collect()
            })
            .collect();
        drop(rank_of);

        // File every posting under its term's rank. The chunks are read in
        // entity order, so each term's entities land in id order.
        let mut starts = vec![0usize; terms.len() + 1];
        for (chunk, ranks) in chunks.iter().zip(&ranks) {
            for (term, _) in &chunk.counts {
                starts[ranks[*term as usize] as usize + 1] += 1;
            }
        }
        for rank in 0..terms.len() {
            starts[rank + 1] += starts[rank];
        }
        let mut filed: Vec<(u64, u32)> = vec![(0, 0); starts[terms.len()]];
        let mut next = starts.clone();
        for (chunk, ranks) in chunks.iter().zip(&ranks) {
            let mut from = 0;
            for (id, end) in &chunk.entities {
                for (term, tf) in &chunk.counts[from..*end as usize] {
                    let at = &mut next[ranks[*term as usize] as usize];
                    filed[*at] = (*id, *tf);
                    *at += 1;
                }
                from = *end as usize;
            }
        }
        drop(next);

        // Each term's posting list: appends, in id order.
        let lists: Vec<CowIdMap<u32>> = par_map_range(terms.len(), |rank| {
            let mut list = CowIdMap::default();
            for (id, tf) in &filed[starts[rank]..starts[rank + 1]] {
                list.insert(*id, *tf);
            }
            list
        });
        drop(filed);

        // Each entity's terms, in term order as `reindex_entity` files
        // them, sharing the posting lists' keys.
        let held: Vec<Vec<(u64, EntityTerms)>> = par_map_range(chunks.len(), |at| {
            let (chunk, ranks) = (&chunks[at], &ranks[at]);
            let mut out = Vec::with_capacity(chunk.entities.len());
            let mut mine: Vec<u32> = Vec::new();
            let mut from = 0;
            for (id, end) in &chunk.entities {
                mine.clear();
                mine.extend(
                    chunk.counts[from..*end as usize]
                        .iter()
                        .map(|(term, _)| ranks[*term as usize]),
                );
                mine.sort_unstable();
                let held: EntityTerms = mine
                    .iter()
                    .map(|rank| terms[*rank as usize].clone())
                    .collect();
                out.push((*id, held));
                from = *end as usize;
            }
            out
        });
        index.entity_terms = CowMap::with_shape(
            held.iter()
                .flatten()
                .map(|(id, _)| super::cow::route(id))
                .collect(),
        );
        for (id, terms) in held.into_iter().flatten() {
            index.entity_terms.insert(id, terms);
        }
        index.postings = CowOrdMap::from_sorted(terms.into_iter().zip(lists));
        index
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
            // Share the posting list's key: a fresh `Arc` per entity would
            // keep another copy of the term for every entity holding it.
            let term: Arc<str> = match self.postings.get_key_value(term.as_str()) {
                Some((stored, _)) => stored.clone(),
                None => Arc::from(term),
            };
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
    ///
    /// With `limit`, only the best `limit` matches are ranked and
    /// returned: the rest are never sorted or handed to the caller, so a
    /// broad prefix (`a*` over millions of titles) costs one compact
    /// `(id, tf)` pair per match, not a row each.
    pub(super) fn query(&self, query_text: &str, limit: Option<usize>) -> Vec<(u64, f64)> {
        let tokens = analyze(query_text, true);
        if tokens.is_empty() || limit == Some(0) {
            return Vec::new();
        }
        // Resolve each query term to a posting list. A prefix term merges
        // the postings of every indexed term starting with it.
        let mut merged: Vec<Vec<(u64, u32)>> = Vec::new();
        let mut exact: Vec<&CowIdMap<u32>> = Vec::with_capacity(tokens.len());
        for (token, prefix) in &tokens {
            if *prefix {
                // Posting lists iterate in no particular order: gather,
                // sort and sum runs into one id-ordered list.
                let prefix = token.clone();
                let mut hits: Vec<(u64, u32)> = Vec::new();
                for (_, posting) in self
                    .postings
                    .range(
                        std::ops::Bound::Included(Arc::from(token.as_str())),
                        std::ops::Bound::Unbounded,
                    )
                    .take_while(|(term, _)| term.starts_with(prefix.as_str()))
                {
                    hits.extend(posting.iter().map(|(id, tf)| (*id, *tf)));
                }
                if hits.is_empty() {
                    return Vec::new();
                }
                hits.sort_unstable_by_key(|(id, _)| *id);
                hits.dedup_by(|(id, tf), (kept_id, kept_tf)| {
                    let same = id == kept_id;
                    if same {
                        *kept_tf = kept_tf.saturating_add(*tf);
                    }
                    same
                });
                hits.shrink_to_fit();
                merged.push(hits);
                continue;
            }
            match self.postings.get(token.as_str()) {
                Some(p) => exact.push(p),
                None => return Vec::new(), // term not present → AND fails
            }
        }
        // Find the smallest posting list to seed the intersection.
        let mut posting_iter: Vec<Posting<'_>> = exact.into_iter().map(Posting::Index).collect();
        posting_iter.extend(merged.iter().map(|m| Posting::Merged(m)));
        posting_iter.sort_by_key(|p| p.len());

        let Some(seed) = posting_iter.first() else {
            return Vec::new();
        };
        let mut results: Vec<(u64, u32)> = seed.iter().collect();
        // Intersect with the rest, summing TF as we go.
        for posting in posting_iter.iter().skip(1) {
            results.retain_mut(|(id, acc)| match posting.get(id) {
                Some(tf) => {
                    *acc = acc.saturating_add(*tf);
                    true
                }
                None => false,
            });
            if results.is_empty() {
                return Vec::new();
            }
        }
        // Descending score, ascending id for ties.
        let order = |a: &(u64, u32), b: &(u64, u32)| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0));
        if let Some(k) = limit {
            if k < results.len() {
                results.select_nth_unstable_by(k - 1, order);
                results.truncate(k);
            }
        }
        results.sort_unstable_by(order);
        results
            .into_iter()
            .map(|(id, tf)| (id, tf as f64))
            .collect()
    }
}

/// A query term's posting list: straight from the index, or the
/// id-ordered union built for a prefix term.
enum Posting<'a> {
    Index(&'a CowIdMap<u32>),
    Merged(&'a [(u64, u32)]),
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
            Posting::Merged(p) => p
                .binary_search_by_key(id, |(entry, _)| *entry)
                .ok()
                .map(|at| &p[at].1),
        }
    }

    fn iter(&self) -> Box<dyn Iterator<Item = (u64, u32)> + '_> {
        match self {
            Posting::Index(p) => Box::new(p.iter().map(|(id, tf)| (*id, *tf))),
            Posting::Merged(p) => Box::new(p.iter().copied()),
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

pub(super) fn string_property_term_counts<'a>(
    properties: impl Into<crate::PropsRef<'a>>,
) -> PropertyTermCounts {
    let mut out = PropertyTermCounts::new();
    for (key, value) in properties.into() {
        if let Some(counts) = value_term_counts(value) {
            // A string with no token still has an (empty) entry.
            out.insert(key.to_string(), counts);
        }
    }
    out
}

/// The text terms of one property value: a string's, or those of a
/// list's strings (like Lucene's multi-valued fields; other elements are
/// skipped). Numbers, booleans, temporals, points and vectors are not
/// text.
fn value_term_counts(value: crate::ValueRef<'_>) -> Option<TermCounts> {
    match value {
        crate::ValueRef::String(value) => Some(tokenize_to_term_counts(value)),
        crate::ValueRef::Other(other) => {
            let crate::PropertyValue::List(items) = &*other.get() else {
                return None;
            };
            let mut counts = TermCounts::new();
            for item in items {
                if let crate::PropertyValue::String(text) = item {
                    merge_term_counts(&mut counts, tokenize_to_term_counts(text));
                }
            }
            (!counts.is_empty()).then_some(counts)
        }
        _ => None,
    }
}

/// A run of entities tokenised for [`FulltextIndex::from_chunks`]: the
/// distinct terms met, numbered in that order, and each entity's
/// `(term number, frequency)` pairs.
#[derive(Default)]
pub(super) struct TokenisedChunk {
    terms: Vec<String>,
    /// Each entity with a term, and where its pairs end in `counts`.
    entities: Vec<(u64, u32)>,
    counts: Vec<(u32, u32)>,
}

/// Tokenises entities into a [`TokenisedChunk`]. A term is spelled once
/// per chunk, not once per occurrence.
#[derive(Default)]
pub(super) struct ChunkTokeniser {
    chunk: TokenisedChunk,
    numbers: HashMap<String, u32>,
    /// The current entity's term numbers, one per token.
    tokens: Vec<u32>,
    buf: String,
}

impl ChunkTokeniser {
    /// Add the covered text of the current entity: its `selected`
    /// properties' strings, and the strings of their lists (see
    /// [`string_property_term_counts`]).
    pub(super) fn add_selected<'a>(
        &mut self,
        properties: impl Into<crate::PropsRef<'a>>,
        selected: &[String],
    ) {
        let properties = properties.into();
        for property in selected {
            match properties.get(property) {
                Some(crate::ValueRef::String(text)) => self.add_text(text),
                Some(crate::ValueRef::Other(other)) => {
                    if let crate::PropertyValue::List(items) = &*other.get() {
                        for item in items {
                            if let crate::PropertyValue::String(text) = item {
                                self.add_text(text);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn add_text(&mut self, text: &str) {
        let Self {
            chunk,
            numbers,
            tokens,
            buf,
        } = self;
        for_each_token(text, false, buf, |token, _| {
            let number = match numbers.get(token) {
                Some(number) => *number,
                None => {
                    let number = chunk.terms.len() as u32;
                    chunk.terms.push(token.to_owned());
                    numbers.insert(token.to_owned(), number);
                    number
                }
            };
            tokens.push(number);
        });
    }

    /// Close the current entity as `id`. One without a token is left out,
    /// as an entity without terms is of the index.
    pub(super) fn end_entity(&mut self, id: u64) {
        if self.tokens.is_empty() {
            return;
        }
        self.tokens.sort_unstable();
        let mut at = 0;
        while at < self.tokens.len() {
            let term = self.tokens[at];
            let run = self.tokens[at..].partition_point(|other| *other == term);
            self.chunk.counts.push((term, run as u32));
            at += run;
        }
        self.tokens.clear();
        self.chunk
            .entities
            .push((id, self.chunk.counts.len() as u32));
    }

    pub(super) fn finish(self) -> TokenisedChunk {
        self.chunk
    }
}

/// `items` sorted by `compare`, on every core when the build allows.
fn par_sort_unstable_by<T: Send>(
    items: &mut [T],
    compare: impl Fn(&T, &T) -> std::cmp::Ordering + Sync,
) {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::prelude::*;
        items.par_sort_unstable_by(compare);
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    items.sort_unstable_by(compare);
}

/// `make(0), make(1), .. make(len - 1)`, on every core when the build
/// allows.
fn par_map_range<T: Send>(len: usize, make: impl Fn(usize) -> T + Sync) -> Vec<T> {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::prelude::*;
        (0..len).into_par_iter().map(&make).collect()
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    (0..len).map(make).collect()
}

pub(super) fn term_counts_for_properties<'a>(
    properties: impl Into<crate::PropsRef<'a>>,
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
        let r = idx.query("foo", None);
        assert!(r.is_empty(), "expected empty after reindex, got {r:?}");
        let r = idx.query("baz", None);
        assert_eq!(r, vec![(1, 1.0)]);
    }

    /// Entities holding the same term share the posting list's key
    /// instead of each keeping a copy.
    #[test]
    fn entities_share_the_stored_term() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("alpha beta"));
        idx.reindex_entity(2, tokenize_to_term_counts("beta alpha"));
        let (stored, _) = idx.postings.get_key_value("alpha").expect("indexed");
        for id in [1, 2] {
            let terms = idx.entity_terms.get(&id).expect("indexed");
            let term = terms.iter().find(|t| &***t == "alpha").expect("term");
            assert!(Arc::ptr_eq(term, stored), "entity {id}");
        }
    }

    #[test]
    fn query_intersects_terms() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("alpha beta gamma"));
        idx.reindex_entity(2, tokenize_to_term_counts("alpha gamma delta"));
        let r = idx.query("alpha beta", None);
        assert_eq!(r, vec![(1, 2.0)], "only entity 1 has both terms");
    }

    #[test]
    fn query_returns_empty_for_unknown_term() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("alpha"));
        assert!(idx.query("zeta", None).is_empty());
    }

    #[test]
    fn query_limit_keeps_the_best_in_order() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(5, tokenize_to_term_counts("night"));
        idx.reindex_entity(3, tokenize_to_term_counts("night nights"));
        idx.reindex_entity(9, tokenize_to_term_counts("nightfall"));
        idx.reindex_entity(1, tokenize_to_term_counts("nightly"));
        idx.reindex_entity(7, tokenize_to_term_counts("day"));
        let all = idx.query("nigh*", None);
        assert_eq!(all, vec![(3, 2.0), (1, 1.0), (5, 1.0), (9, 1.0)]);
        for k in 1..=5 {
            let top = idx.query("nigh*", Some(k));
            assert_eq!(top, all[..k.min(all.len())], "limit {k}");
        }
        assert!(idx.query("nigh*", Some(0)).is_empty());
    }

    #[test]
    fn query_intersects_a_prefix_with_an_exact_term() {
        let mut idx = FulltextIndex::new(vec!["L".into()], vec!["p".into()]);
        idx.reindex_entity(1, tokenize_to_term_counts("love song"));
        idx.reindex_entity(2, tokenize_to_term_counts("lovely song"));
        idx.reindex_entity(3, tokenize_to_term_counts("love story"));
        assert_eq!(idx.query("lov* song", None), vec![(1, 2.0), (2, 2.0)]);
        assert_eq!(idx.query("song lov*", Some(1)), vec![(1, 2.0)]);
    }
}
