//! In-memory reference backend for the storage traits.
//!
//! Layout:
//! - `graph` — the [`InMemoryGraph`] struct, its `Debug`/`Clone` impls,
//!   and the inherent helpers (slab access, adjacency, label/type
//!   indexes, replay hooks).
//! - `impls` — `GraphStorage` / `BorrowedGraphStorage` / `GraphStorageMut`
//!   impls that delegate into the inherent helpers above.
//! - `property_index` — hash-bucket property indexes used by the
//!   `find_*_by_property` lookups.
//! - `snapshot` — bridge between [`InMemoryGraph`] and the portable
//!   [`crate::SnapshotPayload`] vocabulary.
//! - `tests` — unit tests covering the in-memory backend.

mod constraint_catalog;
mod constraint_enforce;
mod entity_index_store;
#[allow(dead_code)]
mod fulltext_index;
mod graph;
mod hnsw;
mod impls;
mod index_catalog;
mod point_index;
mod property_index;
mod secondary_index_maintenance;
mod snapshot;
mod sorted_property_index;
mod stats;
mod text_index;
mod vector_index;

#[cfg(test)]
mod tests;

pub use constraint_catalog::{
    ConstraintCatalog, ConstraintDefinition, ConstraintRequest, CreateConstraintError,
    CreateConstraintOutcome, DropConstraintError, DropConstraintOutcome, StoredConstraintKind,
    StoredPropertyType, StoredPropertyTypeTerm, StoredScalarType, StoredVectorCoordType,
};
pub use constraint_enforce::ConstraintViolation;
pub use graph::InMemoryGraph;
pub use index_catalog::{
    CreateIndexError, CreateIndexOutcome, DropIndexError, DropIndexOutcome, IndexCatalog,
    IndexConfigValue, IndexDefinition, IndexRequest, StoredIndexEntity, StoredIndexKind,
    StoredIndexState,
};
pub use stats::GraphStats;
pub use vector_index::{VectorBackendSnapshot, VectorIndexSnapshot};
