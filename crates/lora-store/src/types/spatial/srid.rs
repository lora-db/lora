//! SRID / CRS constants and resolution helpers.
//!
//! Four SRIDs are recognised across the codebase: 7203 / 9157 (Cartesian
//! 2D / 3D) and 4326 / 4979 (WGS-84 Geographic 2D / 3D). The
//! `crs_name` ↔ `srid` mapping is canonicalised here so the rest of the
//! engine — `point()`, the columnar codec, the executor — speaks one
//! vocabulary.

pub const SRID_CARTESIAN: u32 = 7203;
pub const SRID_CARTESIAN_3D: u32 = 9157;
pub const SRID_WGS84: u32 = 4326;
pub const SRID_WGS84_3D: u32 = 4979;

/// Canonical CRS name strings as understood by `point()`.
pub const CRS_CARTESIAN: &str = "cartesian";
pub const CRS_CARTESIAN_3D: &str = "cartesian-3D";
pub const CRS_WGS84_2D: &str = "WGS-84-2D";
pub const CRS_WGS84_3D: &str = "WGS-84-3D";

/// Which coordinate family the caller used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKeyFamily {
    /// `x`/`y` (+ optional `z`).
    Cartesian,
    /// `longitude`/`latitude` (+ optional `height`).
    Geographic,
}

impl PointKeyFamily {
    fn as_key_description(self) -> &'static str {
        match self {
            PointKeyFamily::Cartesian => "cartesian (x/y)",
            PointKeyFamily::Geographic => "geographic (longitude/latitude)",
        }
    }

    fn as_srid_description(self) -> &'static str {
        match self {
            PointKeyFamily::Cartesian => "cartesian",
            PointKeyFamily::Geographic => "geographic",
        }
    }
}

/// Structured failure modes for SRID/CRS resolution.
///
/// [`resolve_srid`] preserves the historic `String` error contract; new
/// library code can call [`resolve_srid_checked`] when it needs to match
/// specific validation failures without parsing message text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SridResolveError {
    #[error(
        "point() got unsupported crs '{0}' \
         (expected one of cartesian, cartesian-3D, WGS-84, WGS-84-3D)"
    )]
    UnsupportedCrs(String),

    #[error("point() got unsupported srid {0}")]
    SridOutOfRange(i64),

    #[error(
        "point() got unsupported srid {0} \
         (expected one of 7203, 9157, 4326, 4979)"
    )]
    UnsupportedSrid(u32),

    #[error("point() crs '{crs}' and srid {srid} do not agree")]
    CrsSridConflict { crs: String, srid: u32 },

    #[error(
        "point() coordinates use {} keys but crs/srid is {}",
        .coordinates.as_key_description(),
        .srid.as_srid_description()
    )]
    FamilyMismatch {
        coordinates: PointKeyFamily,
        srid: PointKeyFamily,
    },

    #[error(
        "point() dimensionality mismatch: {} coordinates but {} crs/srid",
        if *.coordinates_3d { "3D" } else { "2D" },
        if *.srid_3d { "3D" } else { "2D" }
    )]
    DimensionMismatch { coordinates_3d: bool, srid_3d: bool },
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
    resolve_srid_checked(crs, srid, family, is_3d).map_err(|err| err.to_string())
}

/// Resolve a final SRID, returning a structured error for callers that need
/// to distinguish validation failures without depending on display text.
pub fn resolve_srid_checked(
    crs: Option<&str>,
    srid: Option<i64>,
    family: PointKeyFamily,
    is_3d: bool,
) -> Result<u32, SridResolveError> {
    let crs_srid = match crs {
        Some(name) => Some(
            srid_from_crs_name(name)
                .ok_or_else(|| SridResolveError::UnsupportedCrs(name.to_string()))?,
        ),
        None => None,
    };

    let explicit_srid = match srid {
        Some(n) => {
            if n < 0 || n > u32::MAX as i64 {
                return Err(SridResolveError::SridOutOfRange(n));
            }
            let n = n as u32;
            if !srid_is_supported(n) {
                return Err(SridResolveError::UnsupportedSrid(n));
            }
            Some(n)
        }
        None => None,
    };

    let resolved = match (crs_srid, explicit_srid) {
        (Some(a), Some(b)) if a != b => {
            return Err(SridResolveError::CrsSridConflict {
                crs: crs.unwrap().to_string(),
                srid: b,
            });
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
                return Err(SridResolveError::FamilyMismatch {
                    coordinates: family,
                    srid: if srid_geo {
                        PointKeyFamily::Geographic
                    } else {
                        PointKeyFamily::Cartesian
                    },
                });
            }
            // Dimensionality agreement.
            if srid_is_3d(s) != is_3d {
                return Err(SridResolveError::DimensionMismatch {
                    coordinates_3d: is_3d,
                    srid_3d: srid_is_3d(s),
                });
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
