//! Storage layer: graph value types, the trait surface every backend
//! implements, and the in-memory reference backend.
//!
//! Public surface is laid out here so the full export of the crate is
//! readable in one place.

mod lock_table;
mod memory;
mod mutation;
mod snapshot;
mod traits;
pub mod types;

// ---------- Value types ----------
//
// Re-exported flat for convenience (`lora_store::LoraPoint`, etc.); the
// `types` module is also `pub` so callers can opt for the namespaced
// path (`lora_store::types::spatial::LoraPoint`) when they prefer it.
pub use types::{
    cartesian_distance, cosine_similarity_bounded, cosine_similarity_raw, days_in_month,
    dot_product, euclidean_distance, euclidean_distance_squared, euclidean_norm,
    euclidean_similarity, hamming_distance, haversine_distance, is_leap_year, manhattan_distance,
    manhattan_norm, parse_string_values, point_distance, resolve_srid, resolve_srid_checked,
    srid_from_crs_name, srid_is_3d, srid_is_geographic, srid_is_supported, ExpandedRelationship,
    LoraBinary, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraPoint,
    LoraTime, LoraVector, NodeId, NodeRecord, ParseVectorCoordinateTypeError, PointKeyFamily,
    Properties, PropertyValue, RawCoordinate, RelationshipId, RelationshipRecord, SridResolveError,
    VectorBuildError, VectorCoordinateType, VectorValues, CRS_CARTESIAN, CRS_CARTESIAN_3D,
    CRS_WGS84_2D, CRS_WGS84_3D, MAX_VECTOR_DIMENSION, SRID_CARTESIAN, SRID_CARTESIAN_3D,
    SRID_WGS84, SRID_WGS84_3D,
};

// ---------- Storage trait surface ----------
pub use traits::{BorrowedGraphStorage, GraphCatalog, GraphStorage, GraphStorageMut};

// ---------- In-memory backend ----------
pub use memory::InMemoryGraph;

// ---------- Mutation stream + write-set vocabulary ----------
pub use mutation::{ClosureRecorder, MutationEvent, MutationRecorder, MutationWriteSet};

// ---------- Concurrency primitives ----------
pub use lock_table::{LockTable, WriteSetLocks, LOCK_TABLE_SHARDS};

// ---------- Snapshot vocabulary (codec lives in `lora-snapshot`) ----------
pub use snapshot::{SnapshotError, SnapshotMeta, SnapshotPayload};
