//! Heap-byte breakdown of an [`InMemoryGraph`](super::InMemoryGraph).
//!
//! This is a *debug-only* estimator: it walks the graph's owned
//! structures and sums an approximation of their retained heap bytes,
//! attributed to the component that owns each allocation (node slab,
//! relationship slab, adjacency, label/type indexes, each secondary
//! index registry, the index catalog, the constraint catalog).
//!
//! The numbers it returns are *approximate*. `BTreeMap` and `HashMap`
//! both carry node/bucket overhead that varies with allocator state,
//! load factor, and Rust version; we use fixed amortised constants
//! ([`BTREE_PER_ENTRY`], [`HASHMAP_PER_ENTRY`]) so that callers can
//! diff two reports without the per-run noise an allocator-walking
//! profiler would introduce. For property keys, we count the
//! `Arc<str>` header but *not* the shared byte buffer — keys go
//! through the process-wide `super::super::intern` table, so adding
//! their content per occurrence would dramatically over-count the
//! marginal cost a property bag actually adds to the graph.
//!
//! Used by `crates/lora-database/benches/memory.rs` and the
//! `mem_probe*` examples in `crates/lora-server/examples/` to
//! attribute observed RSS growth to a specific component without
//! resorting to `dhat` / `heaptrack` for every run.

use std::collections::BTreeMap;
use std::mem::size_of;

use crate::{LoraBinary, LoraPoint, LoraVector, NodeRecord, PropertyValue, RelationshipRecord};

use super::entity_index_store::{IndexBundle, ScopedPropertyKey};
use super::fulltext_index::FulltextRegistry;
use super::hnsw::HnswBackend;
use super::index_catalog::{IndexCatalog, StoredIndexEntity};
use super::point_index::PointRegistry;
use super::property_index::{
    PropertyIndex, PropertyIndexKey, PropertyIndexRegistry, PropertyIndexState,
};
use super::sorted_property_index::SortedPropertyIndex;
use super::text_index::TrigramRegistry;
use super::vector_index::{FlatBackend, VectorBackend, VectorIndexRegistry};
use super::ConstraintCatalog;

/// Amortised heap overhead per `BTreeMap` entry. Empirically a BTree
/// inner node holds ~11 entries in a ~256-byte allocation, so the
/// per-entry share is roughly 24 bytes.
const BTREE_PER_ENTRY: usize = 24;

/// Amortised heap overhead per `HashMap` entry — bucket pointer plus
/// hash table load factor.
const HASHMAP_PER_ENTRY: usize = 24;

/// `std::sync::Arc<T>`: strong + weak refcount header that precedes
/// the inline `T` allocation.
const ARC_HEADER: usize = 16;

/// Aggregated retained-heap breakdown of an [`InMemoryGraph`].
///
/// All fields are in *bytes*. `usize` was chosen over `u64` so call
/// sites can use `cargo bench` reports and pretty-print without casts;
/// on 32-bit hosts a graph that overflowed this would already have
/// failed earlier.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReport {
    pub live_node_count: usize,
    pub live_relationship_count: usize,
    pub node_tombstone_count: usize,
    pub relationship_tombstone_count: usize,

    /// `nodes: Vec<Option<Arc<NodeRecord>>>` — slot vector plus the
    /// `Arc` headers plus deep payload of each live record (labels,
    /// properties — see [`property_value_heap_bytes`]).
    pub nodes_bytes: usize,
    /// `relationships: Vec<Option<Arc<RelationshipRecord>>>`.
    pub relationships_bytes: usize,

    /// `outgoing: Vec<Vec<RelationshipId>>` — outer Vec capacity plus
    /// the sum of each inner Vec's capacity in bytes.
    pub outgoing_bytes: usize,
    /// Same for `incoming`.
    pub incoming_bytes: usize,

    /// `nodes_by_label: BTreeMap<String, Vec<NodeId>>`.
    pub label_index_bytes: usize,
    /// `relationships_by_type: BTreeMap<String, Vec<RelationshipId>>`.
    pub type_index_bytes: usize,

    /// Hash-bucket property registry (`find_*_by_property`).
    pub property_index_bytes: usize,
    /// Sorted-range property index (`RANGE` catalog entries).
    pub sorted_index_bytes: usize,
    /// Trigram TEXT indexes.
    pub text_index_bytes: usize,
    /// Spatial grid POINT indexes.
    pub point_index_bytes: usize,
    /// FULLTEXT inverted indexes.
    pub fulltext_index_bytes: usize,
    /// VECTOR indexes (Flat and HNSW backends).
    pub vector_index_bytes: usize,

    pub index_catalog_bytes: usize,
    pub constraint_catalog_bytes: usize,
}

impl MemoryReport {
    /// Bytes attributable to the core graph shape: slabs, adjacency,
    /// and the label/type indexes. This is the floor every workload
    /// pays regardless of which secondary indexes are installed.
    pub fn graph_core_bytes(&self) -> usize {
        self.nodes_bytes
            + self.relationships_bytes
            + self.outgoing_bytes
            + self.incoming_bytes
            + self.label_index_bytes
            + self.type_index_bytes
    }

    /// Bytes attributable to secondary indexes — the amount that would
    /// be reclaimed by dropping every index.
    pub fn secondary_index_bytes(&self) -> usize {
        self.property_index_bytes
            + self.sorted_index_bytes
            + self.text_index_bytes
            + self.point_index_bytes
            + self.fulltext_index_bytes
            + self.vector_index_bytes
    }

    pub fn catalog_bytes(&self) -> usize {
        self.index_catalog_bytes + self.constraint_catalog_bytes
    }

    pub fn total_bytes(&self) -> usize {
        self.graph_core_bytes() + self.secondary_index_bytes() + self.catalog_bytes()
    }

    /// Average retained bytes per live node, including its share of
    /// the slab tombstone overhead. Returns 0 when there are no
    /// nodes.
    pub fn bytes_per_live_node(&self) -> f64 {
        ratio(self.nodes_bytes, self.live_node_count)
    }

    pub fn bytes_per_live_relationship(&self) -> f64 {
        ratio(self.relationships_bytes, self.live_relationship_count)
    }

    /// One-line breakdown for bench / probe output. Stable enough for
    /// `grep`-friendly snapshot diffs across runs.
    pub fn summary(&self) -> String {
        format!(
            "total={} graph={} (nodes={} rels={} out={} in={} labels={} types={}) idx={} cat={}",
            self.total_bytes(),
            self.graph_core_bytes(),
            self.nodes_bytes,
            self.relationships_bytes,
            self.outgoing_bytes,
            self.incoming_bytes,
            self.label_index_bytes,
            self.type_index_bytes,
            self.secondary_index_bytes(),
            self.catalog_bytes(),
        )
    }
}

fn ratio(num: usize, denom: usize) -> f64 {
    if denom == 0 {
        0.0
    } else {
        num as f64 / denom as f64
    }
}

pub(super) fn estimate(graph: &super::InMemoryGraph) -> MemoryReport {
    let mut report = MemoryReport {
        live_node_count: graph.live_node_count,
        live_relationship_count: graph.live_rel_count,
        node_tombstone_count: graph.nodes.len().saturating_sub(graph.live_node_count),
        relationship_tombstone_count: graph
            .relationships
            .len()
            .saturating_sub(graph.live_rel_count),
        ..MemoryReport::default()
    };

    report.nodes_bytes = node_slab_bytes(&graph.nodes);
    report.relationships_bytes = rel_slab_bytes(&graph.relationships);
    report.outgoing_bytes = adjacency_bytes(&graph.outgoing);
    report.incoming_bytes = adjacency_bytes(&graph.incoming);
    report.label_index_bytes = label_or_type_bytes(&graph.nodes_by_label);
    report.type_index_bytes = label_or_type_bytes(&graph.relationships_by_type);

    let bundle = &graph.indexes;
    if let Ok(props) = bundle.properties.read() {
        report.property_index_bytes = property_registry_bytes(&props);
    }
    report.sorted_index_bytes = sorted_registry_bytes(bundle);
    report.text_index_bytes = text_registry_bytes(bundle);
    report.point_index_bytes = point_registry_bytes(bundle);
    report.fulltext_index_bytes = fulltext_registry_bytes(bundle);
    report.vector_index_bytes = vector_registry_bytes(bundle);

    if let Ok(catalog) = bundle.catalog.read() {
        report.index_catalog_bytes = index_catalog_bytes(&catalog);
    }
    if let Ok(catalog) = graph.constraint_catalog.read() {
        report.constraint_catalog_bytes = constraint_catalog_bytes(&catalog);
    }

    report
}

// ---------------- slab + adjacency ----------------

fn node_slab_bytes(slab: &[Option<std::sync::Arc<NodeRecord>>]) -> usize {
    let outer = std::mem::size_of_val(slab);
    let mut payload = 0;
    for arc in slab.iter().flatten() {
        payload += ARC_HEADER + size_of::<NodeRecord>() + node_record_heap_bytes(arc);
    }
    outer + payload
}

fn rel_slab_bytes(slab: &[Option<std::sync::Arc<RelationshipRecord>>]) -> usize {
    let outer = std::mem::size_of_val(slab);
    let mut payload = 0;
    for arc in slab.iter().flatten() {
        payload += ARC_HEADER + size_of::<RelationshipRecord>() + rel_record_heap_bytes(arc);
    }
    outer + payload
}

fn node_record_heap_bytes(record: &NodeRecord) -> usize {
    let labels = record.labels.capacity() * size_of::<String>()
        + record.labels.iter().map(|l| l.capacity()).sum::<usize>();
    labels + properties_heap_bytes(&record.properties)
}

fn rel_record_heap_bytes(record: &RelationshipRecord) -> usize {
    record.rel_type.capacity() + properties_heap_bytes(&record.properties)
}

fn adjacency_bytes<T>(adj: &[Vec<T>]) -> usize {
    let outer = std::mem::size_of_val(adj);
    let inner: usize = adj.iter().map(|v| v.capacity() * size_of::<T>()).sum();
    outer + inner
}

fn label_or_type_bytes(map: &BTreeMap<String, Vec<u64>>) -> usize {
    let mut total = 0;
    for (key, ids) in map {
        total += BTREE_PER_ENTRY
            + size_of::<String>()
            + key.capacity()
            + size_of::<Vec<u64>>()
            + ids.capacity() * size_of::<u64>();
    }
    total
}

// ---------------- property values ----------------

/// Bytes beyond the embedded discriminant of a [`PropertyValue`]. The
/// embedded discriminant itself is counted by the caller (it sits
/// inside whatever container owns the value).
pub fn property_value_heap_bytes(value: &PropertyValue) -> usize {
    match value {
        PropertyValue::Null
        | PropertyValue::Bool(_)
        | PropertyValue::Int(_)
        | PropertyValue::Float(_)
        | PropertyValue::Date(_)
        | PropertyValue::Time(_)
        | PropertyValue::LocalTime(_)
        | PropertyValue::LocalDateTime(_)
        | PropertyValue::DateTime(_)
        | PropertyValue::Duration(_) => 0,
        PropertyValue::String(s) => s.capacity(),
        PropertyValue::Binary(b) => binary_heap_bytes(b),
        PropertyValue::Point(p) => point_heap_bytes(p),
        PropertyValue::Vector(v) => vector_heap_bytes(v),
        PropertyValue::List(items) => {
            items.capacity() * size_of::<PropertyValue>()
                + items.iter().map(property_value_heap_bytes).sum::<usize>()
        }
        PropertyValue::Map(map) => {
            let mut total = 0;
            for (key, value) in map {
                total += BTREE_PER_ENTRY
                    + size_of::<String>()
                    + key.capacity()
                    + size_of::<PropertyValue>()
                    + property_value_heap_bytes(value);
            }
            total
        }
    }
}

fn binary_heap_bytes(b: &LoraBinary) -> usize {
    let mut total = std::mem::size_of_val(b.segments());
    for seg in b.segments() {
        total += seg.capacity();
    }
    total
}

fn point_heap_bytes(_: &LoraPoint) -> usize {
    // LoraPoint is fixed-size (3 × f64 + tag + srid). No owned heap.
    0
}

fn vector_heap_bytes(v: &LoraVector) -> usize {
    use crate::types::VectorValues;
    match &v.values {
        VectorValues::Float64(xs) => xs.capacity() * size_of::<f64>(),
        VectorValues::Float32(xs) => xs.capacity() * size_of::<f32>(),
        VectorValues::Integer64(xs) => xs.capacity() * size_of::<i64>(),
        VectorValues::Integer32(xs) => xs.capacity() * size_of::<i32>(),
        VectorValues::Integer16(xs) => xs.capacity() * size_of::<i16>(),
        VectorValues::Integer8(xs) => xs.capacity() * size_of::<i8>(),
    }
}

fn properties_heap_bytes(properties: &crate::Properties) -> usize {
    let mut total = 0;
    for value in properties.values() {
        total += BTREE_PER_ENTRY
            + ARC_HEADER
            + size_of::<std::sync::Arc<str>>()
            + size_of::<PropertyValue>()
            + property_value_heap_bytes(value);
    }
    total
}

// ---------------- secondary indexes ----------------

fn property_registry_bytes(reg: &PropertyIndexRegistry) -> usize {
    property_state_bytes(&reg.node_properties) + property_state_bytes(&reg.relationship_properties)
}

fn property_state_bytes(state: &PropertyIndexState) -> usize {
    let mut total = 0;
    for key in &state.active_keys {
        total += BTREE_PER_ENTRY + size_of::<String>() + key.capacity();
    }
    total += property_index_map_bytes(&state.values);
    for (scope, by_property) in &state.scoped_values {
        total += HASHMAP_PER_ENTRY + size_of::<String>() + scope.capacity();
        total += property_index_map_bytes(by_property);
    }
    total
}

fn property_index_map_bytes(values: &PropertyIndex) -> usize {
    let mut total = 0;
    for (key, buckets) in values {
        total += HASHMAP_PER_ENTRY + size_of::<String>() + key.capacity();
        for (indexed, ids) in buckets {
            total += HASHMAP_PER_ENTRY
                + property_index_key_bytes(indexed)
                + size_of::<Vec<u64>>()
                + ids.capacity() * size_of::<u64>();
        }
    }
    total
}

fn property_index_key_bytes(key: &PropertyIndexKey) -> usize {
    size_of::<PropertyIndexKey>()
        + match key {
            PropertyIndexKey::Null
            | PropertyIndexKey::Bool(_)
            | PropertyIndexKey::Int(_)
            | PropertyIndexKey::Float(_) => 0,
            PropertyIndexKey::String(s) => s.capacity(),
            PropertyIndexKey::Binary(b) => binary_heap_bytes(b),
            PropertyIndexKey::List(items) => {
                items.capacity() * size_of::<PropertyIndexKey>()
                    + items.iter().map(property_index_key_bytes).sum::<usize>()
            }
            PropertyIndexKey::Map(map) => map
                .iter()
                .map(|(k, v)| {
                    BTREE_PER_ENTRY
                        + size_of::<String>()
                        + k.capacity()
                        + property_index_key_bytes(v)
                })
                .sum::<usize>(),
        }
}

fn scoped_key_bytes(scope: &ScopedPropertyKey) -> usize {
    size_of::<ScopedPropertyKey>() + scope.label.capacity() + scope.property.capacity()
}

fn sorted_registry_bytes(bundle: &IndexBundle) -> usize {
    let node = bundle.sorted.read(StoredIndexEntity::Node);
    let rel = bundle.sorted.read(StoredIndexEntity::Relationship);
    sorted_one(&node) + sorted_one(&rel)
}

fn sorted_one(index: &SortedPropertyIndex) -> usize {
    let mut total = 0;
    for (scope, sorted_scope) in &index.by_scope {
        total += BTREE_PER_ENTRY + scoped_key_bytes(scope);
        for (indexed, ids) in &sorted_scope.by_value {
            total += BTREE_PER_ENTRY
                + property_index_key_bytes(indexed)
                + ids.len() * (BTREE_PER_ENTRY + size_of::<u64>());
        }
    }
    total
}

fn text_registry_bytes(bundle: &IndexBundle) -> usize {
    let node = bundle.text.read(StoredIndexEntity::Node);
    let rel = bundle.text.read(StoredIndexEntity::Relationship);
    text_one(&node) + text_one(&rel)
}

fn text_one(registry: &TrigramRegistry) -> usize {
    let mut total = 0;
    for (scope, trigram_scope) in &registry.by_scope {
        total += HASHMAP_PER_ENTRY + scoped_key_bytes(scope);
        for ids in trigram_scope.grams.values() {
            total += BTREE_PER_ENTRY + 3 + ids.len() * (BTREE_PER_ENTRY + size_of::<u64>());
        }
    }
    total
}

fn point_registry_bytes(bundle: &IndexBundle) -> usize {
    let node = bundle.point.read(StoredIndexEntity::Node);
    let rel = bundle.point.read(StoredIndexEntity::Relationship);
    point_one(&node) + point_one(&rel)
}

fn point_one(registry: &PointRegistry) -> usize {
    let mut total = 0;
    for (scope, scope_data) in &registry.by_scope {
        total += HASHMAP_PER_ENTRY + scoped_key_bytes(scope);
        for cell in scope_data.grid.cells.values() {
            total += HASHMAP_PER_ENTRY
                + size_of::<Vec<(LoraPoint, u64)>>()
                + cell.capacity() * size_of::<(LoraPoint, u64)>();
        }
    }
    total
}

fn fulltext_registry_bytes(bundle: &IndexBundle) -> usize {
    let node = bundle.fulltext.read(StoredIndexEntity::Node);
    let rel = bundle.fulltext.read(StoredIndexEntity::Relationship);
    fulltext_one(&node) + fulltext_one(&rel)
}

fn fulltext_one(registry: &FulltextRegistry) -> usize {
    let mut total = 0;
    for (name, index) in registry.iter() {
        total += HASHMAP_PER_ENTRY + size_of::<String>() + name.capacity();
        total += index.labels.capacity() * size_of::<String>();
        for label in &index.labels {
            total += label.capacity();
        }
        total += index.properties.capacity() * size_of::<String>();
        for property in &index.properties {
            total += property.capacity();
        }
        for (term, postings) in &index.postings {
            total += BTREE_PER_ENTRY + size_of::<String>() + term.capacity();
            total += postings.len() * (BTREE_PER_ENTRY + size_of::<u64>() + size_of::<u32>());
        }
        for terms in index.entity_terms.values() {
            total += BTREE_PER_ENTRY + size_of::<u64>();
            for term in terms {
                total += BTREE_PER_ENTRY + size_of::<String>() + term.capacity();
            }
        }
    }
    total
}

fn vector_registry_bytes(bundle: &IndexBundle) -> usize {
    let node = bundle.vector.read(StoredIndexEntity::Node);
    let rel = bundle.vector.read(StoredIndexEntity::Relationship);
    vector_one(&node) + vector_one(&rel)
}

fn vector_one(registry: &VectorIndexRegistry) -> usize {
    let mut total = 0;
    for (name, entry) in &registry.by_name {
        total += BTREE_PER_ENTRY
            + size_of::<String>()
            + name.capacity()
            + entry.label.capacity()
            + entry.property.capacity();
        total += match &entry.backend {
            VectorBackend::Flat(b) => flat_backend_bytes(b),
            VectorBackend::Hnsw(b) => hnsw_backend_bytes(b),
        };
    }
    total
}

fn flat_backend_bytes(b: &FlatBackend) -> usize {
    let mut total = 0;
    for v in b.items.values() {
        total +=
            BTREE_PER_ENTRY + size_of::<u64>() + size_of::<LoraVector>() + vector_heap_bytes(v);
    }
    total
}

fn hnsw_backend_bytes(b: &HnswBackend) -> usize {
    let mut total = 0;
    for node in b.nodes.values() {
        total += BTREE_PER_ENTRY
            + size_of::<u64>()
            + size_of::<LoraVector>()
            + vector_heap_bytes(&node.vector)
            + size_of::<usize>() // level
            + node.neighbors.capacity() * size_of::<Vec<u64>>();
        for layer in &node.neighbors {
            total += layer.capacity() * size_of::<u64>();
        }
    }
    total
}

// ---------------- catalogs ----------------

fn index_catalog_bytes(catalog: &IndexCatalog) -> usize {
    let mut total = 0;
    for def in catalog.list() {
        total += BTREE_PER_ENTRY + size_of::<String>() + def.name.capacity();
        if let Some(label) = &def.label {
            total += label.capacity();
        }
        for label in &def.additional_labels {
            total += size_of::<String>() + label.capacity();
        }
        for property in &def.properties {
            total += size_of::<String>() + property.capacity();
        }
        for key in def.options.keys() {
            total += BTREE_PER_ENTRY + size_of::<String>() + key.capacity();
        }
    }
    total
}

fn constraint_catalog_bytes(catalog: &ConstraintCatalog) -> usize {
    let mut total = 0;
    for def in catalog.list() {
        total += BTREE_PER_ENTRY + size_of::<String>() + def.name.capacity() + def.label.capacity();
        for property in &def.properties {
            total += size_of::<String>() + property.capacity();
        }
        if let Some(idx) = &def.owned_index {
            total += idx.capacity();
        }
    }
    total
}
