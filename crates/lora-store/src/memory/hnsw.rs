//! Hand-rolled HNSW (Hierarchical Navigable Small World) backend for
//! vector indexes. Implements Malkov & Yashunin (2018), with the
//! simple closest-M neighbor selection rather than the heuristic
//! variant (Algorithm 4) — empirically still delivers recall@10 ≥
//! 0.95 on uniform data at the defaults below.
//!
//! ## Determinism
//!
//! HNSW's per-insert layer assignment is randomized. We feed a
//! per-index LCG from the index name so two backends with the same
//! name produce the same graph topology for the same insert sequence.
//! This matters for snapshot replay: the backend is not persisted
//! today (Phase 4); it is rebuilt from scratch via the property-store
//! backfill in `activate_vector_index`. With a name-derived seed,
//! that rebuild yields the same internal structure as the original
//! session, which keeps query results stable across restarts.
//!
//! ## Scoring inversion
//!
//! HNSW's literature talks about distance (lower = closer). LoraDB's
//! [`VectorSimilarity`] returns similarity (higher = better). We
//! treat `dist = -similarity` internally — same ordering, no
//! per-metric conversion gymnastics, and the public surface still
//! returns (id, similarity) pairs.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashSet};

use serde::{Deserialize, Serialize};

use crate::types::vector::RawCoordinate;
use crate::{LoraVector, VectorCoordinateType};

use super::index_catalog::IndexConfigValue;
use super::vector_index::VectorSimilarity;

/// Persistable snapshot of a single HNSW backend. Captured at
/// snapshot time and restored on load to skip the O(n log n) rebuild
/// cost. Serde-derived so future format changes can use
/// `#[serde(default)]` for backwards compatibility within a major
/// snapshot format version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HnswSnapshot {
    pub similarity: VectorSimilarity,
    pub params: HnswParams,
    pub entry_point: Option<u64>,
    pub max_level: usize,
    pub rng_state: u64,
    pub nodes: Vec<HnswNodeSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HnswNodeSnapshot {
    pub id: u64,
    pub level: usize,
    pub vector: LoraVector,
    pub neighbors: Vec<Vec<u64>>,
}

/// On-disk / in-memory representation choice for stored vectors.
/// `None` keeps the input vector verbatim (FLOAT32 typically).
/// `Int8` scales each coordinate by 127 and stores as INTEGER8,
/// reducing memory ~4× at the cost of quantization error.
///
/// Today only cosine similarity is allowed with `Int8` because
/// cosine is scale-invariant — scaling every coordinate by the same
/// constant leaves the similarity unchanged. Other metrics (e.g.
/// euclidean) would return scores in a degenerate range, so the
/// schema validator rejects the combination at DDL time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HnswQuantization {
    None,
    Int8,
}

impl HnswQuantization {
    pub fn parse(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("none") {
            Some(HnswQuantization::None)
        } else if s.eq_ignore_ascii_case("int8") {
            Some(HnswQuantization::Int8)
        } else {
            None
        }
    }
}

/// User-tunable knobs. Defaults mirror Neo4j's defaults so existing
/// embeddings configs port over without surprise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HnswParams {
    pub m: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
    pub quantization: HnswQuantization,
}

impl Default for HnswParams {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 200,
            ef_search: 100,
            quantization: HnswQuantization::None,
        }
    }
}

impl HnswParams {
    /// Extract HNSW knobs from a catalog `OPTIONS` map, falling back
    /// to the algorithm defaults for any missing key. The schema
    /// validator has already rejected out-of-range values; a corrupted
    /// snapshot is the only path to a non-positive integer here, which
    /// we silently treat as "use the default" rather than panic.
    pub(super) fn from_options(
        options: &std::collections::BTreeMap<String, IndexConfigValue>,
    ) -> Self {
        let mut params = Self::default();
        if let Some(IndexConfigValue::Integer(v)) = options.get("vector.hnsw.m") {
            if *v > 0 {
                params.m = *v as usize;
            }
        }
        if let Some(IndexConfigValue::Integer(v)) = options.get("vector.hnsw.ef_construction") {
            if *v > 0 {
                params.ef_construction = *v as usize;
            }
        }
        if let Some(IndexConfigValue::Integer(v)) = options.get("vector.hnsw.ef_search") {
            if *v > 0 {
                params.ef_search = *v as usize;
            }
        }
        if let Some(IndexConfigValue::String(s)) = options.get("vector.hnsw.quantization") {
            if let Some(q) = HnswQuantization::parse(s) {
                params.quantization = q;
            }
        }
        params
    }
}

/// Quantize an input vector to int8 by scaling each f32 coordinate
/// by 127 and clamping to `[-128, 127]`. Designed for unit-norm
/// embeddings; out-of-range values clip rather than panic. Used for
/// both stored vectors at insert time and the query vector at
/// search time so distances stay comparable.
fn quantize_to_int8(input: &LoraVector) -> LoraVector {
    let dim = input.dimension;
    let coords: Vec<RawCoordinate> = (0..dim)
        .map(|i| {
            let f = input.values.f32_at(i).unwrap_or(0.0);
            let scaled = (f * 127.0).round().clamp(-128.0, 127.0) as i64;
            RawCoordinate::Int(scaled)
        })
        .collect();
    LoraVector::try_new(coords, dim as i64, VectorCoordinateType::Integer8)
        .expect("quantized vector must validate")
}

#[derive(Debug, Clone)]
struct HnswNode {
    vector: LoraVector,
    level: usize,
    /// One neighbor list per layer 0..=level. Each is a `Vec<u64>` so
    /// graph traversal stays cache-friendly; the list length is
    /// bounded by `m_max(layer)` (2·m at layer 0, m above).
    neighbors: Vec<Vec<u64>>,
}

/// Wrapper that turns f64 into a totally-ordered type for heap use.
/// Similarity functions only return finite values (they check for
/// degenerate inputs), so NaN should not arise here; if it ever did,
/// treat it as equal to everything to avoid panicking the search
/// loop.
#[derive(Copy, Clone, PartialEq)]
struct FiniteF64(f64);

impl Eq for FiniteF64 {}
impl PartialOrd for FiniteF64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FiniteF64 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.partial_cmp(&other.0).unwrap_or(Ordering::Equal)
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
struct Candidate {
    dist: FiniteF64,
    id: u64,
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.dist
            .cmp(&other.dist)
            .then_with(|| self.id.cmp(&other.id))
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
pub(super) struct HnswBackend {
    params: HnswParams,
    similarity: VectorSimilarity,
    nodes: BTreeMap<u64, HnswNode>,
    entry_point: Option<u64>,
    max_level: usize,
    /// `1 / ln(M)` precomputed for layer sampling.
    ml: f64,
    /// Deterministic LCG state seeded from the index name.
    rng_state: u64,
}

impl HnswBackend {
    pub(super) fn new(similarity: VectorSimilarity, params: HnswParams, seed: u64) -> Self {
        let m = params.m.max(2);
        Self {
            params: HnswParams { m, ..params },
            similarity,
            nodes: BTreeMap::new(),
            entry_point: None,
            max_level: 0,
            ml: 1.0 / (m as f64).ln(),
            // Mix the seed so callers don't have to.
            rng_state: seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(0xBF58_476D_1CE4_E5B9),
        }
    }

    pub(super) fn insert(&mut self, id: u64, vector: LoraVector) {
        // Replace-on-duplicate: drop the old links first so we don't
        // double-link. The maintenance hook does this on property
        // SET; protect against direct re-insert just in case.
        if self.nodes.contains_key(&id) {
            self.remove(id);
        }

        // Quantize at the boundary so the rest of the graph
        // (neighbor distances, search expansion) reads a single
        // representation. Cosine is scale-invariant, so the same
        // distance helpers work without modification.
        let vector = match self.params.quantization {
            HnswQuantization::None => vector,
            HnswQuantization::Int8 => quantize_to_int8(&vector),
        };

        let level = self.sample_level();
        let node = HnswNode {
            vector,
            level,
            neighbors: vec![Vec::new(); level + 1],
        };
        self.nodes.insert(id, node);

        // First node ever — becomes the entry point. No links to make.
        let entry = match self.entry_point {
            Some(ep) => ep,
            None => {
                self.entry_point = Some(id);
                self.max_level = level;
                return;
            }
        };

        // Phase 1: zoom from max_level down to level+1 with ef=1 to
        // find a good local entry point at layer `level`.
        let mut nearest = entry;
        for l in ((level + 1)..=self.max_level).rev() {
            nearest = self
                .greedy_search_layer(id, nearest, l, 1)
                .into_iter()
                .next()
                .map(|c| c.id)
                .unwrap_or(nearest);
        }

        // Phase 2: at each layer from `level` down to 0, run a
        // wider ef-search, pick top-M closest, install bidirectional
        // links (with bounded-degree pruning for the back-link
        // direction).
        for l in (0..=level.min(self.max_level)).rev() {
            let m_max = self.m_max(l);
            let candidates = self.greedy_search_layer(id, nearest, l, self.params.ef_construction);
            let selected = self.select_closest(candidates, self.params.m);

            // Forward links: id -> selected.
            if let Some(node) = self.nodes.get_mut(&id) {
                node.neighbors[l] = selected.iter().map(|c| c.id).collect();
            }

            // Back links: each selected -> id. Prune the back-side
            // list if it grows beyond `m_max(l)` by keeping the
            // closest m_max neighbors.
            for c in &selected {
                self.add_back_link(c.id, l, id, m_max);
            }

            nearest = selected.first().map(|c| c.id).unwrap_or(nearest);
        }

        if level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(id);
        }
    }

    pub(super) fn remove(&mut self, id: u64) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        // Strip back-references. The neighbor lists may carry the
        // removed id at any layer up to `node.level`.
        for (layer, neighbors) in node.neighbors.iter().enumerate() {
            for &nid in neighbors {
                if let Some(other) = self.nodes.get_mut(&nid) {
                    if let Some(list) = other.neighbors.get_mut(layer) {
                        list.retain(|&x| x != id);
                    }
                }
            }
        }
        // If we removed the entry point, pick any surviving node at
        // the highest available level. This is the cheap recovery —
        // formally optimal recovery would re-link the orphaned
        // subgraph; in practice the graph stays well-connected
        // because every insert touched many nodes.
        if self.entry_point == Some(id) {
            let new_entry = self
                .nodes
                .iter()
                .max_by_key(|(_, n)| n.level)
                .map(|(&id, n)| (id, n.level));
            match new_entry {
                Some((new_id, new_level)) => {
                    self.entry_point = Some(new_id);
                    self.max_level = new_level;
                }
                None => {
                    self.entry_point = None;
                    self.max_level = 0;
                }
            }
        }
    }

    pub(super) fn query(
        &self,
        query: &LoraVector,
        k: usize,
        restrict_to: Option<&BTreeSet<u64>>,
    ) -> Vec<(u64, f64)> {
        let Some(entry) = self.entry_point else {
            return Vec::new();
        };
        if k == 0 || self.nodes.is_empty() {
            return Vec::new();
        }

        // Quantize the query to match stored vectors. The clone-on-
        // None branch avoids touching the input when quantization is
        // off (no per-query allocation in the common case).
        let owned_query;
        let query: &LoraVector = match self.params.quantization {
            HnswQuantization::None => query,
            HnswQuantization::Int8 => {
                owned_query = quantize_to_int8(query);
                &owned_query
            }
        };

        // Top-down zoom with ef=1 from max_level..1. Routing layers
        // ignore `restrict_to` — they pick the best hop, even if
        // that hop itself isn't in the allowed set, because the
        // graph structure carries information from every node.
        let mut nearest = entry;
        for l in (1..=self.max_level).rev() {
            if let Some(closer) = self
                .greedy_search_layer_against(query, nearest, l, 1)
                .into_iter()
                .next()
            {
                nearest = closer.id;
            }
        }

        // Layer 0: when a filter is in play, bump ef so the
        // post-filter has enough candidates to return k. The
        // multiplier is empirical — under tight filters, recall
        // degrades faster than ef grows; users facing very
        // selective filters should raise vector.hnsw.ef_search.
        let mut ef = self.params.ef_search.max(k);
        if restrict_to.is_some() {
            ef = ef.saturating_mul(4).max(k * 8);
        }
        let candidates = self.greedy_search_layer_against(query, nearest, 0, ef);
        let mut filtered: Vec<Candidate> = match restrict_to {
            None => candidates,
            Some(set) => candidates
                .into_iter()
                .filter(|c| set.contains(&c.id))
                .collect(),
        };
        filtered.truncate(k);
        filtered.into_iter().map(|c| (c.id, -c.dist.0)).collect()
    }

    fn m_max(&self, layer: usize) -> usize {
        // Layer 0 retains a wider neighborhood (typically 2·M);
        // upper layers cap at M.
        if layer == 0 {
            self.params.m * 2
        } else {
            self.params.m
        }
    }

    /// Sample a level from a geometric distribution: most nodes land
    /// on layer 0; a handful percolate up to layer N. Uses the
    /// internal LCG so two backends with the same seed produce
    /// identical level assignments.
    fn sample_level(&mut self) -> usize {
        // Step LCG; produce u in (0, 1].
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Use upper 53 bits to fill a [0, 1) double; bump to (0, 1].
        let bits = self.rng_state >> 11;
        let unit = (bits as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
        (-(unit.ln()) * self.ml).floor() as usize
    }

    /// Run a Phase-2 greedy search starting from `entry` at `layer`,
    /// scoring against the vector for the *newly inserted* node
    /// `query_id` (which is now in the node store). Returns candidates
    /// sorted ascending by distance.
    fn greedy_search_layer(
        &self,
        query_id: u64,
        entry: u64,
        layer: usize,
        ef: usize,
    ) -> Vec<Candidate> {
        let query_vec = match self.nodes.get(&query_id) {
            Some(n) => &n.vector,
            None => return Vec::new(),
        };
        self.greedy_search_layer_against(query_vec, entry, layer, ef)
    }

    /// The actual greedy search. Maintains a min-heap frontier and
    /// max-heap result set, both capped at `ef`. Skips nodes whose
    /// (id == query_id_to_exclude) match — but Phase 2 doesn't need
    /// exclusion because the only caller from `insert` runs *after*
    /// the new node is already in `nodes`, and we want to allow
    /// linking it (just not include itself as its own neighbor; that
    /// is filtered in `select_closest`).
    fn greedy_search_layer_against(
        &self,
        query: &LoraVector,
        entry: u64,
        layer: usize,
        ef: usize,
    ) -> Vec<Candidate> {
        let mut visited: HashSet<u64> = HashSet::new();
        // `frontier` is a min-heap of unexplored nodes (closest first).
        // BinaryHeap is max-heap; use std::cmp::Reverse for min behavior.
        let mut frontier: BinaryHeap<std::cmp::Reverse<Candidate>> = BinaryHeap::new();
        // `results` is a max-heap of best-so-far (farthest first, so
        // peek() is the worst we'd evict when the heap is full).
        let mut results: BinaryHeap<Candidate> = BinaryHeap::new();

        let entry_dist = self.dist(query, entry);
        let Some(entry_dist) = entry_dist else {
            return Vec::new();
        };
        let start = Candidate {
            dist: FiniteF64(entry_dist),
            id: entry,
        };
        frontier.push(std::cmp::Reverse(start));
        results.push(start);
        visited.insert(entry);

        while let Some(std::cmp::Reverse(c)) = frontier.pop() {
            // Early exit when the nearest unexplored is already worse
            // than our worst result and the result set is full.
            if results.len() >= ef {
                if let Some(worst) = results.peek() {
                    if c.dist > worst.dist {
                        break;
                    }
                }
            }
            let Some(node) = self.nodes.get(&c.id) else {
                continue;
            };
            let Some(neighbors) = node.neighbors.get(layer) else {
                continue;
            };
            for &nid in neighbors {
                if !visited.insert(nid) {
                    continue;
                }
                let Some(d) = self.dist(query, nid) else {
                    continue;
                };
                let cand = Candidate {
                    dist: FiniteF64(d),
                    id: nid,
                };
                if results.len() < ef {
                    frontier.push(std::cmp::Reverse(cand));
                    results.push(cand);
                } else if let Some(worst) = results.peek() {
                    if cand.dist < worst.dist {
                        frontier.push(std::cmp::Reverse(cand));
                        results.pop();
                        results.push(cand);
                    }
                }
            }
        }

        let mut out: Vec<Candidate> = results.into_sorted_vec();
        // into_sorted_vec yields ascending by Ord — Candidate ordering
        // is ascending by dist, so this is already nearest-first.
        out.truncate(ef);
        out
    }

    fn select_closest(&self, mut candidates: Vec<Candidate>, m: usize) -> Vec<Candidate> {
        candidates.sort();
        candidates.truncate(m);
        candidates
    }

    fn add_back_link(&mut self, target: u64, layer: usize, source: u64, m_max: usize) {
        // Pull the target's neighbor list at this layer, push the new
        // source, prune if oversized by selecting the closest m_max.
        let target_vec = match self.nodes.get(&target) {
            Some(n) if layer <= n.level => n.vector.clone(),
            _ => return,
        };
        // Gather candidate distances for prune-by-closest.
        let mut current: Vec<Candidate> = {
            let target_node = self.nodes.get(&target).expect("checked above");
            target_node
                .neighbors
                .get(layer)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|&id| id != source)
                .filter_map(|id| {
                    let d = self.dist(&target_vec, id)?;
                    Some(Candidate {
                        dist: FiniteF64(d),
                        id,
                    })
                })
                .collect()
        };
        if let Some(d) = self.dist(&target_vec, source) {
            current.push(Candidate {
                dist: FiniteF64(d),
                id: source,
            });
        }
        current.sort();
        current.truncate(m_max);

        let target_node = self.nodes.get_mut(&target).expect("checked above");
        if let Some(list) = target_node.neighbors.get_mut(layer) {
            *list = current.into_iter().map(|c| c.id).collect();
        }
    }

    fn dist(&self, query: &LoraVector, id: u64) -> Option<f64> {
        let other = &self.nodes.get(&id)?.vector;
        self.similarity.score(query, other).map(|s| -s)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Capture the entire graph state as a serializable snapshot.
    /// `similarity` is folded in so the snapshot is self-contained:
    /// restore doesn't need a separate trip to the catalog to learn
    /// the metric.
    pub(super) fn to_snapshot(&self, similarity: VectorSimilarity) -> HnswSnapshot {
        let nodes = self
            .nodes
            .iter()
            .map(|(&id, n)| HnswNodeSnapshot {
                id,
                level: n.level,
                vector: n.vector.clone(),
                neighbors: n.neighbors.clone(),
            })
            .collect();
        HnswSnapshot {
            similarity,
            params: self.params,
            entry_point: self.entry_point,
            max_level: self.max_level,
            rng_state: self.rng_state,
            nodes,
        }
    }

    /// Rebuild a backend from a previously-captured snapshot. Skips
    /// the expensive insert path entirely: nodes go straight into
    /// the slab with their neighbor lists already wired.
    pub(super) fn from_snapshot(snap: HnswSnapshot) -> Self {
        let m = snap.params.m.max(2);
        let mut nodes = BTreeMap::new();
        for n in snap.nodes {
            nodes.insert(
                n.id,
                HnswNode {
                    vector: n.vector,
                    level: n.level,
                    neighbors: n.neighbors,
                },
            );
        }
        Self {
            params: HnswParams { m, ..snap.params },
            similarity: snap.similarity,
            nodes,
            entry_point: snap.entry_point,
            max_level: snap.max_level,
            ml: 1.0 / (m as f64).ln(),
            rng_state: snap.rng_state,
        }
    }
}

/// Seed an HnswBackend from a stable string (typically the index
/// name). Same name → same seed → same internal graph topology, so
/// snapshot reload + backfill yields the same internal structure as
/// the original session.
pub(super) fn seed_from_name(name: &str) -> u64 {
    // FxHash-style mixer, stable across runs and platforms.
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01B3);
    }
    h
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

    fn lcg(state: &mut u64) -> f32 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let bits = (*state >> 32) as u32 as i32;
        bits as f32 / (i32::MAX as f32 + 1.0)
    }

    fn make_vecs(seed: u64, n: usize, dim: usize) -> Vec<LoraVector> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        (0..n)
            .map(|_| {
                let coords: Vec<f32> = (0..dim).map(|_| lcg(&mut state)).collect();
                vec(&coords)
            })
            .collect()
    }

    #[test]
    fn empty_query_returns_empty() {
        let backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 1);
        let q = vec(&[1.0, 0.0]);
        assert!(backend.query(&q, 5, None).is_empty());
    }

    #[test]
    fn single_node_is_returned() {
        let mut backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 2);
        backend.insert(7, vec(&[1.0, 0.0, 0.0]));
        let hits = backend.query(&vec(&[1.0, 0.0, 0.0]), 1, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 7);
        assert!((hits[0].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_10_meets_target_cosine() {
        // 1k uniform random vectors at d=32; top-10 recall against
        // flat oracle on a held-out query.
        let dim = 32;
        let n = 1_000;
        let mut backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 42);
        let vectors = make_vecs(0xC051, n, dim);
        for (i, v) in vectors.iter().enumerate() {
            backend.insert(i as u64, v.clone());
        }

        let query = make_vecs(0xDEAD, 1, dim).pop().unwrap();
        let hnsw_top10 = backend.query(&query, 10, None);
        let hnsw_ids: HashSet<u64> = hnsw_top10.iter().map(|(id, _)| *id).collect();
        assert_eq!(hnsw_ids.len(), 10);

        // Oracle: brute-force top-10 over the same vectors.
        let mut scored: Vec<(u64, f64)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let s = VectorSimilarity::Cosine.score(v, &query).unwrap();
                (i as u64, s)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let oracle_top10: HashSet<u64> = scored.iter().take(10).map(|(id, _)| *id).collect();

        let recall = (hnsw_ids.intersection(&oracle_top10).count() as f64) / 10.0;
        assert!(
            recall >= 0.9,
            "recall@10 too low: {recall} (HNSW={hnsw_ids:?}, oracle={oracle_top10:?})"
        );
    }

    #[test]
    fn remove_disappears_from_results() {
        let mut backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 5);
        backend.insert(1, vec(&[1.0, 0.0]));
        backend.insert(2, vec(&[0.0, 1.0]));
        backend.insert(3, vec(&[0.9, 0.1]));
        let before = backend.query(&vec(&[1.0, 0.0]), 3, None);
        assert_eq!(before.len(), 3);
        backend.remove(1);
        let after = backend.query(&vec(&[1.0, 0.0]), 3, None);
        assert_eq!(after.len(), 2);
        assert!(after.iter().all(|(id, _)| *id != 1));
    }

    #[test]
    fn deterministic_across_two_builds_same_seed() {
        let v = make_vecs(0xBEEF, 50, 8);
        let build_one = {
            let mut b = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 99);
            for (i, vec) in v.iter().enumerate() {
                b.insert(i as u64, vec.clone());
            }
            b.query(&vec(&[0.5, -0.5, 0.1, 0.0, 0.3, -0.1, 0.2, 0.4]), 5, None)
        };
        let build_two = {
            let mut b = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 99);
            for (i, vec) in v.iter().enumerate() {
                b.insert(i as u64, vec.clone());
            }
            b.query(&vec(&[0.5, -0.5, 0.1, 0.0, 0.3, -0.1, 0.2, 0.4]), 5, None)
        };
        assert_eq!(build_one, build_two);
    }
}
