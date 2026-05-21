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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    cosine_similarity_bounded, dot_product, euclidean_similarity, manhattan_distance, LoraVector,
};

use super::hnsw::{seed_from_name, HnswBackend, HnswParams, HnswSnapshot};
use super::index_catalog::{IndexConfigValue, StoredIndexEntity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorSimilarity {
    Cosine,
    Euclidean,
    /// Raw dot product. Higher is more similar; unbounded above and
    /// below. The right choice for embeddings already L2-normalized
    /// (cosine reduces to dot in that case, and dot skips one
    /// reciprocal-sqrt per pair).
    Dot,
    /// L1-derived: `1 / (1 + d_L1)`. Same higher-is-better shape as
    /// `Euclidean`; useful for quantized vectors where L1 is the
    /// natural metric.
    Manhattan,
}

impl VectorSimilarity {
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("cosine") {
            Some(VectorSimilarity::Cosine)
        } else if s.eq_ignore_ascii_case("euclidean") {
            Some(VectorSimilarity::Euclidean)
        } else if s.eq_ignore_ascii_case("dot") || s.eq_ignore_ascii_case("dot_product") {
            Some(VectorSimilarity::Dot)
        } else if s.eq_ignore_ascii_case("manhattan") {
            Some(VectorSimilarity::Manhattan)
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
            VectorSimilarity::Dot => dot_product(a, b),
            VectorSimilarity::Manhattan => manhattan_distance(a, b).map(|d| 1.0 / (1.0 + d)),
        }
    }

    /// Resolve `vector.similarity_function` from a catalog `OPTIONS`
    /// map. Returns `None` when the key is missing or unrecognised;
    /// DDL validation has already rejected invalid values, so a
    /// `None` here only occurs on a malformed snapshot/WAL payload —
    /// the caller picks a default in that case.
    pub(super) fn from_options(options: &BTreeMap<String, IndexConfigValue>) -> Option<Self> {
        match options.get("vector.similarity_function")? {
            IndexConfigValue::String(s) => Self::parse(s),
            _ => None,
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

    fn query(
        &self,
        query: &LoraVector,
        similarity: VectorSimilarity,
        restrict_to: Option<&BTreeSet<u64>>,
    ) -> Vec<(u64, f64)> {
        let mut out = Vec::with_capacity(self.items.len());
        for (&id, v) in &self.items {
            if let Some(set) = restrict_to {
                if !set.contains(&id) {
                    continue;
                }
            }
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

/// Selector for which backend powers a given index. Surfaced via the
/// `vector.indexProvider` index option; defaults to `Flat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorIndexProvider {
    Flat,
    Hnsw,
}

impl VectorIndexProvider {
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("flat") {
            Some(VectorIndexProvider::Flat)
        } else if s.eq_ignore_ascii_case("hnsw") {
            Some(VectorIndexProvider::Hnsw)
        } else {
            None
        }
    }

    /// Resolve `vector.indexProvider` from a catalog `OPTIONS` map.
    /// `'flat'` and `'hnsw'` are accepted; anything else returns
    /// `None` and the caller falls back to the safe default.
    pub(super) fn from_options(options: &BTreeMap<String, IndexConfigValue>) -> Option<Self> {
        match options.get("vector.indexProvider")? {
            IndexConfigValue::String(s) => Self::parse(s),
            _ => None,
        }
    }
}

/// Backend dispatch. The Hnsw arm owns its own similarity (it
/// internalizes scoring during graph construction); the Flat arm
/// takes similarity per-query because it has no precomputed work to
/// pin to a single metric.
#[derive(Debug, Clone)]
pub(super) enum VectorBackend {
    Flat(FlatBackend),
    Hnsw(HnswBackend),
}

impl VectorBackend {
    fn insert(&mut self, id: u64, vector: LoraVector) {
        match self {
            VectorBackend::Flat(b) => b.insert(id, vector),
            VectorBackend::Hnsw(b) => b.insert(id, vector),
        }
    }

    fn remove(&mut self, id: u64) {
        match self {
            VectorBackend::Flat(b) => b.remove(id),
            VectorBackend::Hnsw(b) => b.remove(id),
        }
    }

    /// `similarity` and `k` are only honored by some arms:
    /// - Flat: scores every point with `similarity`, returns all
    ///   matching (id, score). The caller sorts + truncates to k.
    /// - Hnsw: ignores `similarity` (configured at construction),
    ///   uses `k` to size the result set inside the graph walk.
    ///
    /// `restrict_to` is a hard filter: only ids in the set may
    /// appear in the result. HNSW still traverses through other
    /// nodes for routing — recall against a very selective filter
    /// degrades; callers facing tight filters should raise
    /// `vector.hnsw.ef_search`.
    fn query(
        &self,
        query: &LoraVector,
        similarity: VectorSimilarity,
        k: usize,
        restrict_to: Option<&BTreeSet<u64>>,
    ) -> Vec<(u64, f64)> {
        match self {
            VectorBackend::Flat(b) => b.query(query, similarity, restrict_to),
            VectorBackend::Hnsw(b) => b.query(query, k, restrict_to),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        match self {
            VectorBackend::Flat(b) => b.len(),
            VectorBackend::Hnsw(b) => b.len(),
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
        provider: VectorIndexProvider,
        hnsw: HnswParams,
    ) {
        let backend = match provider {
            VectorIndexProvider::Flat => VectorBackend::Flat(FlatBackend::default()),
            VectorIndexProvider::Hnsw => {
                let seed = seed_from_name(&name);
                VectorBackend::Hnsw(HnswBackend::new(similarity, hnsw, seed))
            }
        };
        self.by_name.insert(
            name,
            VectorIndexEntry {
                label,
                property,
                similarity,
                backend,
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

    /// Run a top-k scan against a named index, optionally
    /// restricting results to the given id set. Returns `(id,
    /// score)` pairs from the backend; the caller applies the
    /// canonical `sort_by_desc(score) then asc(id)` + truncate
    /// post-step that matches the legacy `scored_rows` contract.
    /// The flat arm returns all matching entities; the HNSW arm
    /// caps at k internally.
    pub(super) fn query(
        &self,
        name: &str,
        query: &LoraVector,
        k: usize,
        restrict_to: Option<&BTreeSet<u64>>,
    ) -> Option<Vec<(u64, f64)>> {
        let entry = self.by_name.get(name)?;
        Some(entry.backend.query(query, entry.similarity, k, restrict_to))
    }

    /// Capture a serializable snapshot of every HNSW backend in this
    /// registry. Flat backends are skipped because their state is
    /// reconstructible from the property store at zero cost — only
    /// HNSW pays the O(n log n) rebuild penalty that justifies
    /// shipping graph topology through the snapshot pipeline.
    pub(super) fn to_snapshots(&self, entity: StoredIndexEntity) -> Vec<VectorIndexSnapshot> {
        let mut out = Vec::new();
        for (name, entry) in &self.by_name {
            if let VectorBackend::Hnsw(b) = &entry.backend {
                out.push(VectorIndexSnapshot {
                    name: name.clone(),
                    entity,
                    label: entry.label.clone(),
                    property: entry.property.clone(),
                    data: VectorBackendSnapshot::Hnsw(b.to_snapshot(entry.similarity)),
                });
            }
        }
        out
    }

    /// Replace the backend for `snapshot.name` with one rebuilt from
    /// the snapshot data. No-op if the index isn't registered, the
    /// snapshot kind doesn't match, or the scope (label/property)
    /// diverges — all signals that the catalog and the snapshot are
    /// out of step, in which case we fall back to the property-store
    /// backfill.
    pub(super) fn restore_snapshot(&mut self, snapshot: VectorIndexSnapshot) -> bool {
        let Some(entry) = self.by_name.get_mut(&snapshot.name) else {
            return false;
        };
        if entry.label != snapshot.label || entry.property != snapshot.property {
            return false;
        }
        match snapshot.data {
            VectorBackendSnapshot::Hnsw(snap) => {
                if !matches!(entry.backend, VectorBackend::Hnsw(_)) {
                    return false;
                }
                entry.similarity = snap.similarity;
                entry.backend = VectorBackend::Hnsw(HnswBackend::from_snapshot(snap));
                true
            }
        }
    }
}

/// Snapshot of one vector index, carried through the snapshot
/// pipeline. Only HNSW backends are persisted today (see
/// [`VectorIndexRegistry::to_snapshots`]); the enum is open for a
/// future Flat arm if pre-built flat backends become expensive to
/// rebuild for some workload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VectorIndexSnapshot {
    pub name: String,
    pub entity: StoredIndexEntity,
    pub label: String,
    pub property: String,
    pub data: VectorBackendSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VectorBackendSnapshot {
    Hnsw(HnswSnapshot),
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

    fn register_flat(
        reg: &mut VectorIndexRegistry,
        name: &str,
        label: &str,
        prop: &str,
        sim: VectorSimilarity,
    ) {
        reg.register(
            name.into(),
            label.into(),
            prop.into(),
            sim,
            VectorIndexProvider::Flat,
            HnswParams::default(),
        );
    }

    #[test]
    fn register_and_query_returns_scores() {
        let mut reg = VectorIndexRegistry::default();
        register_flat(&mut reg, "vidx", "V", "e", VectorSimilarity::Cosine);
        reg.insert_for("V", "e", 1, &vec(&[1.0, 0.0, 0.0]));
        reg.insert_for("V", "e", 2, &vec(&[0.0, 1.0, 0.0]));
        let scored = reg.query("vidx", &vec(&[1.0, 0.0, 0.0]), 10, None).unwrap();
        // Two entries; entity 1 (identical to query) scores 1.0.
        assert_eq!(scored.len(), 2);
        let by_id: BTreeMap<u64, f64> = scored.into_iter().collect();
        assert!((by_id[&1] - 1.0).abs() < 1e-9);
        assert!(by_id[&2] < by_id[&1]);
    }

    #[test]
    fn remove_drops_from_backend() {
        let mut reg = VectorIndexRegistry::default();
        register_flat(&mut reg, "vidx", "V", "e", VectorSimilarity::Cosine);
        reg.insert_for("V", "e", 1, &vec(&[1.0, 0.0]));
        reg.insert_for("V", "e", 2, &vec(&[0.0, 1.0]));
        reg.remove_for("V", "e", 1);
        let scored = reg.query("vidx", &vec(&[1.0, 0.0]), 10, None).unwrap();
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].0, 2);
    }

    #[test]
    fn unrelated_scope_is_skipped() {
        let mut reg = VectorIndexRegistry::default();
        register_flat(
            &mut reg,
            "movie_emb",
            "Movie",
            "embedding",
            VectorSimilarity::Cosine,
        );
        // Wrong label — must not be picked up.
        reg.insert_for("Other", "embedding", 99, &vec(&[1.0, 0.0]));
        let scored = reg.query("movie_emb", &vec(&[1.0, 0.0]), 10, None).unwrap();
        assert!(scored.is_empty());
    }

    #[test]
    fn two_indexes_on_same_scope_with_different_metrics() {
        let mut reg = VectorIndexRegistry::default();
        register_flat(&mut reg, "by_cos", "V", "e", VectorSimilarity::Cosine);
        register_flat(&mut reg, "by_euc", "V", "e", VectorSimilarity::Euclidean);
        reg.insert_for("V", "e", 1, &vec(&[1.0, 0.0]));
        reg.insert_for("V", "e", 2, &vec(&[0.0, 1.0]));
        let cos = reg.query("by_cos", &vec(&[1.0, 0.0]), 10, None).unwrap();
        let euc = reg.query("by_euc", &vec(&[1.0, 0.0]), 10, None).unwrap();
        assert_eq!(cos.len(), 2);
        assert_eq!(euc.len(), 2);
        // Distinct metrics → distinct second backends populated.
        for entry in reg.by_name.values() {
            assert_eq!(entry.backend.len(), 2);
        }
    }

    #[test]
    fn hnsw_provider_returns_top_k() {
        let mut reg = VectorIndexRegistry::default();
        reg.register(
            "vh".into(),
            "V".into(),
            "e".into(),
            VectorSimilarity::Cosine,
            VectorIndexProvider::Hnsw,
            HnswParams::default(),
        );
        for i in 0..50u64 {
            let v = vec(&[(i as f32) / 50.0, 1.0 - (i as f32) / 50.0]);
            reg.insert_for("V", "e", i, &v);
        }
        let hits = reg.query("vh", &vec(&[1.0, 0.0]), 5, None).unwrap();
        assert_eq!(hits.len(), 5);
        // Closest two should be the high-i (≈[1, 0]) end of the line.
        let ids: Vec<u64> = hits.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&49) || ids.contains(&48), "got {ids:?}");
    }
}
