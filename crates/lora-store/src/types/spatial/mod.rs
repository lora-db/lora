//! Spatial value types and helpers.
//!
//! Layout:
//!
//! * [`point`] — the [`LoraPoint`] struct, constructors, and `Display`.
//! * [`srid`] — SRID / CRS constants, [`PointKeyFamily`], and the
//!   `resolve_srid` user-input → canonical SRID resolver.
//! * [`distance`] — Cartesian + Haversine distance + the
//!   [`point_distance`] dispatcher.

mod distance;
mod point;
mod srid;

pub use distance::{cartesian_distance, haversine_distance, point_distance};
pub use point::LoraPoint;
pub use srid::{
    resolve_srid, srid_from_crs_name, srid_is_3d, srid_is_geographic, srid_is_supported,
    PointKeyFamily, CRS_CARTESIAN, CRS_CARTESIAN_3D, CRS_WGS84_2D, CRS_WGS84_3D, SRID_CARTESIAN,
    SRID_CARTESIAN_3D, SRID_WGS84, SRID_WGS84_3D,
};

#[cfg(test)]
mod tests;
