//! Hand-rolled HNSW (Hierarchical Navigable Small World) backend for
//! vector indexes. Implements Malkov & Yashunin (2018) with the
//! neighbor selection heuristic (Algorithm 4): keeping just the closest
//! M neighbors met recall targets on uniform data but split clustered
//! data into islands a search could not leave (recall 0 at 20,000
//! clustered 64-dim vectors).
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
use std::collections::{BTreeSet, BinaryHeap, HashSet};
use std::hash::BuildHasherDefault;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::types::vector::RawCoordinate;
use crate::{LoraVector, VectorCoordinateType};

use super::cow::KeyHasher;
use super::id_map::CowIdMap;
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

/// Quantize an input vector to int8: normalise it to unit length, then
/// scale each coordinate by 127. Int8 indexes are cosine-only, and cosine
/// ignores length, so normalising first keeps every direction apart;
/// scaling an un-normalised vector directly clips its coordinates to
/// `[-128, 127]` and makes long vectors look alike. Used for both stored
/// vectors at insert time and the query vector at search time so
/// distances stay comparable.
fn quantize_to_int8(input: &LoraVector) -> Option<LoraVector> {
    let dim = input.dimension;
    let values: Vec<f32> = (0..dim)
        .map(|i| input.values.f32_at(i).unwrap_or(0.0))
        .collect();
    let norm = values
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    let scale = if norm > 0.0 && norm.is_finite() {
        127.0 / norm
    } else {
        0.0
    };
    let coords: Vec<RawCoordinate> = values
        .iter()
        .map(|v| {
            let scaled = (f64::from(*v) * scale).round().clamp(-128.0, 127.0) as i64;
            RawCoordinate::Int(scaled)
        })
        .collect();
    let dim = i64::try_from(dim).ok()?;
    LoraVector::try_new(coords, dim, VectorCoordinateType::Integer8).ok()
}

/// A stored vector's coordinates, flat: what the distance kernels read.
/// FLOAT vectors keep their `f32` image (LoraDB's vector math runs in
/// `f32`, so scores are unchanged), and an int8-quantized index keeps one
/// byte per coordinate. Behind `Arc`, so copying a node shares them.
#[derive(Debug, Clone)]
pub(super) enum Coords {
    F32(Arc<[f32]>),
    I8(Arc<[i8]>),
}

impl Coords {
    /// The coordinates of a stored vector (already quantized, for an
    /// int8 index): its integer-8 values as bytes, any other type as
    /// `f32`.
    fn of(vector: &LoraVector) -> Self {
        match &vector.values {
            crate::types::VectorValues::Integer8(values) => Self::I8(values.as_slice().into()),
            values => Self::F32(
                (0..values.len())
                    .map(|i| values.f32_at(i).unwrap_or(0.0))
                    .collect(),
            ),
        }
    }

    fn view(&self) -> View<'_> {
        match self {
            Self::F32(values) => View::F32(values),
            Self::I8(values) => View::I8(values),
        }
    }

    /// The coordinates as `f32`, the image the similarity functions
    /// compute with.
    fn as_f32(&self) -> std::borrow::Cow<'_, [f32]> {
        match self {
            Self::F32(values) => std::borrow::Cow::Borrowed(values),
            Self::I8(values) => values.iter().map(|v| f32::from(*v)).collect(),
        }
    }

    /// Back to a vector, for snapshots.
    pub(super) fn to_vector(&self) -> LoraVector {
        let (coords, kind): (Vec<RawCoordinate>, _) = match self {
            Self::F32(values) => (
                values
                    .iter()
                    .map(|v| RawCoordinate::Float(f64::from(*v)))
                    .collect(),
                VectorCoordinateType::Float32,
            ),
            Self::I8(values) => (
                values
                    .iter()
                    .map(|v| RawCoordinate::Int(i64::from(*v)))
                    .collect(),
                VectorCoordinateType::Integer8,
            ),
        };
        let dim = coords.len() as i64;
        LoraVector::try_new(coords, dim, kind).expect("coordinates of a stored vector")
    }

    pub(super) fn heap_bytes(&self) -> usize {
        match self {
            Self::F32(values) => values.len() * std::mem::size_of::<f32>(),
            Self::I8(values) => values.len(),
        }
    }

    #[cfg(test)]
    pub(super) fn ptr_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::F32(a), Self::F32(b)) => Arc::ptr_eq(a, b),
            (Self::I8(a), Self::I8(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Borrowed coordinates. Both sides of a distance are of one kind: an
/// int8 index quantizes its queries too.
#[derive(Clone, Copy)]
enum View<'a> {
    F32(&'a [f32]),
    I8(&'a [i8]),
}

/// Lanes the kernels accumulate in: independent sums the compiler keeps
/// in vector registers, where one running sum would serialize every add.
const LANES: usize = 8;

fn lanes_f32(a: &[f32], b: &[f32], term: impl Fn(f32, f32) -> f32) -> f32 {
    let mut acc = [0f32; LANES];
    let (ca, cb) = (a.chunks_exact(LANES), b.chunks_exact(LANES));
    let (ra, rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        for i in 0..LANES {
            acc[i] += term(x[i], y[i]);
        }
    }
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| term(*x, *y)).sum();
    acc.iter().sum::<f32>() + tail
}

fn lanes_i8(a: &[i8], b: &[i8], term: impl Fn(i32, i32) -> i32) -> f32 {
    // Exact in i32: 127² per term, far below overflow for any dimension
    // the catalog accepts.
    a.iter()
        .zip(b)
        .map(|(x, y)| term(i32::from(*x), i32::from(*y)))
        .sum::<i32>() as f32
}

impl View<'_> {
    fn dot(self, other: View<'_>) -> f32 {
        match (self, other) {
            (View::F32(a), View::F32(b)) => lanes_f32(a, b, |x, y| x * y),
            (View::I8(a), View::I8(b)) => lanes_i8(a, b, |x, y| x * y),
            _ => 0.0,
        }
    }

    fn l2_squared(self, other: View<'_>) -> f32 {
        match (self, other) {
            (View::F32(a), View::F32(b)) => lanes_f32(a, b, |x, y| (x - y) * (x - y)),
            (View::I8(a), View::I8(b)) => lanes_i8(a, b, |x, y| (x - y) * (x - y)),
            _ => f32::INFINITY,
        }
    }

    fn l1(self, other: View<'_>) -> f32 {
        match (self, other) {
            (View::F32(a), View::F32(b)) => lanes_f32(a, b, |x, y| (x - y).abs()),
            (View::I8(a), View::I8(b)) => lanes_i8(a, b, |x, y| (x - y).abs()),
            _ => f32::INFINITY,
        }
    }

    fn norm(self) -> f32 {
        self.dot(self).sqrt()
    }
}

/// One graph node. The coordinates and the neighbor lists are behind
/// `Arc`, so copying a node (a chunk of [`CowIdMap`] after a graph clone)
/// bumps two counts, and changing its links copies only the lists.
#[derive(Debug, Clone)]
pub(super) struct HnswNode {
    pub(super) vector: Coords,
    /// The L2 norm of `vector`, for cosine.
    norm: f32,
    pub(super) level: usize,
    /// One neighbor list per layer 0..=level, each at most
    /// `m_max(layer)` long (2·m at layer 0, m above). Exact-size slices:
    /// a list grown by pushes held up to twice its links.
    pub(super) neighbors: Arc<[Links]>,
}

impl HnswNode {
    fn new(vector: Coords, level: usize, neighbors: Arc<[Links]>) -> Self {
        let norm = vector.view().norm();
        Self {
            vector,
            norm,
            level,
            neighbors,
        }
    }

    fn point(&self) -> Point<'_> {
        Point {
            view: self.vector.view(),
            norm: self.norm,
        }
    }
}

/// Coordinates with their norm: one side of a distance.
#[derive(Clone, Copy)]
struct Point<'a> {
    view: View<'a>,
    norm: f32,
}

/// Wrapper that turns f64 into a totally-ordered type for heap use.
/// Distances are finite (degenerate inputs are skipped); if one ever
/// were NaN, treat it as equal to everything rather than panic.
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

type VisitedSet = HashSet<u64, BuildHasherDefault<KeyHasher>>;

/// One layer's neighbor ids.
type Links = Box<[u64]>;

/// A new node's links per layer, top down: see [`HnswBackend::plan`].
type Plan = Vec<(usize, Vec<Candidate>)>;

#[derive(Debug, Clone)]
pub(super) struct HnswBackend {
    params: HnswParams,
    similarity: VectorSimilarity,
    /// Copy-on-write by id: a staged write that inserts one vector copies
    /// the chunks of the nodes it links, not the whole graph.
    pub(super) nodes: CowIdMap<HnswNode>,
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
            nodes: CowIdMap::default(),
            entry_point: None,
            max_level: 0,
            ml: 1.0 / (m as f64).ln(),
            // Mix the seed so callers don't have to.
            rng_state: seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(0xBF58_476D_1CE4_E5B9),
        }
    }

    /// `vector` as the index stores it: quantized for an int8 index.
    /// `None` when it cannot be (a zero vector under int8).
    fn stored(&self, vector: &LoraVector) -> Option<Coords> {
        match self.params.quantization {
            HnswQuantization::None => Some(Coords::of(vector)),
            HnswQuantization::Int8 => quantize_to_int8(vector).map(|q| Coords::of(&q)),
        }
    }

    pub(super) fn insert(&mut self, id: u64, vector: LoraVector) {
        if let Some(node) = self.prepare(id, &vector) {
            let plan = self.plan(id, &node);
            self.apply(id, node, plan);
        }
    }

    /// Insert many vectors, as building the index over existing data
    /// does. They go in rounds: each round's vectors search the graph as
    /// it stood when the round began (on every core when the build
    /// allows), then are linked one by one in input order. A round is a
    /// sixteenth of the graph, so a vector misses at most that share of
    /// the others while it searches, and then only ones it is linked to
    /// through their common neighbors. The rounds do not depend on the
    /// thread count: every build of the same input gives the same graph.
    pub(super) fn insert_many(&mut self, items: Vec<(u64, LoraVector)>) {
        const MAX_ROUND: usize = 1024;
        let mut pending: Vec<(u64, HnswNode)> = Vec::with_capacity(items.len());
        for (id, vector) in items {
            if let Some(node) = self.prepare(id, &vector) {
                pending.push((id, node));
            }
        }
        let mut rest = pending.as_slice();
        while !rest.is_empty() {
            let size = (self.nodes.len() / 16).clamp(1, MAX_ROUND).min(rest.len());
            let (round, after) = rest.split_at(size);
            let plans = self.plan_round(round);
            for ((id, node), plan) in round.iter().zip(plans) {
                self.apply(*id, node.clone(), plan);
            }
            rest = after;
        }
    }

    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    fn plan_round(&self, round: &[(u64, HnswNode)]) -> Vec<Plan> {
        use rayon::prelude::*;
        round
            .par_iter()
            .map(|(id, node)| self.plan(*id, node))
            .collect()
    }

    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    fn plan_round(&self, round: &[(u64, HnswNode)]) -> Vec<Plan> {
        round
            .iter()
            .map(|(id, node)| self.plan(*id, node))
            .collect()
    }

    /// The node `vector` becomes, with its level drawn, after dropping any
    /// node `id` already has (a property SET re-inserts). `None` when the
    /// vector cannot be stored.
    fn prepare(&mut self, id: u64, vector: &LoraVector) -> Option<HnswNode> {
        if self.nodes.contains_key(&id) {
            self.remove(id);
        }
        let coords = self.stored(vector)?;
        let level = self.sample_level();
        Some(HnswNode::new(
            coords,
            level,
            vec![Links::default(); level + 1].into(),
        ))
    }

    /// Where `node` links on each layer it shares with the graph, top
    /// down: an ef=1 descent to its level, then on each layer an
    /// ef_construction search and the selection heuristic.
    fn plan(&self, id: u64, node: &HnswNode) -> Plan {
        let Some(entry) = self.entry_point else {
            return Vec::new();
        };
        let query = node.point();
        let mut entries = vec![entry];
        for l in ((node.level + 1)..=self.max_level).rev() {
            if let Some(closer) = self.search_layer(query, &entries, l, 1).first() {
                entries = vec![closer.id];
            }
        }
        let mut plan = Vec::new();
        for l in (0..=node.level.min(self.max_level)).rev() {
            let candidates: Vec<Candidate> = self
                .search_layer(query, &entries, l, self.params.ef_construction)
                .into_iter()
                .filter(|c| c.id != id)
                .collect();
            if !candidates.is_empty() {
                entries = candidates.iter().map(|c| c.id).collect();
            }
            plan.push((l, self.select_neighbors(&candidates, self.params.m)));
        }
        plan
    }

    /// Add `node` and its links from `plan`, both ways.
    fn apply(&mut self, id: u64, mut node: HnswNode, plan: Plan) {
        let level = node.level;
        {
            let links = Arc::make_mut(&mut node.neighbors);
            for (l, selected) in &plan {
                links[*l] = selected.iter().map(|c| c.id).collect();
            }
        }
        self.nodes.insert(id, node);
        for (l, selected) in plan {
            let m_max = self.m_max(l);
            for c in selected {
                self.add_back_link(c.id, l, id, c.dist, m_max);
            }
        }
        if self.entry_point.is_none() || level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(id);
        }
    }

    pub(super) fn remove(&mut self, id: u64) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        // Repair the links through the removed node: each neighbor that
        // linked back re-selects its links from its own and the removed
        // node's other neighbors. Only dropping the link could cut a
        // point off: the heuristic may have given it just that one.
        for (layer, neighbors) in node.neighbors.iter().enumerate() {
            for &nid in neighbors {
                // Links need not be mutual: only copy a node that has one.
                let links_back = self
                    .nodes
                    .get(&nid)
                    .and_then(|other| other.neighbors.get(layer))
                    .is_some_and(|list| list.contains(&id));
                if links_back {
                    self.relink(nid, layer, id, neighbors);
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
        // Quantized like the stored vectors, so distances compare.
        let Some(coords) = self.stored(query) else {
            return Vec::new();
        };
        let probe = HnswNode::new(coords, 0, Arc::from([]));
        let point = probe.point();

        // With a filter, a wider search so the filtered set still holds
        // k. The multiplier is empirical: under tight filters recall
        // degrades faster than ef grows, so very selective filters
        // should raise vector.hnsw.ef_search.
        let mut ef = self.params.ef_search.max(k);
        if restrict_to.is_some() {
            ef = ef.saturating_mul(4).max(k * 8);
        }
        // Down the layers with a beam of `ef`, each layer starting from
        // the whole result of the one above, not the paper's single
        // greedy hop. Layer 0 barely links one cluster to another (its
        // links come from searches that stay in a cluster), so a query
        // between clusters (a taste between genres) must reach layer 0
        // in each of them: a single hop picks one and found 60% of the
        // true top 20 on clustered vectors, this beam 94% at the
        // default ef. The upper layers hold 1/m of the points, so the
        // beam costs a fraction of the layer-0 search. Routing ignores
        // `restrict_to`: a hop outside the allowed set still leads on.
        let mut entries = vec![entry];
        for l in (1..=self.max_level).rev() {
            let found = self.search_layer(point, &entries, l, ef);
            if !found.is_empty() {
                entries = found.into_iter().map(|c| c.id).collect();
            }
        }
        let found = self.search_layer(point, &entries, 0, ef);
        self.score(&probe, found, k, restrict_to)
    }

    /// The first `k` of `candidates` in `restrict_to`, scored exactly as
    /// `vector.similarity` does, against the query as the index sees it:
    /// the kernels only rank.
    fn score(
        &self,
        probe: &HnswNode,
        candidates: Vec<Candidate>,
        k: usize,
        restrict_to: Option<&BTreeSet<u64>>,
    ) -> Vec<(u64, f64)> {
        let query = probe.vector.as_f32();
        candidates
            .into_iter()
            .filter(|c| restrict_to.is_none_or(|set| set.contains(&c.id)))
            .take(k)
            .filter_map(|c| {
                let stored = self.nodes.get(&c.id)?.vector.as_f32();
                Some((c.id, self.similarity.score_f32(&query, &stored)?))
            })
            .collect()
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

    /// Distance between two points, lower is closer: the similarity
    /// negated, or for the distance-derived similarities the distance
    /// itself (same order, without the final transform). `None` where
    /// the similarity is undefined (cosine of a zero vector).
    fn distance(&self, a: Point<'_>, b: Point<'_>) -> Option<f64> {
        let d = match self.similarity {
            VectorSimilarity::Cosine => {
                let denom = a.norm * b.norm;
                if denom == 0.0 {
                    return None;
                }
                -(a.view.dot(b.view) / denom)
            }
            VectorSimilarity::Dot => -a.view.dot(b.view),
            VectorSimilarity::Euclidean => a.view.l2_squared(b.view),
            VectorSimilarity::Manhattan => a.view.l1(b.view),
        };
        Some(f64::from(d))
    }

    fn distance_to(&self, a: Point<'_>, id: u64) -> Option<f64> {
        self.distance(a, self.nodes.get(&id)?.point())
    }

    /// Greedy best-first search of `layer` from `entry` for the `ef`
    /// points closest to `query`, nearest first. A min-heap frontier and
    /// a max-heap of results, both capped at `ef`.
    /// The `ef` nearest points to `query` on `layer`, nearest first,
    /// searched from every point of `entries` (Malkov & Yashunin,
    /// Algorithm 2: the layer above hands down its whole result set).
    fn search_layer(
        &self,
        query: Point<'_>,
        entries: &[u64],
        layer: usize,
        ef: usize,
    ) -> Vec<Candidate> {
        let mut visited = VisitedSet::with_capacity_and_hasher(ef * 8, Default::default());
        let mut frontier: BinaryHeap<std::cmp::Reverse<Candidate>> = BinaryHeap::new();
        let mut results: BinaryHeap<Candidate> = BinaryHeap::new();
        for &entry in entries {
            if !visited.insert(entry) {
                continue;
            }
            let Some(dist) = self.distance_to(query, entry) else {
                continue;
            };
            let start = Candidate {
                dist: FiniteF64(dist),
                id: entry,
            };
            frontier.push(std::cmp::Reverse(start));
            results.push(start);
            if results.len() > ef {
                results.pop();
            }
        }

        while let Some(std::cmp::Reverse(c)) = frontier.pop() {
            // The nearest unexplored point is farther than the worst
            // result of a full set: nothing closer is left to find.
            if results.len() >= ef && results.peek().is_some_and(|worst| c.dist > worst.dist) {
                break;
            }
            let Some(neighbors) = self.nodes.get(&c.id).and_then(|n| n.neighbors.get(layer)) else {
                continue;
            };
            for &nid in neighbors {
                if !visited.insert(nid) {
                    continue;
                }
                let Some(d) = self.distance_to(query, nid) else {
                    continue;
                };
                let cand = Candidate {
                    dist: FiniteF64(d),
                    id: nid,
                };
                if results.len() < ef {
                    frontier.push(std::cmp::Reverse(cand));
                    results.push(cand);
                } else if results.peek().is_some_and(|worst| cand.dist < worst.dist) {
                    frontier.push(std::cmp::Reverse(cand));
                    results.pop();
                    results.push(cand);
                }
            }
        }
        results.into_sorted_vec()
    }

    /// The neighbor selection heuristic (Malkov & Yashunin, Algorithm 4):
    /// from `candidates`, nearest first, keep each one closer to the base
    /// point than to every point already kept, up to `m`. Unlike keeping
    /// the `m` closest, this keeps links that lead out of a cluster, so
    /// clustered data (real embeddings) stays one connected graph.
    fn select_neighbors(&self, candidates: &[Candidate], m: usize) -> Vec<Candidate> {
        let mut kept: Vec<(Candidate, &HnswNode)> = Vec::with_capacity(m);
        for c in candidates {
            if kept.len() >= m {
                break;
            }
            let Some(node) = self.nodes.get(&c.id) else {
                continue;
            };
            let diverse = kept.iter().all(|(_, other)| {
                self.distance(node.point(), other.point())
                    .is_none_or(|d| FiniteF64(d) > c.dist)
            });
            if diverse {
                kept.push((*c, node));
            }
        }
        kept.into_iter().map(|(c, _)| c).collect()
    }

    /// Link `target` to `source` (at `dist`) on `layer`. A list with room
    /// takes the link as is; a full one is re-selected with the heuristic
    /// from its links and the new one.
    fn add_back_link(
        &mut self,
        target: u64,
        layer: usize,
        source: u64,
        dist: FiniteF64,
        m_max: usize,
    ) {
        let Some(node) = self.nodes.get(&target).filter(|n| layer <= n.level) else {
            return;
        };
        let current = &node.neighbors[layer];
        if current.contains(&source) {
            return;
        }
        let list: Links = if current.len() < m_max {
            current.iter().copied().chain([source]).collect()
        } else {
            let base = node.point();
            let mut candidates: Vec<Candidate> = current
                .iter()
                .filter_map(|&id| {
                    Some(Candidate {
                        dist: FiniteF64(self.distance_to(base, id)?),
                        id,
                    })
                })
                .collect();
            candidates.push(Candidate { dist, id: source });
            candidates.sort();
            self.select_neighbors(&candidates, m_max)
                .into_iter()
                .map(|c| c.id)
                .collect()
        };
        if let Some(node) = self.nodes.get_mut(&target) {
            Arc::make_mut(&mut node.neighbors)[layer] = list;
        }
    }

    /// Re-select `target`'s links on `layer` without `removed`, from its
    /// other links and `offered` (the removed node's neighbors).
    fn relink(&mut self, target: u64, layer: usize, removed: u64, offered: &[u64]) {
        let Some(node) = self.nodes.get(&target) else {
            return;
        };
        let base = node.point();
        let mut ids: Vec<u64> = node.neighbors[layer]
            .iter()
            .chain(offered)
            .copied()
            .filter(|&id| id != removed && id != target)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let mut candidates: Vec<Candidate> = ids
            .into_iter()
            .filter(|id| self.nodes.get(id).is_some_and(|n| layer <= n.level))
            .filter_map(|id| {
                Some(Candidate {
                    dist: FiniteF64(self.distance_to(base, id)?),
                    id,
                })
            })
            .collect();
        candidates.sort();
        let list: Links = self
            .select_neighbors(&candidates, self.m_max(layer))
            .into_iter()
            .map(|c| c.id)
            .collect();
        if let Some(node) = self.nodes.get_mut(&target) {
            Arc::make_mut(&mut node.neighbors)[layer] = list;
        }
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
                vector: n.vector.to_vector(),
                neighbors: n.neighbors.iter().map(|l| l.to_vec()).collect(),
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
        let mut nodes = CowIdMap::default();
        for n in snap.nodes {
            nodes.insert(
                n.id,
                HnswNode::new(
                    Coords::of(&n.vector),
                    n.level,
                    n.neighbors.into_iter().map(Vec::into_boxed_slice).collect(),
                ),
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

    /// Embedding-like vectors: 40 cluster centers and per-vector noise,
    /// the shape closest-M selection splits into islands.
    fn clustered(n: usize, dim: usize) -> Vec<LoraVector> {
        let centers = make_vecs(0xC1u64, 40, dim);
        let mut state = 0x5EED_u64;
        (0..n)
            .map(|i| {
                let center = &centers[i % centers.len()];
                let coords: Vec<f32> = (0..dim)
                    .map(|d| center.values.f32_at(d).unwrap() + 0.25 * lcg(&mut state))
                    .collect();
                vec(&coords)
            })
            .collect()
    }

    fn recall_at(
        backend: &HnswBackend,
        vectors: &[LoraVector],
        query: &LoraVector,
        k: usize,
    ) -> f64 {
        let found: HashSet<u64> = backend
            .query(query, k, None)
            .iter()
            .map(|(id, _)| *id)
            .collect();
        let mut exact: Vec<(u64, f64)> = vectors
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Some((i as u64, VectorSimilarity::Cosine.score(v, query)?)))
            .collect();
        exact.sort_by(|a, b| b.1.total_cmp(&a.1));
        let hits = exact
            .iter()
            .take(k)
            .filter(|(id, _)| found.contains(id))
            .count();
        hits as f64 / k as f64
    }

    #[test]
    fn recall_holds_on_clustered_vectors() {
        let (n, dim) = (6_000, 64);
        let vectors = clustered(n, dim);
        let mut backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 7);
        for (i, v) in vectors.iter().enumerate() {
            backend.insert(i as u64, v.clone());
        }
        let queries = make_vecs(0x0BE7, 5, dim);
        for q in &queries {
            let recall = recall_at(&backend, &vectors, q, 20);
            assert!(recall >= 0.9, "recall@20 {recall}");
        }
    }

    /// A query between two clusters (a taste between genres) has its true
    /// neighbours in both. Layer 0 barely links clusters, so the descent
    /// must reach it in each: one greedy hop per layer found ~60%.
    #[test]
    fn recall_holds_between_clusters() {
        let (n, dim) = (20_000, 64);
        let vectors = clustered(n, dim);
        let mut backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 7);
        backend.insert_many(
            vectors
                .iter()
                .enumerate()
                .map(|(i, v)| (i as u64, v.clone()))
                .collect(),
        );
        let queries: Vec<LoraVector> = (0..20)
            .map(|j| {
                // Neighbouring indices sit in neighbouring clusters.
                let (a, b) = (&vectors[j * 397], &vectors[j * 397 + 1]);
                let mid: Vec<f32> = (0..dim)
                    .map(|d| (a.values.f32_at(d).unwrap() + b.values.f32_at(d).unwrap()) / 2.0)
                    .collect();
                vec(&mid)
            })
            .collect();
        let mean = queries
            .iter()
            .map(|q| recall_at(&backend, &vectors, q, 20))
            .sum::<f64>()
            / queries.len() as f64;
        assert!(mean >= 0.85, "mean recall@20 between clusters {mean}");
    }

    #[test]
    fn a_bulk_build_keeps_recall_and_is_deterministic() {
        let (n, dim) = (6_000, 64);
        let vectors = clustered(n, dim);
        let items: Vec<(u64, LoraVector)> = vectors
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, v)| (i as u64, v))
            .collect();
        let build = || {
            let mut backend = HnswBackend::new(VectorSimilarity::Cosine, HnswParams::default(), 7);
            backend.insert_many(items.clone());
            backend
        };
        let backend = build();
        assert_eq!(backend.len(), n);
        for q in &make_vecs(0x0BE7, 5, dim) {
            let recall = recall_at(&backend, &vectors, q, 20);
            assert!(recall >= 0.9, "recall@20 {recall}");
        }
        let similarity = VectorSimilarity::Cosine;
        assert_eq!(
            backend.to_snapshot(similarity),
            build().to_snapshot(similarity)
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
