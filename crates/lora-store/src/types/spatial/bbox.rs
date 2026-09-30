//! Bounding-box containment shared by `geo.within_bbox` and the point
//! index seek.
//!
//! On a geographic CRS a box whose lower-left longitude is greater than its
//! upper-right longitude crosses the antimeridian: it covers
//! `[ll.lon, 180]` and `[-180, ur.lon]`, as in Neo4j's `point.withinBBox`.
//! Map viewports report such bounds when they cross the date line. On a
//! cartesian CRS the corners are normalised to min/max.

use super::point::LoraPoint;

/// The x (longitude) intervals a box covers: one, or two when a geographic
/// box crosses the antimeridian.
pub fn bbox_x_ranges(ll: &LoraPoint, ur: &LoraPoint) -> ([(f64, f64); 2], usize) {
    if ll.is_geographic() && ll.x > ur.x {
        ([(ll.x, 180.0), (-180.0, ur.x)], 2)
    } else {
        ([(ll.x.min(ur.x), ll.x.max(ur.x)), (0.0, 0.0)], 1)
    }
}

/// Whether `p` lies in the closed box `[ll, ur]`. `None` when the point and
/// the corners do not share an SRID, or mix 2D and 3D.
pub fn bbox_contains(p: &LoraPoint, ll: &LoraPoint, ur: &LoraPoint) -> Option<bool> {
    if p.srid != ll.srid || p.srid != ur.srid {
        return None;
    }
    let (ranges, n) = bbox_x_ranges(ll, ur);
    let in_x = ranges[..n].iter().any(|(lo, hi)| p.x >= *lo && p.x <= *hi);
    let in_y = p.y >= ll.y.min(ur.y) && p.y <= ll.y.max(ur.y);
    let in_z = match (p.z, ll.z, ur.z) {
        (Some(pz), Some(lz), Some(uz)) => pz >= lz.min(uz) && pz <= lz.max(uz),
        (None, None, None) => true,
        _ => return None,
    };
    Some(in_x && in_y && in_z)
}
