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

use std::collections::{BTreeSet, HashMap};

use crate::types::spatial::LoraPoint;

/// Default cell side. Tuned for cartesian space at world scale; WGS-84
/// users with denser data should use `OPTIONS { indexConfig: { ... } }`
/// to override (config plumbed in [`super::index_catalog`]).
const DEFAULT_CELL_SIZE: f64 = 100.0;
const MAX_CELLS_TO_ENUMERATE: u128 = 100_000;

#[derive(Debug, Default, Clone)]
pub(super) struct PointRegistry {
    by_scope: HashMap<PointScopeKey, PointScope>,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub(super) struct PointScopeKey {
    pub label: String,
    pub property: String,
}

#[derive(Debug, Clone)]
pub(super) struct PointScope {
    grid: PointGrid,
    refcount: u32,
}

impl Default for PointScope {
    fn default() -> Self {
        Self {
            grid: PointGrid::with_cell_size(DEFAULT_CELL_SIZE),
            refcount: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct PointGrid {
    cell_size: f64,
    cells: HashMap<(i32, i32), Vec<(LoraPoint, u64)>>,
}

impl PointGrid {
    pub(super) fn with_cell_size(cell_size: f64) -> Self {
        Self {
            cell_size: if cell_size.is_finite() && cell_size > 0.0 {
                cell_size
            } else {
                DEFAULT_CELL_SIZE
            },
            cells: HashMap::new(),
        }
    }

    fn cell_for(&self, x: f64, y: f64) -> (i32, i32) {
        (
            (x / self.cell_size).floor() as i32,
            (y / self.cell_size).floor() as i32,
        )
    }

    fn insert(&mut self, point: LoraPoint, id: u64) {
        let cell = self.cell_for(point.x, point.y);
        self.cells.entry(cell).or_default().push((point, id));
    }

    fn remove(&mut self, point: &LoraPoint, id: u64) {
        let cell = self.cell_for(point.x, point.y);
        if let Some(bucket) = self.cells.get_mut(&cell) {
            if let Some(pos) = bucket
                .iter()
                .position(|(p, i)| *i == id && points_equal(p, point))
            {
                bucket.swap_remove(pos);
            }
            if bucket.is_empty() {
                self.cells.remove(&cell);
            }
        }
    }

    fn cells_in_bbox(
        &self,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> Option<Vec<(i32, i32)>> {
        let (lo_x, lo_y) = self.cell_for(min_x, min_y);
        let (hi_x, hi_y) = self.cell_for(max_x, max_y);
        let width = (hi_x as i64 - lo_x as i64).unsigned_abs() as u128 + 1;
        let height = (hi_y as i64 - lo_y as i64).unsigned_abs() as u128 + 1;
        let cell_count = width.saturating_mul(height);
        if cell_count > MAX_CELLS_TO_ENUMERATE {
            return None;
        }

        let mut out = Vec::with_capacity(cell_count as usize);
        for cx in lo_x..=hi_x {
            for cy in lo_y..=hi_y {
                out.push((cx, cy));
            }
        }
        Some(out)
    }

    fn candidates_in_bbox(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> BTreeSet<u64> {
        let mut out = BTreeSet::new();
        let Some(cells) = self.cells_in_bbox(min_x, min_y, max_x, max_y) else {
            return self.all_ids();
        };
        for cell in cells {
            if let Some(bucket) = self.cells.get(&cell) {
                for (_, id) in bucket {
                    out.insert(*id);
                }
            }
        }
        out
    }

    fn all_ids(&self) -> BTreeSet<u64> {
        self.cells
            .values()
            .flat_map(|bucket| bucket.iter().map(|(_, id)| *id))
            .collect()
    }
}

impl PointRegistry {
    pub(super) fn add_scope(
        &mut self,
        label: &str,
        property: &str,
        cell_size: Option<f64>,
    ) -> bool {
        let key = PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        };
        let entry = self.by_scope.entry(key).or_insert_with(|| PointScope {
            grid: PointGrid::with_cell_size(cell_size.unwrap_or(DEFAULT_CELL_SIZE)),
            refcount: 0,
        });
        let was_empty = entry.refcount == 0;
        entry.refcount = entry.refcount.saturating_add(1);
        was_empty
    }

    pub(super) fn remove_scope(&mut self, label: &str, property: &str) {
        let key = PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        };
        if let Some(scope) = self.by_scope.get_mut(&key) {
            scope.refcount = scope.refcount.saturating_sub(1);
            if scope.refcount == 0 {
                self.by_scope.remove(&key);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn has_scope(&self, label: &str, property: &str) -> bool {
        self.by_scope.contains_key(&PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        })
    }

    pub(super) fn insert(&mut self, label: &str, property: &str, id: u64, point: LoraPoint) {
        if let Some(scope) = self.by_scope.get_mut(&PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        }) {
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
        let Some(scope) = self.by_scope.get_mut(&PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        }) else {
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
        let scope = self.by_scope.get(&PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        })?;
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
        let scope = self.by_scope.get(&PointScopeKey {
            label: label.to_string(),
            property: property.to_string(),
        })?;
        // Conservative bounding box: a square of side 2 * max_distance.
        // For WGS-84 this overcollects near the poles (the square in
        // (lat, lon) space is wider than the actual great-circle
        // circle), and the refilter trims it.
        let r = max_distance.abs();
        Some(
            scope
                .grid
                .candidates_in_bbox(center.0 - r, center.1 - r, center.0 + r, center.1 + r),
        )
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
    fn huge_bbox_falls_back_to_all_indexed_ids() {
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
