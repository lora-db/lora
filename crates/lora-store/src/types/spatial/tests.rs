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
    let err = resolve_srid(Some("cartesian"), None, PointKeyFamily::Cartesian, true).unwrap_err();
    assert!(err.contains("dimensionality"));
}

#[test]
fn resolve_srid_rejects_3d_crs_with_2d_coords() {
    let err = resolve_srid(Some("WGS-84-3D"), None, PointKeyFamily::Geographic, false).unwrap_err();
    assert!(err.contains("dimensionality"));
}

#[test]
fn resolve_srid_rejects_family_mismatch() {
    let err = resolve_srid(Some("cartesian"), None, PointKeyFamily::Geographic, false).unwrap_err();
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
