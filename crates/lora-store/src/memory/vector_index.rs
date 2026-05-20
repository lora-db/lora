//! VECTOR index storage and query backend.
//!
//! Phase 1 of the vector-indexing extension: introduce a backend
//! abstraction so the procedure layer no longer scans the property
//! store directly. Today only the `Flat` brute-force backend is
//! implemented; the [`VectorBackend`] enum exists so Phase 2 can drop
//! in an HNSW arm without disturbing the registry shape or the
//! maintenance hooks.
//!
//! Unlike the TEXT / POINT / SORTED registries — which key by
//! `(label, property)` because the underlying structure is shared
//! across catalog entries with the same scope — vector indexes are
//! keyed by **index name**. Two vector indexes on the same
//! `(label, property)` can coexist with different similarity functions
//! (e.g. one cosine, one euclidean), each owning its own backend.

use std::collections::BTreeMap;

use crate::{cosine_similarity_bounded, euclidean_similarity, LoraVector};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorSimilarity {
    Cosine,
    Euclidean,
}

impl VectorSimilarity {
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("cosine") {
            Some(VectorSimilarity::Cosine)
        } else if s.eq_ignore_ascii_case("euclidean") {
            Some(VectorSimilarity::Euclidean)
        } else {
            None
        }
    }

    pub fn score(self, a: &LoraVector, b: &LoraVector) -> Option<f64> {
        if a.dimension != b.dimension {
            return None;
        }
        match self {
            VectorSimilarity::Cosine => cosine_similarity_bounded(a, b),
            VectorSimilarity::Euclidean => euclidean_similarity(a, b),
        }
    }
}

/// Brute-force backend: store every vector, score them all per query.
/// `BTreeMap` keying gives deterministic iteration order, which keeps
/// score-tie ordering stable across runs (matches the legacy
/// `score_entities` contract).
#[derive(Debug, Default, Clone)]
pub(super) struct FlatBackend {
    items: BTreeMap<u64, LoraVector>,
}

impl FlatBackend {
    fn insert(&mut self, id: u64, vector: LoraVector) {
        self.items.insert(id, vector);
    }

    fn remove(&mut self, id: u64) {
        self.items.remove(&id);
    }

    fn query(&self, query: &LoraVector, similarity: VectorSimilarity) -> Vec<(u64, f64)> {
        let mut out = Vec::with_capacity(self.items.len());
        for (&id, v) in &self.items {
            if let Some(score) = similarity.score(v, query) {
                out.push((id, score));
            }
        }
        out
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.items.len()
    }
}

/// Backend dispatch. Phase 2 adds an `Hnsw(HnswBackend)` arm here and
/// updates the per-method `match` to route through it; the registry,
/// graph plumbing, and procedure layer don't change.
#[derive(Debug, Clone)]
pub(super) enum VectorBackend {
    Flat(FlatBackend),
}

impl VectorBackend {
    fn insert(&mut self, id: u64, vector: LoraVector) {
        match self {
            VectorBackend::Flat(b) => b.insert(id, vector),
        }
    }

    fn remove(&mut self, id: u64) {
        match self {
            VectorBackend::Flat(b) => b.remove(id),
        }
    }

    fn query(&self, query: &LoraVector, similarity: VectorSimilarity) -> Vec<(u64, f64)> {
        match self {
            VectorBackend::Flat(b) => b.query(query, similarity),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        match self {
            VectorBackend::Flat(b) => b.len(),
        }
    }
}

/// One installed VECTOR index, plus the resolved metadata the
/// maintenance hook needs to decide whether a given property change
/// applies (without having to re-read the catalog).
#[derive(Debug, Clone)]
pub(super) struct VectorIndexEntry {
    pub label: String,
    pub property: String,
    pub similarity: VectorSimilarity,
    pub backend: VectorBackend,
}

/// Per-entity-kind registry of vector indexes. Keyed by index name.
#[derive(Debug, Default, Clone)]
pub(super) struct VectorIndexRegistry {
    by_name: BTreeMap<String, VectorIndexEntry>,
}

impl VectorIndexRegistry {
    pub(super) fn register(
        &mut self,
        name: String,
        label: String,
        property: String,
        similarity: VectorSimilarity,
    ) {
        self.by_name.insert(
            name,
            VectorIndexEntry {
                label,
                property,
                similarity,
                backend: VectorBackend::Flat(FlatBackend::default()),
            },
        );
    }

    pub(super) fn deregister(&mut self, name: &str) {
        self.by_name.remove(name);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Insert `vector` for `entity_id` into every index whose
    /// `(label, property)` matches. Used by both the initial backfill
    /// from `activate_vector_index` and per-mutation maintenance.
    pub(super) fn insert_for(
        &mut self,
        label: &str,
        property: &str,
        entity_id: u64,
        vector: &LoraVector,
    ) {
        for entry in self.by_name.values_mut() {
            if entry.label == label && entry.property == property {
                entry.backend.insert(entity_id, vector.clone());
            }
        }
    }

    /// Drop `entity_id` from every index whose `(label, property)`
    /// matches.
    pub(super) fn remove_for(&mut self, label: &str, property: &str, entity_id: u64) {
        for entry in self.by_name.values_mut() {
            if entry.label == label && entry.property == property {
                entry.backend.remove(entity_id);
            }
        }
    }

    /// Run a top-N scan against a named index. Returns the unsorted
    /// (id, score) pairs — the caller applies `sort_by_desc(score)
    /// then asc(id)` + truncate, matching the legacy `scored_rows`
    /// contract.
    pub(super) fn query(&self, name: &str, query: &LoraVector) -> Option<Vec<(u64, f64)>> {
        let entry = self.by_name.get(name)?;
        Some(entry.backend.query(query, entry.similarity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RawCoordinate, VectorCoordinateType};

    fn vec(values: &[f32]) -> LoraVector {
        let coords: Vec<RawCoordinate> = values
            .iter()
            .map(|v| RawCoordinate::Float(*v as f64))
            .collect();
        LoraVector::try_new(coords, values.len() as i64, VectorCoordinateType::Float32).unwrap()
    }

    #[test]
    fn register_and_query_returns_scores() {
        let mut reg = VectorIndexRegistry::default();
        reg.register(
            "vidx".into(),
            "V".into(),
            "e".into(),
            VectorSimilarity::Cosine,
        );
        reg.insert_for("V", "e", 1, &vec(&[1.0, 0.0, 0.0]));
        reg.insert_for("V", "e", 2, &vec(&[0.0, 1.0, 0.0]));
        let scored = reg.query("vidx", &vec(&[1.0, 0.0, 0.0])).unwrap();
        // Two entries; entity 1 (identical to query) scores 1.0.
        assert_eq!(scored.len(), 2);
        let by_id: BTreeMap<u64, f64> = scored.into_iter().collect();
        assert!((by_id[&1] - 1.0).abs() < 1e-9);
        assert!(by_id[&2] < by_id[&1]);
    }

    #[test]
    fn remove_drops_from_backend() {
        let mut reg = VectorIndexRegistry::default();
        reg.register(
            "vidx".into(),
            "V".into(),
            "e".into(),
            VectorSimilarity::Cosine,
        );
        reg.insert_for("V", "e", 1, &vec(&[1.0, 0.0]));
        reg.insert_for("V", "e", 2, &vec(&[0.0, 1.0]));
        reg.remove_for("V", "e", 1);
        let scored = reg.query("vidx", &vec(&[1.0, 0.0])).unwrap();
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, 2);
    }

    #[test]
    fn unrelated_scope_is_skipped() {
        let mut reg = VectorIndexRegistry::default();
        reg.register(
            "movie_emb".into(),
            "Movie".into(),
            "embedding".into(),
            VectorSimilarity::Cosine,
        );
        // Wrong label — must not be picked up.
        reg.insert_for("Other", "embedding", 99, &vec(&[1.0, 0.0]));
        let scored = reg.query("movie_emb", &vec(&[1.0, 0.0])).unwrap();
        assert!(scored.is_empty());
    }

    #[test]
    fn two_indexes_on_same_scope_with_different_metrics() {
        let mut reg = VectorIndexRegistry::default();
        reg.register(
            "by_cos".into(),
            "V".into(),
            "e".into(),
            VectorSimilarity::Cosine,
        );
        reg.register(
            "by_euc".into(),
            "V".into(),
            "e".into(),
            VectorSimilarity::Euclidean,
        );
        reg.insert_for("V", "e", 1, &vec(&[1.0, 0.0]));
        reg.insert_for("V", "e", 2, &vec(&[0.0, 1.0]));
        let cos = reg.query("by_cos", &vec(&[1.0, 0.0])).unwrap();
        let euc = reg.query("by_euc", &vec(&[1.0, 0.0])).unwrap();
        assert_eq!(cos.len(), 2);
        assert_eq!(euc.len(), 2);
        // Distinct metrics → distinct second backends populated.
        for entry in reg.by_name.values() {
            assert_eq!(entry.backend.len(), 2);
        }
    }

}
