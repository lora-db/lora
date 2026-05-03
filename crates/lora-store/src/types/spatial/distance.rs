//! Distance helpers — Euclidean for Cartesian SRIDs, Haversine for
//! geographic SRIDs, plus the `point_distance` dispatcher used by the
//! `point.distance` Cypher function.

use super::point::LoraPoint;

/// Euclidean distance between two Cartesian points (2D or 3D).
///
/// Callers are responsible for ensuring both points share the same SRID;
/// `point_distance` is the usual entry point.
pub fn cartesian_distance(a: &LoraPoint, b: &LoraPoint) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = match (a.z, b.z) {
        (Some(za), Some(zb)) => za - zb,
        _ => 0.0,
    };
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Haversine distance in metres between two WGS-84 geographic points.
///
/// Height is **ignored** even for WGS-84-3D inputs — we compute the
/// great-circle distance on the reference sphere.
pub fn haversine_distance(a: &LoraPoint, b: &LoraPoint) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;

    let lat1 = a.latitude().to_radians();
    let lat2 = b.latitude().to_radians();
    let dlat = (b.latitude() - a.latitude()).to_radians();
    let dlon = (b.longitude() - a.longitude()).to_radians();

    let half = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * half.sqrt().asin();
    EARTH_RADIUS_M * c
}

/// Distance between two points — dispatches to Euclidean or Haversine
/// depending on SRID. Returns `None` if the SRIDs don't match, which
/// also covers the 2D-vs-3D mismatch since the dimension is baked into
/// the SRID (7203 vs 9157, 4326 vs 4979).
pub fn point_distance(a: &LoraPoint, b: &LoraPoint) -> Option<f64> {
    if a.srid != b.srid {
        return None;
    }
    if a.is_geographic() {
        Some(haversine_distance(a, b))
    } else {
        Some(cartesian_distance(a, b))
    }
}
