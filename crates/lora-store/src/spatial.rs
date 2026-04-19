use std::fmt;

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
#[derive(Debug, Clone)]
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

pub const SRID_CARTESIAN: u32 = 7203;
pub const SRID_CARTESIAN_3D: u32 = 9157;
pub const SRID_WGS84: u32 = 4326;
pub const SRID_WGS84_3D: u32 = 4979;

/// Canonical CRS name strings as understood by `point()`.
pub const CRS_CARTESIAN: &str = "cartesian";
pub const CRS_CARTESIAN_3D: &str = "cartesian-3D";
pub const CRS_WGS84_2D: &str = "WGS-84-2D";
pub const CRS_WGS84_3D: &str = "WGS-84-3D";

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

// ---------------------------------------------------------------------------
// CRS / SRID resolution helpers
// ---------------------------------------------------------------------------

/// Which coordinate family the caller used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKeyFamily {
    /// `x`/`y` (+ optional `z`).
    Cartesian,
    /// `longitude`/`latitude` (+ optional `height`).
    Geographic,
}

/// Normalise a CRS name string to its canonical SRID.
///
/// Case-insensitive. Accepts "WGS-84" as an alias for the 2D form.
pub fn srid_from_crs_name(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "cartesian" => Some(SRID_CARTESIAN),
        "cartesian-3d" => Some(SRID_CARTESIAN_3D),
        "wgs-84" | "wgs-84-2d" => Some(SRID_WGS84),
        "wgs-84-3d" => Some(SRID_WGS84_3D),
        _ => None,
    }
}

/// Recognise a bare SRID as one of the four supported values.
pub fn srid_is_supported(srid: u32) -> bool {
    matches!(
        srid,
        SRID_CARTESIAN | SRID_CARTESIAN_3D | SRID_WGS84 | SRID_WGS84_3D
    )
}

pub fn srid_is_3d(srid: u32) -> bool {
    matches!(srid, SRID_CARTESIAN_3D | SRID_WGS84_3D)
}

pub fn srid_is_geographic(srid: u32) -> bool {
    matches!(srid, SRID_WGS84 | SRID_WGS84_3D)
}

/// Resolve a final SRID from the optional user-provided `crs`, `srid`, plus
/// the detected key family and dimensionality.
///
/// Errors on: unknown CRS name, unsupported SRID, CRS/SRID conflict,
/// CRS/family conflict (e.g. geographic keys with `crs: "cartesian"`),
/// CRS/dimensionality conflict (2D coords with a 3D CRS or vice-versa).
pub fn resolve_srid(
    crs: Option<&str>,
    srid: Option<i64>,
    family: PointKeyFamily,
    is_3d: bool,
) -> Result<u32, String> {
    let crs_srid = match crs {
        Some(name) => Some(srid_from_crs_name(name).ok_or_else(|| {
            format!(
                "point() got unsupported crs '{name}' \
                 (expected one of cartesian, cartesian-3D, WGS-84, WGS-84-3D)"
            )
        })?),
        None => None,
    };

    let explicit_srid = match srid {
        Some(n) => {
            if n < 0 || n > u32::MAX as i64 {
                return Err(format!("point() got unsupported srid {n}"));
            }
            let n = n as u32;
            if !srid_is_supported(n) {
                return Err(format!(
                    "point() got unsupported srid {n} \
                     (expected one of 7203, 9157, 4326, 4979)"
                ));
            }
            Some(n)
        }
        None => None,
    };

    let resolved = match (crs_srid, explicit_srid) {
        (Some(a), Some(b)) if a != b => {
            return Err(format!(
                "point() crs '{}' and srid {} do not agree",
                crs.unwrap(),
                b
            ));
        }
        (Some(a), _) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    let final_srid = match resolved {
        Some(s) => {
            // Family agreement.
            let srid_geo = srid_is_geographic(s);
            let family_geo = matches!(family, PointKeyFamily::Geographic);
            if srid_geo != family_geo {
                return Err(format!(
                    "point() coordinates use {} keys but crs/srid is {}",
                    if family_geo {
                        "geographic (longitude/latitude)"
                    } else {
                        "cartesian (x/y)"
                    },
                    if srid_geo { "geographic" } else { "cartesian" }
                ));
            }
            // Dimensionality agreement.
            if srid_is_3d(s) != is_3d {
                return Err(format!(
                    "point() dimensionality mismatch: {} coordinates but {} crs/srid",
                    if is_3d { "3D" } else { "2D" },
                    if srid_is_3d(s) { "3D" } else { "2D" }
                ));
            }
            s
        }
        None => match (family, is_3d) {
            (PointKeyFamily::Cartesian, false) => SRID_CARTESIAN,
            (PointKeyFamily::Cartesian, true) => SRID_CARTESIAN_3D,
            (PointKeyFamily::Geographic, false) => SRID_WGS84,
            (PointKeyFamily::Geographic, true) => SRID_WGS84_3D,
        },
    };

    Ok(final_srid)
}

// ---------------------------------------------------------------------------
// Distance
// ---------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_srid_defaults_cartesian_2d() {
        let s = resolve_srid(None, None, PointKeyFamily::Cartesian, false).unwrap();
        assert_eq!(s, SRID_CARTESIAN);
    }

    #[test]
    fn resolve_srid_defaults_cartesian_3d() {
        let s = resolve_srid(None, None, PointKeyFamily::Cartesian, true).unwrap();
        assert_eq!(s, SRID_CARTESIAN_3D);
    }

    #[test]
    fn resolve_srid_defaults_wgs84_2d() {
        let s = resolve_srid(None, None, PointKeyFamily::Geographic, false).unwrap();
        assert_eq!(s, SRID_WGS84);
    }

    #[test]
    fn resolve_srid_defaults_wgs84_3d() {
        let s = resolve_srid(None, None, PointKeyFamily::Geographic, true).unwrap();
        assert_eq!(s, SRID_WGS84_3D);
    }

    #[test]
    fn resolve_srid_accepts_crs_name_case_insensitive() {
        let s = resolve_srid(Some("WGS-84"), None, PointKeyFamily::Geographic, false).unwrap();
        assert_eq!(s, SRID_WGS84);
        let s = resolve_srid(Some("wgs-84-3d"), None, PointKeyFamily::Geographic, true).unwrap();
        assert_eq!(s, SRID_WGS84_3D);
        let s = resolve_srid(Some("CARTESIAN"), None, PointKeyFamily::Cartesian, false).unwrap();
        assert_eq!(s, SRID_CARTESIAN);
    }

    #[test]
    fn resolve_srid_conflict_between_crs_and_srid() {
        let err = resolve_srid(
            Some("cartesian"),
            Some(4326),
            PointKeyFamily::Cartesian,
            false,
        )
        .unwrap_err();
        assert!(err.contains("do not agree"));
    }

    #[test]
    fn resolve_srid_rejects_unknown_crs() {
        let err =
            resolve_srid(Some("mars-centric"), None, PointKeyFamily::Cartesian, false).unwrap_err();
        assert!(err.contains("unsupported crs"));
    }

    #[test]
    fn resolve_srid_rejects_unsupported_srid() {
        let err = resolve_srid(None, Some(9999), PointKeyFamily::Cartesian, false).unwrap_err();
        assert!(err.contains("unsupported srid"));
    }

    #[test]
    fn resolve_srid_rejects_2d_crs_with_3d_coords() {
        let err =
            resolve_srid(Some("cartesian"), None, PointKeyFamily::Cartesian, true).unwrap_err();
        assert!(err.contains("dimensionality"));
    }

    #[test]
    fn resolve_srid_rejects_3d_crs_with_2d_coords() {
        let err =
            resolve_srid(Some("WGS-84-3D"), None, PointKeyFamily::Geographic, false).unwrap_err();
        assert!(err.contains("dimensionality"));
    }

    #[test]
    fn resolve_srid_rejects_family_mismatch() {
        let err =
            resolve_srid(Some("cartesian"), None, PointKeyFamily::Geographic, false).unwrap_err();
        assert!(err.contains("coordinates use"));
    }

    #[test]
    fn cartesian_3d_distance() {
        let a = LoraPoint::cartesian_3d(0.0, 0.0, 0.0);
        let b = LoraPoint::cartesian_3d(1.0, 2.0, 2.0);
        // sqrt(1 + 4 + 4) = 3
        assert!((cartesian_distance(&a, &b) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn point_distance_returns_none_on_srid_mismatch() {
        let a = LoraPoint::cartesian(0.0, 0.0);
        let b = LoraPoint::cartesian_3d(0.0, 0.0, 0.0);
        assert!(point_distance(&a, &b).is_none());
    }
}
