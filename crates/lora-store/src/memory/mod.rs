//! In-memory reference backend for the storage traits.
//!
//! Layout:
//! - `graph` — the [`InMemoryGraph`] struct, its `Debug`/`Clone` impls,
//!   and the inherent helpers (slab access, adjacency, label/type
//!   indexes, replay hooks).
//! - `impls` — `GraphStorage` / `GraphStorageMut` impls that delegate into
//!   the inherent helpers above.
//! - `record_slab` — node and relationship records, stored encoded (see
//!   `crate::encoded` and `crate::dict`) in leaves of 512 ids.
//! - `adjacency` — per node, one allocation holding both lists of
//!   `(type, neighbour, relationship)` entries.
//! - `id_bitmap` — the compressed id sets behind the label and
//!   relationship-type indexes.
//! - `chunked_vec` — the copy-on-write radix tree behind adjacency and the
//!   other per-id tables; `record_slab` has the same shape.
//! - `property_index` — hash-bucket property indexes used by the
//!   `find_*_by_property` lookups.
//! - `snapshot` — bridge between [`InMemoryGraph`] and the portable
//!   [`crate::SnapshotPayload`] vocabulary.
//! - `tests` — unit tests covering the in-memory backend.

mod adjacency;
mod chunked_vec;
mod constraint_catalog;
mod constraint_enforce;
mod cow;
mod distinct_stats;
mod entity_index_store;
#[allow(dead_code)]
mod fulltext_index;
mod graph;
mod hnsw;
mod id_bitmap;
mod id_map;
mod id_set;
mod impls;
mod index_catalog;
mod mem_report;
mod point_index;
mod property_index;
mod record_slab;
mod secondary_index_maintenance;
mod snapshot;
mod sorted_property_index;
mod stats;
mod text_index;
mod vector_index;

/// Bytes of an `Arc` allocation's header: the strong and weak counts.
/// Memory reports charge it once per shared allocation.
const ARC_HEADER: usize = 2 * std::mem::size_of::<usize>();

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod parallel_build_tests;
#[cfg(test)]
mod test_util;
#[cfg(test)]
mod tests;

pub use constraint_catalog::{
    ConstraintBacking, ConstraintCatalog, ConstraintDefinition, ConstraintRequest,
    CreateConstraintError, CreateConstraintOutcome, DropConstraintError, DropConstraintOutcome,
    StoredConstraintKind, StoredPropertyType, StoredPropertyTypeTerm, StoredScalarType,
    StoredVectorCoordType,
};
pub use constraint_enforce::ConstraintViolation;
pub use graph::InMemoryGraph;
pub use index_catalog::{
    CreateIndexError, CreateIndexOutcome, DropIndexError, DropIndexOutcome, IndexCatalog,
    IndexConfigValue, IndexDefinition, IndexRequest, StoredIndexEntity, StoredIndexKind,
    StoredIndexState,
};
pub use mem_report::{property_value_heap_bytes, MemoryReport, PropertyIndexKeyUsage};
pub use snapshot::{
    NodeBatch, RelationshipBatch, SnapshotCatalogs, SnapshotEncoder, SnapshotGraphBuilder,
    SnapshotNames, SnapshotNode, SnapshotRelationship,
};
pub use stats::{DistinctValues, GraphStats, ListElementStats, PlanSignature};
pub use vector_index::{VectorBackendSnapshot, VectorIndexSnapshot};
