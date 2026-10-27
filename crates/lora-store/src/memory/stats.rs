//! Lightweight cardinality stats used by the cost model.
//!
//! Label and type counts are exact, derived in O(labels + types) from
//! the existing `nodes_by_label` / `relationships_by_type` maps. The
//! per-(label / type, property) distinct counts are estimates from the
//! counting multi-resolution bitmaps in `memory::distinct_stats`, kept
//! for every property key on every write (deletes included) and
//! rebuilt on snapshot load / WAL replay. No separate ANALYZE phase, no
//! background sampling, and no dependence on which hash property indexes
//! happen to be active.

use std::collections::{hash_map::DefaultHasher, BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::index_catalog::{StoredIndexEntity, StoredIndexKind};

/// `(label or relationship type, property key) → estimated distinct
/// values`.
pub type DistinctValues = BTreeMap<(String, String), usize>;

/// Snapshot of graph cardinality. Populated by the storage backend
/// (see [`super::InMemoryGraph::stats`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GraphStats {
    /// Total live node count.
    pub node_count: usize,
    /// Total live relationship count.
    pub relationship_count: usize,
    /// Per-label node count. `nodes_by_label[label].len()`.
    pub nodes_by_label: BTreeMap<String, usize>,
    /// Per-rel-type relationship count. `relationships_by_type[type].len()`.
    pub relationships_by_type: BTreeMap<String, usize>,
    /// Per-(label, property) approximate distinct value count, for
    /// every property key that a live node with that label carries,
    /// indexed or not. Estimated from a distinct-value sketch kept on
    /// every write (see `memory::distinct_stats`): a function of the live
    /// data alone, so a restarted process, the writer and a process with
    /// a different lookup history get the same numbers. Absent only for
    /// keys no node of the label has (or whose values the hash index
    /// can't key: NaN, durations, points, vectors). Shared: the store
    /// reuses one map while the estimates don't change.
    pub node_distinct_values: Arc<DistinctValues>,
    /// Per-(relationship type, property) distinct value count; as above.
    pub relationship_distinct_values: Arc<DistinctValues>,
    /// Online catalog-backed range indexes by `(label_or_type, property)`.
    pub node_range_indexes: BTreeSet<(String, String)>,
    pub relationship_range_indexes: BTreeSet<(String, String)>,
    /// Online catalog-backed text indexes by `(label_or_type, property)`.
    pub node_text_indexes: BTreeSet<(String, String)>,
    pub relationship_text_indexes: BTreeSet<(String, String)>,
    /// Online catalog-backed point indexes by `(label_or_type, property)`.
    pub node_point_indexes: BTreeSet<(String, String)>,
    pub relationship_point_indexes: BTreeSet<(String, String)>,
    /// Online catalog-backed vector indexes by `(label_or_type, property)`.
    /// Tracked alongside the other index scopes so the optimizer / planner
    /// can see them in stats fingerprints, though kNN currently goes
    /// through a flat per-query scan rather than a dedicated structure.
    pub node_vector_indexes: BTreeSet<(String, String)>,
    pub relationship_vector_indexes: BTreeSet<(String, String)>,
    /// List elements held by the RANGE index scopes whose property holds
    /// lists, by `(label_or_type, property)`: what an element seek
    /// (`$x IN n.list`) is costed from. Exact counts, kept on every write.
    pub node_list_elements: BTreeMap<(String, String), ListElementStats>,
    pub relationship_list_elements: BTreeMap<(String, String), ListElementStats>,
}

/// List elements in one RANGE index scope (see
/// [`GraphStats::node_list_elements`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListElementStats {
    /// Distinct elements across every list.
    pub distinct: usize,
    /// `(element, entity)` pairs: each list counts each distinct element
    /// it holds once.
    pub postings: usize,
}

impl ListElementStats {
    /// Entities one element seek finds, on average.
    pub fn per_element(&self) -> u64 {
        (self.postings as u64).div_ceil(self.distinct.max(1) as u64)
    }
}

/// Builds a [`GraphStats::plan_signature`]: a hash of every input the
/// planner reads from the stats, with cardinalities coarsened to
/// power-of-two buckets. Backends feed it the same items `GraphStats`
/// holds, in a deterministic order, without building the maps.
pub struct PlanSignature {
    hasher: DefaultHasher,
}

impl Default for PlanSignature {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanSignature {
    pub fn new() -> Self {
        Self {
            hasher: DefaultHasher::new(),
        }
    }

    /// Bucket of a cardinality: 0 for 0, else its bit length (1, 2-3,
    /// 4-7, ...). Cost estimates within one bucket are within a factor
    /// of two of each other.
    pub fn bucket(count: usize) -> u32 {
        usize::BITS - count.leading_zeros()
    }

    pub fn totals(&mut self, nodes: usize, relationships: usize) {
        (0u8, Self::bucket(nodes), Self::bucket(relationships)).hash(&mut self.hasher);
    }

    /// A label present in the stats (even with no nodes left: an absent
    /// label and an empty one estimate differently).
    pub fn label(&mut self, label: &str, nodes: usize) {
        (1u8, label, Self::bucket(nodes)).hash(&mut self.hasher);
    }

    pub fn relationship_type(&mut self, rel_type: &str, relationships: usize) {
        (2u8, rel_type, Self::bucket(relationships)).hash(&mut self.hasher);
    }

    pub fn node_distinct(&mut self, label: &str, property: &str, distinct: usize) {
        (3u8, label, property, Self::bucket(distinct)).hash(&mut self.hasher);
    }

    pub fn relationship_distinct(&mut self, rel_type: &str, property: &str, distinct: usize) {
        (4u8, rel_type, property, Self::bucket(distinct)).hash(&mut self.hasher);
    }

    /// An online range / text / point / vector index scope, as the
    /// `*_indexes` sets of [`GraphStats`] hold them.
    pub fn index(
        &mut self,
        entity: StoredIndexEntity,
        kind: StoredIndexKind,
        label: &str,
        property: &str,
    ) {
        (5u8, entity, kind.as_str(), label, property).hash(&mut self.hasher);
    }

    /// A RANGE index scope holding list elements, by the entities one
    /// element seek finds on average.
    pub fn list_elements(
        &mut self,
        entity: StoredIndexEntity,
        label: &str,
        property: &str,
        stats: ListElementStats,
    ) {
        (
            6u8,
            entity,
            label,
            property,
            Self::bucket(stats.per_element() as usize),
        )
            .hash(&mut self.hasher);
    }

    pub fn finish(&self) -> u64 {
        self.hasher.finish()
    }
}

impl GraphStats {
    /// Selectivity of an equality predicate `label:prop = value`.
    /// Returns `Some(rows)` when we have enough info to answer; `None`
    /// when the optimizer should fall back to its conservative default.
    pub fn estimate_node_property_equality(&self, label: &str, property: &str) -> Option<u64> {
        let total = self.nodes_by_label.get(label).copied()? as u64;
        // No distinct count: no node of the label holds the key (G-75), so
        // an equality on it finds nothing. Read as the whole label, it lost
        // to any other indexed equality beside it.
        let Some(&distinct) = self
            .node_distinct_values
            .get(&(label.to_string(), property.to_string()))
        else {
            return Some(0);
        };
        // Uniform-distribution heuristic: each value owns
        // ⌈total / distinct⌉ rows.
        Some(total.div_ceil(distinct.max(1) as u64))
    }

    /// Rows an element seek `$x IN label.property` finds, from the RANGE
    /// index on the pair: the average entities per element, or 0 when
    /// the indexed values hold no list element. `None` without the index.
    pub fn estimate_node_list_element(&self, label: &str, property: &str) -> Option<u64> {
        let scope = (label.to_owned(), property.to_owned());
        if !self.node_range_indexes.contains(&scope) {
            return None;
        }
        Some(
            self.node_list_elements
                .get(&scope)
                .map_or(0, ListElementStats::per_element),
        )
    }

    /// Mirror of [`Self::estimate_node_list_element`] for relationships.
    pub fn estimate_relationship_list_element(
        &self,
        rel_type: &str,
        property: &str,
    ) -> Option<u64> {
        let scope = (rel_type.to_owned(), property.to_owned());
        if !self.relationship_range_indexes.contains(&scope) {
            return None;
        }
        Some(
            self.relationship_list_elements
                .get(&scope)
                .map_or(0, ListElementStats::per_element),
        )
    }

    pub fn label_count(&self, label: &str) -> Option<u64> {
        self.nodes_by_label.get(label).copied().map(|c| c as u64)
    }

    pub fn relationship_type_count(&self, rel_type: &str) -> Option<u64> {
        self.relationships_by_type
            .get(rel_type)
            .copied()
            .map(|c| c as u64)
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.node_count.hash(&mut hasher);
        self.relationship_count.hash(&mut hasher);
        self.nodes_by_label.hash(&mut hasher);
        self.relationships_by_type.hash(&mut hasher);
        self.node_distinct_values.hash(&mut hasher);
        self.relationship_distinct_values.hash(&mut hasher);
        self.node_range_indexes.hash(&mut hasher);
        self.relationship_range_indexes.hash(&mut hasher);
        self.node_text_indexes.hash(&mut hasher);
        self.relationship_text_indexes.hash(&mut hasher);
        self.node_point_indexes.hash(&mut hasher);
        self.relationship_point_indexes.hash(&mut hasher);
        self.node_vector_indexes.hash(&mut hasher);
        self.relationship_vector_indexes.hash(&mut hasher);
        self.node_list_elements.hash(&mut hasher);
        self.relationship_list_elements.hash(&mut hasher);
        hasher.finish()
    }

    /// Hash of what the planner reads from these stats, with every
    /// cardinality coarsened to a power-of-two bucket (see
    /// [`PlanSignature`]). Two stats with the same signature have the
    /// same labels, types, counted keys and online index scopes, and
    /// counts within a factor of two: a plan made for one is a sound
    /// and reasonable plan for the other. Plans never depend on stats
    /// for correctness, only for cost.
    pub fn plan_signature(&self) -> u64 {
        let mut sig = PlanSignature::new();
        sig.totals(self.node_count, self.relationship_count);
        for (label, count) in &self.nodes_by_label {
            sig.label(label, *count);
        }
        for (rel_type, count) in &self.relationships_by_type {
            sig.relationship_type(rel_type, *count);
        }
        for ((label, property), distinct) in self.node_distinct_values.iter() {
            sig.node_distinct(label, property, *distinct);
        }
        for ((rel_type, property), distinct) in self.relationship_distinct_values.iter() {
            sig.relationship_distinct(rel_type, property, *distinct);
        }
        let scopes = [
            (
                StoredIndexEntity::Node,
                StoredIndexKind::Range,
                &self.node_range_indexes,
            ),
            (
                StoredIndexEntity::Node,
                StoredIndexKind::Text,
                &self.node_text_indexes,
            ),
            (
                StoredIndexEntity::Node,
                StoredIndexKind::Point,
                &self.node_point_indexes,
            ),
            (
                StoredIndexEntity::Node,
                StoredIndexKind::Vector,
                &self.node_vector_indexes,
            ),
            (
                StoredIndexEntity::Relationship,
                StoredIndexKind::Range,
                &self.relationship_range_indexes,
            ),
            (
                StoredIndexEntity::Relationship,
                StoredIndexKind::Text,
                &self.relationship_text_indexes,
            ),
            (
                StoredIndexEntity::Relationship,
                StoredIndexKind::Point,
                &self.relationship_point_indexes,
            ),
            (
                StoredIndexEntity::Relationship,
                StoredIndexKind::Vector,
                &self.relationship_vector_indexes,
            ),
        ];
        for (entity, kind, set) in scopes {
            for (label, property) in set {
                sig.index(entity, kind, label, property);
            }
        }
        for (entity, elements) in [
            (StoredIndexEntity::Node, &self.node_list_elements),
            (
                StoredIndexEntity::Relationship,
                &self.relationship_list_elements,
            ),
        ] {
            for ((label, property), stats) in elements {
                sig.list_elements(entity, label, property, *stats);
            }
        }
        sig.finish()
    }

    pub fn has_node_range_index(&self, label: &str, property: &str) -> bool {
        self.node_range_indexes
            .contains(&(label.to_owned(), property.to_owned()))
    }

    pub fn has_node_text_index(&self, label: &str, property: &str) -> bool {
        self.node_text_indexes
            .contains(&(label.to_owned(), property.to_owned()))
    }

    pub fn has_node_point_index(&self, label: &str, property: &str) -> bool {
        self.node_point_indexes
            .contains(&(label.to_owned(), property.to_owned()))
    }

    pub fn has_relationship_range_index(&self, rel_type: &str, property: &str) -> bool {
        self.relationship_range_indexes
            .contains(&(rel_type.to_owned(), property.to_owned()))
    }

    pub fn has_relationship_text_index(&self, rel_type: &str, property: &str) -> bool {
        self.relationship_text_indexes
            .contains(&(rel_type.to_owned(), property.to_owned()))
    }

    pub fn has_relationship_point_index(&self, rel_type: &str, property: &str) -> bool {
        self.relationship_point_indexes
            .contains(&(rel_type.to_owned(), property.to_owned()))
    }

    pub fn has_node_vector_index(&self, label: &str, property: &str) -> bool {
        self.node_vector_indexes
            .contains(&(label.to_owned(), property.to_owned()))
    }

    pub fn has_relationship_vector_index(&self, rel_type: &str, property: &str) -> bool {
        self.relationship_vector_indexes
            .contains(&(rel_type.to_owned(), property.to_owned()))
    }
}
