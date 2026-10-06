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
}

impl GraphStats {
    /// Selectivity of an equality predicate `label:prop = value`.
    /// Returns `Some(rows)` when we have enough info to answer; `None`
    /// when the optimizer should fall back to its conservative default.
    pub fn estimate_node_property_equality(&self, label: &str, property: &str) -> Option<u64> {
        let total = self.nodes_by_label.get(label).copied()? as u64;
        let distinct = self
            .node_distinct_values
            .get(&(label.to_string(), property.to_string()))
            .copied()
            .unwrap_or(1)
            .max(1) as u64;
        // Uniform-distribution heuristic: each value owns
        // ⌈total / distinct⌉ rows.
        Some(total.div_ceil(distinct))
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
        hasher.finish()
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
