//! Grid-bucket spatial index for `POINT` Cypher indexes.
//!
//! Each indexed `(label, property)` scope owns a [`PointGrid`]: a
//! `HashMap<(i32, i32), Vec<(LoraPoint, NodeId)>>` keyed by floor-
//! quantised cell coordinates. Bounding-box and distance queries
//! enumerate the cells that overlap the query envelope, then refilter
//! every candidate against the precise predicate. The grid is plain
//! 2D — z, where present, is preserved on the stored point and
//! refiltered, so 3D points work correctly even though the bucketing
//! collapses on (x, y).
//!
//! ## Why a grid (not an R-tree)
//!
//! - Insert / remove are O(1); R-tree rebalancing is amortised but
//!   has worst-case spikes that show up under bulk loads.
//! - The grid degrades gracefully if the cell size is misconfigured —
//!   queries still return the right answer, just with more refilter
//!   work. R-tree mistuning corrupts query selectivity.
//! - No new dependency. The whole module is ~150 lines of std types.
//!
//! When workloads outgrow the grid, swap in `rstar` here; the
//! `PointRegistry` surface (`add_scope` / `insert` / `update` /
//! `within_bbox` / `within_distance`) is the seam.
//!
//! ## Refcount + maintenance contract
//!
//! Same shape as [`super::text_index::TrigramRegistry`]: refcounted
//! scopes (so multiple POINT indexes on the same `(label, property)`
//! share storage); `update` mirrors the property-mutation hook.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::types::spatial::LoraPoint;

use super::cow::CowMap;
use super::entity_index_store::{ScopeKey, ScopedPropertyKey};
use super::id_map::CowIdMap;
use super::index_catalog::IndexConfigValue;

/// Default cell side. Tuned for cartesian space at world scale; WGS-84
/// users with denser data should use `OPTIONS { indexConfig: { ... } }`
/// to override (config plumbed in [`super::index_catalog`]).
/// Cell side for cartesian points, in their own units.
const DEFAULT_CELL_SIZE: f64 = 100.0;
/// Cell side for WGS-84 points, in degrees (~28 km of latitude). With the
/// cartesian default of 100 every point on Earth shared a handful of
/// cells, so a box around one country read every indexed point.
const DEFAULT_GEOGRAPHIC_CELL_SIZE: f64 = 0.25;
/// Metres per degree of latitude, rounded down so an envelope built from
/// it is never smaller than the circle (mean Earth radius 6371 km gives
/// 111 195 m).
const METRES_PER_DEGREE: f64 = 111_000.0;

#[derive(Debug, Default, Clone)]
pub(super) struct PointRegistry {
    pub(super) by_scope: HashMap<ScopedPropertyKey, PointScope>,
}

#[derive(Debug, Clone)]
pub(super) struct PointScope {
    pub(super) grid: PointGrid,
    refcount: u32,
}

impl Default for PointScope {
    fn default() -> Self {
        Self {
            grid: PointGrid::new(None),
            refcount: 0,
        }
    }
}

/// Two grids: cartesian points are bucketed in their units, geographic
/// ones in degrees. A query asks both; candidates are refiltered by the
/// caller, which also drops points of another SRID.
#[derive(Debug, Clone)]
pub(super) struct PointGrid {
    pub(super) planar: Cells,
    pub(super) geographic: Cells,
}

#[derive(Debug, Clone)]
pub(super) struct Cells {
    cell_size: f64,
    pub(super) cells: CowMap<(i32, i32), CowIdMap<LoraPoint>>,
}

impl PointGrid {
    /// `cell_size` (the index's `cellSize` option) applies to both grids.
    pub(super) fn new(cell_size: Option<f64>) -> Self {
        Self {
            planar: Cells::with_cell_size(cell_size.unwrap_or(DEFAULT_CELL_SIZE)),
            geographic: Cells::with_cell_size(cell_size.unwrap_or(DEFAULT_GEOGRAPHIC_CELL_SIZE)),
        }
    }

    fn cells_for(&mut self, point: &LoraPoint) -> &mut Cells {
        if point.is_geographic() {
            &mut self.geographic
        } else {
            &mut self.planar
        }
    }

    fn insert(&mut self, point: LoraPoint, id: u64) {
        self.cells_for(&point).insert(point, id);
    }

    fn remove(&mut self, point: &LoraPoint, id: u64) {
        self.cells_for(point).remove(point, id);
    }

    fn candidates_in_bbox(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> BTreeSet<u64> {
        let mut out = Vec::new();
        self.planar
            .extend_in_bbox(min_x, min_y, max_x, max_y, &mut out);
        self.geographic
            .extend_in_bbox(min_x, min_y, max_x, max_y, &mut out);
        out.into_iter().collect()
    }

    /// Ids within `radius` of `center`: a cartesian square of side
    /// `2 * radius`, and for geographic points (radius in metres) the
    /// latitude/longitude envelope of that circle.
    fn candidates_within(&self, center: (f64, f64), radius: f64) -> BTreeSet<u64> {
        let r = radius.abs();
        let mut out = Vec::new();
        self.planar.extend_in_bbox(
            center.0 - r,
            center.1 - r,
            center.0 + r,
            center.1 + r,
            &mut out,
        );
        if !self.geographic.cells.is_empty() {
            for (min_x, min_y, max_x, max_y) in geographic_envelope(center, r) {
                self.geographic
                    .extend_in_bbox(min_x, min_y, max_x, max_y, &mut out);
            }
        }
        out.into_iter().collect()
    }
}

/// The boxes (one, or two across the antimeridian) holding every point
/// within `radius` metres of `center` (longitude, latitude).
fn geographic_envelope(center: (f64, f64), radius: f64) -> Vec<(f64, f64, f64, f64)> {
    const WHOLE: (f64, f64, f64, f64) = (-180.0, -90.0, 180.0, 90.0);
    let (lon, lat) = center;
    let d_lat = radius / METRES_PER_DEGREE;
    let (min_lat, max_lat) = (lat - d_lat, lat + d_lat);
    if !d_lat.is_finite() || min_lat <= -90.0 || max_lat >= 90.0 {
        // The circle holds a pole: every longitude.
        return vec![(WHOLE.0, min_lat.max(-90.0), WHOLE.2, max_lat.min(90.0))];
    }
    // A degree of longitude is shortest at the latitude farthest from the
    // equator.
    let widest = min_lat.abs().max(max_lat.abs()).to_radians().cos();
    let d_lon = radius / (METRES_PER_DEGREE * widest);
    if !d_lon.is_finite() || d_lon >= 180.0 {
        return vec![(WHOLE.0, min_lat, WHOLE.2, max_lat)];
    }
    let (west, east) = (lon - d_lon, lon + d_lon);
    if west < -180.0 {
        vec![
            (-180.0, min_lat, east, max_lat),
            (west + 360.0, min_lat, 180.0, max_lat),
        ]
    } else if east > 180.0 {
        vec![
            (west, min_lat, 180.0, max_lat),
            (-180.0, min_lat, east - 360.0, max_lat),
        ]
    } else {
        vec![(west, min_lat, east, max_lat)]
    }
}

impl Cells {
    fn with_cell_size(cell_size: f64) -> Self {
        Self {
            cell_size: if cell_size.is_finite() && cell_size > 0.0 {
                cell_size
            } else {
                DEFAULT_CELL_SIZE
            },
            cells: CowMap::default(),
        }
    }

    fn cell_for(&self, x: f64, y: f64) -> (i32, i32) {
        // `as` saturates, so a far-off or infinite coordinate lands in an
        // edge cell instead of wrapping.
        (
            (x / self.cell_size).floor() as i32,
            (y / self.cell_size).floor() as i32,
        )
    }

    fn insert(&mut self, point: LoraPoint, id: u64) {
        let cell = self.cell_for(point.x, point.y);
        self.cells
            .get_or_insert_with(cell, CowIdMap::default)
            .insert(id, point);
    }

    fn remove(&mut self, point: &LoraPoint, id: u64) {
        let cell = self.cell_for(point.x, point.y);
        let present = self
            .cells
            .get(&cell)
            .and_then(|bucket| bucket.get(&id))
            .is_some_and(|stored| points_equal(stored, point));
        if !present {
            return;
        }
        let emptied = self.cells.get_mut(&cell).is_some_and(|bucket| {
            bucket.remove(&id);
            bucket.is_empty()
        });
        if emptied {
            self.cells.remove(&cell);
        }
    }

    /// Adds the ids in the cells overlapping the box. A box spanning more
    /// cells than are occupied walks the occupied ones instead, so a huge
    /// box costs what the index holds, never more.
    fn extend_in_bbox(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64, out: &mut Vec<u64>) {
        if self.cells.is_empty()
            || min_x.is_nan()
            || min_y.is_nan()
            || max_x.is_nan()
            || max_y.is_nan()
        {
            return;
        }
        let (lo_x, lo_y) = self.cell_for(min_x, min_y);
        let (hi_x, hi_y) = self.cell_for(max_x, max_y);
        let width = (hi_x as i64 - lo_x as i64).unsigned_abs() + 1;
        let height = (hi_y as i64 - lo_y as i64).unsigned_abs() + 1;
        if width.saturating_mul(height) > self.cells.len() as u64 {
            let inside =
                |&(x, y): &(i32, i32)| (lo_x..=hi_x).contains(&x) && (lo_y..=hi_y).contains(&y);
            for (cell, bucket) in self.cells.iter() {
                if inside(cell) {
                    out.extend(bucket.keys());
                }
            }
            return;
        }
        for cx in lo_x..=hi_x {
            for cy in lo_y..=hi_y {
                if let Some(bucket) = self.cells.get(&(cx, cy)) {
                    out.extend(bucket.keys());
                }
            }
        }
    }
}

impl PointRegistry {
    /// Whether any of `scopes` has an index on `property`. Checked through
    /// a read guard before maintenance takes a (copy-on-write) write guard.
    pub(super) fn covers_any<'a>(
        &self,
        scopes: impl IntoIterator<Item = &'a str>,
        property: &str,
    ) -> bool {
        !self.by_scope.is_empty()
            && scopes.into_iter().any(|scope| {
                self.by_scope
                    .contains_key(&(scope, property) as &dyn ScopeKey)
            })
    }

    /// Resolve the optional `cellSize` knob from a catalog `OPTIONS`
    /// map. Returns `None` when the key is missing, non-numeric, or
    /// non-positive — the registry then falls back to
    /// [`DEFAULT_CELL_SIZE`].
    pub(super) fn cell_size_from_options(
        options: &BTreeMap<String, IndexConfigValue>,
    ) -> Option<f64> {
        match options.get("cellSize")? {
            IndexConfigValue::Number(v) if *v > 0.0 && v.is_finite() => Some(*v),
            IndexConfigValue::Integer(v) if *v > 0 => Some(*v as f64),
            _ => None,
        }
    }

    pub(super) fn add_scope(
        &mut self,
        label: &str,
        property: &str,
        cell_size: Option<f64>,
    ) -> bool {
        let key = ScopedPropertyKey::new(label, property);
        let entry = self.by_scope.entry(key).or_insert_with(|| PointScope {
            grid: PointGrid::new(cell_size),
            refcount: 0,
        });
        let was_empty = entry.refcount == 0;
        entry.refcount = entry.refcount.saturating_add(1);
        was_empty
    }

    pub(super) fn remove_scope(&mut self, label: &str, property: &str) {
        let key = ScopedPropertyKey::new(label, property);
        if let Some(scope) = self.by_scope.get_mut(&key) {
            scope.refcount = scope.refcount.saturating_sub(1);
            if scope.refcount == 0 {
                self.by_scope.remove(&key);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn has_scope(&self, label: &str, property: &str) -> bool {
        self.by_scope
            .contains_key(&(label, property) as &dyn ScopeKey)
    }

    pub(super) fn insert(&mut self, label: &str, property: &str, id: u64, point: LoraPoint) {
        if let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) {
            scope.grid.insert(point, id);
        }
    }

    pub(super) fn update(
        &mut self,
        label: &str,
        property: &str,
        id: u64,
        old: Option<&LoraPoint>,
        new: Option<&LoraPoint>,
    ) {
        let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) else {
            return;
        };
        if let Some(old) = old {
            scope.grid.remove(old, id);
        }
        if let Some(new) = new {
            scope.grid.insert(new.clone(), id);
        }
    }

    /// Candidate ids whose stored point falls in the closed `[ll, ur]`
    /// 2D bounding box. The executor refilters with the precise
    /// inclusive/exclusive semantics.
    pub(super) fn within_bbox(
        &self,
        label: &str,
        property: &str,
        ll: (f64, f64),
        ur: (f64, f64),
    ) -> Option<BTreeSet<u64>> {
        let scope = self.by_scope.get(&(label, property) as &dyn ScopeKey)?;
        Some(scope.grid.candidates_in_bbox(
            ll.0.min(ur.0),
            ll.1.min(ur.1),
            ll.0.max(ur.0),
            ll.1.max(ur.1),
        ))
    }

    /// Candidate ids that *might* fall within `max_distance` of
    /// `(center_x, center_y)`. Implemented as the `[c-r, c+r]` 2D
    /// bounding-box probe; great-circle / cartesian distance is left
    /// to the executor's refilter.
    pub(super) fn within_distance(
        &self,
        label: &str,
        property: &str,
        center: (f64, f64),
        max_distance: f64,
    ) -> Option<BTreeSet<u64>> {
        let scope = self.by_scope.get(&(label, property) as &dyn ScopeKey)?;
        Some(scope.grid.candidates_within(center, max_distance))
    }
}

fn points_equal(a: &LoraPoint, b: &LoraPoint) -> bool {
    a.x.to_bits() == b.x.to_bits()
        && a.y.to_bits() == b.y.to_bits()
        && a.z.map(f64::to_bits) == b.z.map(f64::to_bits)
        && a.srid == b.srid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> LoraPoint {
        LoraPoint {
            x,
            y,
            z: None,
            srid: 7203, // cartesian
        }
    }

    #[test]
    fn within_bbox_returns_only_intersecting_cells() {
        let mut reg = PointRegistry::default();
        reg.add_scope("Place", "loc", None);
        reg.insert("Place", "loc", 1, p(50.0, 50.0));
        reg.insert("Place", "loc", 2, p(150.0, 150.0));
        reg.insert("Place", "loc", 3, p(500.0, 500.0));

        let got = reg
            .within_bbox("Place", "loc", (0.0, 0.0), (200.0, 200.0))
            .unwrap();
        assert!(got.contains(&1));
        assert!(got.contains(&2));
        assert!(!got.contains(&3));
    }

    #[test]
    fn update_moves_point_between_cells() {
        let mut reg = PointRegistry::default();
        reg.add_scope("Place", "loc", None);
        reg.insert("Place", "loc", 1, p(50.0, 50.0));

        let old = p(50.0, 50.0);
        let new = p(500.0, 500.0);
        reg.update("Place", "loc", 1, Some(&old), Some(&new));

        let inside_old = reg
            .within_bbox("Place", "loc", (0.0, 0.0), (100.0, 100.0))
            .unwrap();
        assert!(!inside_old.contains(&1));
        let inside_new = reg
            .within_bbox("Place", "loc", (400.0, 400.0), (600.0, 600.0))
            .unwrap();
        assert!(inside_new.contains(&1));
    }

    #[test]
    fn refcount_keeps_scope_until_last_remove() {
        let mut reg = PointRegistry::default();
        assert!(reg.add_scope("Place", "loc", None));
        assert!(!reg.add_scope("Place", "loc", None));
        reg.remove_scope("Place", "loc");
        assert!(reg.has_scope("Place", "loc"));
        reg.remove_scope("Place", "loc");
        assert!(!reg.has_scope("Place", "loc"));
    }

    #[test]
    fn within_distance_overcollects_then_executor_refilters() {
        let mut reg = PointRegistry::default();
        reg.add_scope("Place", "loc", None);
        reg.insert("Place", "loc", 1, p(0.0, 0.0));
        reg.insert("Place", "loc", 2, p(50.0, 50.0));
        reg.insert("Place", "loc", 3, p(1000.0, 1000.0));

        let near = reg
            .within_distance("Place", "loc", (0.0, 0.0), 100.0)
            .unwrap();
        // The bbox includes the diagonal; refilter would drop id 2 if
        // the precise distance check is < 50.0, but the index is
        // intentionally conservative.
        assert!(near.contains(&1));
        assert!(near.contains(&2));
        assert!(!near.contains(&3));
    }

    #[test]
    fn huge_bbox_walks_the_occupied_cells() {
        let mut reg = PointRegistry::default();
        reg.add_scope("Place", "loc", Some(1.0));
        reg.insert("Place", "loc", 1, p(0.0, 0.0));
        reg.insert("Place", "loc", 2, p(1_000_000.0, 1_000_000.0));

        let got = reg
            .within_bbox(
                "Place",
                "loc",
                (-1_000_000.0, -1_000_000.0),
                (1_000_000.0, 1_000_000.0),
            )
            .unwrap();
        assert_eq!(got, BTreeSet::from([1, 2]));
    }
}
