//! [`LoraPoint`] — the 2D / 3D, Cartesian / Geographic point value.

use std::fmt;

use super::srid::{
    CRS_CARTESIAN, CRS_CARTESIAN_3D, CRS_WGS84_2D, CRS_WGS84_3D, SRID_CARTESIAN, SRID_CARTESIAN_3D,
    SRID_WGS84, SRID_WGS84_3D,
};

/// A 2D or 3D point, either Cartesian or WGS-84 Geographic.
///
/// SRID distinguishes coordinate systems:
///   - SRID 7203: Cartesian 2D
///   - SRID 9157: Cartesian 3D
///   - SRID 4326: WGS-84 Geographic 2D
///   - SRID 4979: WGS-84 Geographic 3D
///
/// `z` is `Some` iff the point is 3D. For geographic 3D points `z` holds
/// the `height` in metres.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LoraPoint {
    /// First coordinate: x (Cartesian) or longitude (Geographic)
    pub x: f64,
    /// Second coordinate: y (Cartesian) or latitude (Geographic)
    pub y: f64,
    /// Third coordinate: z (Cartesian) or height in metres (Geographic).
    /// `None` for 2D points.
    pub z: Option<f64>,
    /// Spatial Reference ID.
    pub srid: u32,
}

impl LoraPoint {
    pub fn cartesian(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            z: None,
            srid: SRID_CARTESIAN,
        }
    }

    pub fn cartesian_3d(x: f64, y: f64, z: f64) -> Self {
        Self {
            x,
            y,
            z: Some(z),
            srid: SRID_CARTESIAN_3D,
        }
    }

    pub fn geographic(longitude: f64, latitude: f64) -> Self {
        Self {
            x: longitude,
            y: latitude,
            z: None,
            srid: SRID_WGS84,
        }
    }

    pub fn geographic_3d(longitude: f64, latitude: f64, height: f64) -> Self {
        Self {
            x: longitude,
            y: latitude,
            z: Some(height),
            srid: SRID_WGS84_3D,
        }
    }

    /// True for WGS-84 points (2D or 3D).
    pub fn is_geographic(&self) -> bool {
        self.srid == SRID_WGS84 || self.srid == SRID_WGS84_3D
    }

    /// True for 3D points (Cartesian or WGS-84).
    pub fn is_3d(&self) -> bool {
        self.z.is_some()
    }

    /// For geographic points, y is latitude.
    pub fn latitude(&self) -> f64 {
        self.y
    }

    /// For geographic points, x is longitude.
    pub fn longitude(&self) -> f64 {
        self.x
    }

    /// For geographic 3D points, z is height in metres.
    pub fn height(&self) -> Option<f64> {
        if self.is_geographic() {
            self.z
        } else {
            None
        }
    }

    /// Canonical CRS name string for this point's SRID.
    pub fn crs_name(&self) -> &'static str {
        match self.srid {
            SRID_CARTESIAN => CRS_CARTESIAN,
            SRID_CARTESIAN_3D => CRS_CARTESIAN_3D,
            SRID_WGS84 => CRS_WGS84_2D,
            SRID_WGS84_3D => CRS_WGS84_3D,
            _ => "unknown",
        }
    }
}

impl PartialEq for LoraPoint {
    fn eq(&self, other: &Self) -> bool {
        if self.srid != other.srid {
            return false;
        }
        if (self.x - other.x).abs() >= f64::EPSILON {
            return false;
        }
        if (self.y - other.y).abs() >= f64::EPSILON {
            return false;
        }
        match (self.z, other.z) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() < f64::EPSILON,
            _ => false,
        }
    }
}

impl Eq for LoraPoint {}

impl fmt::Display for LoraPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.is_geographic(), self.z) {
            (true, Some(z)) => write!(
                f,
                "point({{srid:{}, x:{}, y:{}, z:{}}})",
                self.srid, self.x, self.y, z
            ),
            (true, None) => write!(
                f,
                "point({{srid:{}, x:{}, y:{}}})",
                self.srid, self.x, self.y
            ),
            (false, Some(z)) => write!(f, "point({{x:{}, y:{}, z:{}}})", self.x, self.y, z),
            (false, None) => write!(f, "point({{x:{}, y:{}}})", self.x, self.y),
        }
    }
}
