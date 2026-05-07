//! Storage-layer value types — the vocabulary every backend speaks.
//!
//! Layout:
//!
//! * [`graph`] — graph-shaped envelopes: `NodeId`, `RelationshipId`,
//!   `NodeRecord`, `RelationshipRecord`, `Properties`,
//!   `ExpandedRelationship`.
//! * [`property_value`] — the polymorphic `PropertyValue` enum.
//! * [`binary`] — `LoraBinary`, the byte-string property type.
//! * [`spatial`] — points, SRID/CRS constants, distance functions.
//! * [`temporal`] — date / time / datetime / duration types.
//! * [`vector`] — vector property + similarity / distance functions.
//!
//! The trait surface that describes how a backend exposes these lives
//! in [`crate::traits`].

pub mod binary;
pub mod graph;
pub mod property_value;
pub mod spatial;
pub mod temporal;
pub mod vector;

pub use binary::LoraBinary;
pub use graph::{
    ExpandedRelationship, NodeId, NodeRecord, Properties, RelationshipId, RelationshipRecord,
};
pub use property_value::PropertyValue;
pub use spatial::{
    cartesian_distance, haversine_distance, point_distance, resolve_srid, resolve_srid_checked,
    srid_from_crs_name, srid_is_3d, srid_is_geographic, srid_is_supported, LoraPoint,
    PointKeyFamily, SridResolveError, CRS_CARTESIAN, CRS_CARTESIAN_3D, CRS_WGS84_2D, CRS_WGS84_3D,
    SRID_CARTESIAN, SRID_CARTESIAN_3D, SRID_WGS84, SRID_WGS84_3D,
};
pub use temporal::{
    days_in_month, is_leap_year, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime,
    LoraLocalTime, LoraTime,
};
pub use vector::{
    cosine_similarity_bounded, cosine_similarity_raw, dot_product, euclidean_distance,
    euclidean_distance_squared, euclidean_norm, euclidean_similarity, hamming_distance,
    manhattan_distance, manhattan_norm, parse_string_values, LoraVector,
    ParseVectorCoordinateTypeError, RawCoordinate, VectorBuildError, VectorCoordinateType,
    VectorValues, MAX_VECTOR_DIMENSION,
};
