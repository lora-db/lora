//! First-class VECTOR value type.
//!
//! LoraDB VECTOR values are fixed-dimension, typed numeric coordinate
//! collections. A `LoraVector` can be stored directly as a node or
//! relationship property, returned through every binding, compared for
//! equality, and used as input to the built-in vector math functions
//! (`vector.similarity`, `vector.distance`, `vector.norm`, `vector.dim`,
//! `vector.coordinates`).
//!
//! Layout:
//!
//! * [`types`] — coordinate-type tag, storage variant, [`LoraVector`]
//!   itself, and `Display`.
//! * [`build`] — [`VectorBuildError`], [`RawCoordinate`],
//!   [`LoraVector::try_new`], [`parse_string_values`].
//! * [`similarity`] — distance, norm, and similarity functions.
//!
//! Vector indexes and approximate kNN are intentionally out of scope for
//! this pass — exhaustive search via `ORDER BY vector.similarity.*(…)
//! LIMIT k` works today; an index-backed variant is future work.

mod build;
mod similarity;
mod types;

pub use build::{parse_string_values, RawCoordinate, VectorBuildError};
pub use similarity::{
    cosine_similarity_bounded, cosine_similarity_raw, dot_product, euclidean_distance,
    euclidean_distance_squared, euclidean_norm, euclidean_similarity, hamming_distance,
    manhattan_distance, manhattan_norm,
};
pub use types::{
    LoraVector, ParseVectorCoordinateTypeError, VectorCoordinateType, VectorValues,
    MAX_VECTOR_DIMENSION,
};

#[cfg(test)]
mod tests;
