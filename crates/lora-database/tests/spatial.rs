//! Spatial tests — 3D, CRS/SRID inference & validation, and null propagation
//! for `point()`. Existing 2D happy-path tests live in `types_advanced.rs`;
//! this file covers the extended surface introduced alongside 3D support.

mod test_helpers;

use serde_json::json;
use test_helpers::TestDb;

// --- Cartesian 3D construction ---------------------------------------------

#[test]
fn point_cartesian_3d_from_xyz() {
    let v = TestDb::new().scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0}) AS p");
    assert_eq!(v["srid"], 9157);
    assert_eq!(v["x"], 1.0);
    assert_eq!(v["y"], 2.0);
    assert_eq!(v["z"], 3.0);
}

#[test]
fn point_cartesian_3d_from_crs_name() {
    let v =
        TestDb::new().scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0, crs: 'cartesian-3D'}) AS p");
    assert_eq!(v["srid"], 9157);
}

#[test]
fn point_cartesian_3d_from_explicit_srid() {
    let v = TestDb::new().scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0, srid: 9157}) AS p");
    assert_eq!(v["srid"], 9157);
}

#[test]
fn point_cartesian_3d_crs_and_srid_agree() {
    let v = TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0, crs: 'cartesian-3D', srid: 9157}) AS p");
    assert_eq!(v["srid"], 9157);
}

// --- WGS-84 3D construction -------------------------------------------------

#[test]
fn point_wgs84_3d_from_longitude_latitude_height() {
    let v =
        TestDb::new().scalar("RETURN point({longitude: 4.89, latitude: 52.37, height: 15.0}) AS p");
    assert_eq!(v["srid"], 4979);
    assert_eq!(v["x"], 4.89);
    assert_eq!(v["y"], 52.37);
    assert_eq!(v["z"], 15.0);
}

#[test]
fn point_wgs84_3d_with_z_alias_for_height() {
    let v = TestDb::new().scalar("RETURN point({longitude: 4.89, latitude: 52.37, z: 15.0}) AS p");
    assert_eq!(v["srid"], 4979);
    assert_eq!(v["z"], 15.0);
}

#[test]
fn point_wgs84_3d_from_explicit_srid() {
    let v = TestDb::new()
        .scalar("RETURN point({longitude: 4.89, latitude: 52.37, height: 15.0, srid: 4979}) AS p");
    assert_eq!(v["srid"], 4979);
}

#[test]
fn point_wgs84_3d_from_crs_name() {
    let v = TestDb::new().scalar(
        "RETURN point({longitude: 4.89, latitude: 52.37, height: 15.0, crs: 'WGS-84-3D'}) AS p",
    );
    assert_eq!(v["srid"], 4979);
}

// --- CRS / SRID inference & defaults ---------------------------------------

#[test]
fn point_2d_cartesian_default_srid_is_7203() {
    let v = TestDb::new().scalar("RETURN point({x: 1.0, y: 2.0}) AS p");
    assert_eq!(v["srid"], 7203);
    assert!(v.get("z").is_none(), "2D points must not carry a z field");
}

#[test]
fn point_2d_wgs84_default_srid_is_4326() {
    let v = TestDb::new().scalar("RETURN point({longitude: 4.89, latitude: 52.37}) AS p");
    assert_eq!(v["srid"], 4326);
}

#[test]
fn point_explicit_crs_only_resolves() {
    let v = TestDb::new()
        .scalar("RETURN point({longitude: 4.89, latitude: 52.37, crs: 'WGS-84-2D'}) AS p");
    assert_eq!(v["srid"], 4326);
}

#[test]
fn point_explicit_srid_only_resolves() {
    let v =
        TestDb::new().scalar("RETURN point({longitude: 4.89, latitude: 52.37, srid: 4326}) AS p");
    assert_eq!(v["srid"], 4326);
}

#[test]
fn point_crs_name_is_case_insensitive() {
    let v = TestDb::new()
        .scalar("RETURN point({longitude: 4.89, latitude: 52.37, crs: 'wgs-84'}) AS p");
    assert_eq!(v["srid"], 4326);
}

#[test]
fn point_wgs84_alias_without_2d_suffix() {
    // The bare alias "WGS-84" should resolve to the 2D SRID.
    let v = TestDb::new()
        .scalar("RETURN point({longitude: 4.89, latitude: 52.37, crs: 'WGS-84'}) AS p");
    assert_eq!(v["srid"], 4326);
}

// --- Null propagation ------------------------------------------------------

#[test]
fn point_null_argument_returns_null() {
    assert!(TestDb::new().scalar("RETURN point(null)").is_null());
}

#[test]
fn point_null_x_returns_null() {
    assert!(TestDb::new()
        .scalar("RETURN point({x: null, y: 2.0})")
        .is_null());
}

#[test]
fn point_null_latitude_returns_null() {
    assert!(TestDb::new()
        .scalar("RETURN point({longitude: 4.0, latitude: null})")
        .is_null());
}

#[test]
fn point_null_srid_returns_null() {
    assert!(TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0, srid: null})")
        .is_null());
}

#[test]
fn point_null_crs_returns_null() {
    assert!(TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0, crs: null})")
        .is_null());
}

// --- Validation failures ---------------------------------------------------

#[test]
fn point_missing_coordinates_errors() {
    let err = TestDb::new().run_err("RETURN point({})");
    assert!(err.contains("requires coordinates"), "got: {err}");
}

#[test]
fn point_missing_y_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0})");
    assert!(err.contains("missing y"), "got: {err}");
}

#[test]
fn point_missing_longitude_errors() {
    let err = TestDb::new().run_err("RETURN point({latitude: 52.0})");
    assert!(err.contains("missing longitude"), "got: {err}");
}

#[test]
fn point_mixed_families_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, latitude: 52.0})");
    assert!(err.contains("cannot mix"), "got: {err}");
}

#[test]
fn point_conflicting_crs_and_srid_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, y: 2.0, crs: 'cartesian', srid: 4326})");
    assert!(err.contains("do not agree"), "got: {err}");
}

#[test]
fn point_unknown_crs_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, y: 2.0, crs: 'mercator'})");
    assert!(err.contains("unsupported crs"), "got: {err}");
}

#[test]
fn point_unsupported_srid_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, y: 2.0, srid: 9999})");
    assert!(err.contains("unsupported srid"), "got: {err}");
}

#[test]
fn point_2d_crs_with_z_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, y: 2.0, z: 3.0, crs: 'cartesian'})");
    assert!(err.contains("dimensionality"), "got: {err}");
}

#[test]
fn point_3d_crs_without_z_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, y: 2.0, crs: 'cartesian-3D'})");
    assert!(err.contains("dimensionality"), "got: {err}");
}

#[test]
fn point_geographic_keys_with_cartesian_crs_errors() {
    let err =
        TestDb::new().run_err("RETURN point({longitude: 4.89, latitude: 52.37, crs: 'cartesian'})");
    assert!(err.contains("coordinates use"), "got: {err}");
}

#[test]
fn point_non_map_argument_errors() {
    let err = TestDb::new().run_err("RETURN point(42)");
    assert!(err.contains("requires a map"), "got: {err}");
}

#[test]
fn point_non_numeric_x_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 'hello', y: 2.0})");
    assert!(err.contains("must be numeric"), "got: {err}");
}

#[test]
fn point_unknown_key_errors() {
    let err = TestDb::new().run_err("RETURN point({x: 1.0, y: 2.0, elevation: 5.0})");
    assert!(err.contains("unknown key"), "got: {err}");
}

#[test]
fn point_z_and_height_together_errors() {
    let err = TestDb::new()
        .run_err("RETURN point({longitude: 4.0, latitude: 52.0, z: 1.0, height: 1.0})");
    assert!(err.contains("cannot specify both"), "got: {err}");
}

// --- Property access on points ---------------------------------------------

#[test]
fn point_3d_cartesian_property_access() {
    let db = TestDb::new();
    let p = "point({x: 1.0, y: 2.0, z: 3.0})";
    assert_eq!(db.scalar(&format!("RETURN {p}.x")), 1.0);
    assert_eq!(db.scalar(&format!("RETURN {p}.y")), 2.0);
    assert_eq!(db.scalar(&format!("RETURN {p}.z")), 3.0);
    assert_eq!(db.scalar(&format!("RETURN {p}.srid")), 9157);
    assert_eq!(db.scalar(&format!("RETURN {p}.crs")), "cartesian-3D");
}

#[test]
fn point_3d_wgs84_property_access() {
    let db = TestDb::new();
    let p = "point({longitude: 4.89, latitude: 52.37, height: 15.0})";
    assert!(
        (db.scalar(&format!("RETURN {p}.longitude"))
            .as_f64()
            .unwrap()
            - 4.89)
            .abs()
            < 1e-6
    );
    assert!((db.scalar(&format!("RETURN {p}.latitude")).as_f64().unwrap() - 52.37).abs() < 1e-6);
    assert_eq!(db.scalar(&format!("RETURN {p}.height")), 15.0);
    assert_eq!(db.scalar(&format!("RETURN {p}.srid")), 4979);
    assert_eq!(db.scalar(&format!("RETURN {p}.crs")), "WGS-84-3D");
}

#[test]
fn point_2d_z_access_returns_null() {
    assert!(TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0}).z")
        .is_null());
}

#[test]
fn point_cartesian_latitude_access_returns_null() {
    // Strict: cartesian points have no geographic projection.
    assert!(TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0}).latitude")
        .is_null());
    assert!(TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0}).longitude")
        .is_null());
    assert!(TestDb::new()
        .scalar("RETURN point({x: 1.0, y: 2.0}).height")
        .is_null());
}

#[test]
fn point_2d_crs_name() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN point({x: 1.0, y: 2.0}).crs"), "cartesian");
    assert_eq!(
        db.scalar("RETURN point({longitude: 4.0, latitude: 52.0}).crs"),
        "WGS-84-2D"
    );
}

// --- Distance in 3D --------------------------------------------------------

#[test]
fn distance_cartesian_3d() {
    // (0,0,0) -> (2,3,6) = sqrt(4+9+36) = 7
    let v = TestDb::new().scalar(
        "RETURN distance(point({x: 0.0, y: 0.0, z: 0.0}), point({x: 2.0, y: 3.0, z: 6.0}))",
    );
    assert!((v.as_f64().unwrap() - 7.0).abs() < 1e-6);
}

#[test]
fn distance_dimension_mismatch_returns_null_with_error() {
    // 2D vs 3D cartesian points have different SRIDs (7203 vs 9157), so
    // distance() returns null and sets an evaluation error.
    let err = TestDb::new()
        .run_err("RETURN distance(point({x: 0.0, y: 0.0}), point({x: 1.0, y: 2.0, z: 3.0}))");
    assert!(err.contains("different SRIDs"), "got: {err}");
}

#[test]
fn distance_wgs84_3d_ignores_height() {
    // Haversine is defined on the reference sphere; adding height to both
    // points must not change the surface distance.
    let db = TestDb::new();
    let d_2d = db.scalar(
        "RETURN distance(point({latitude: 52.37, longitude: 4.89}), \
                         point({latitude: 48.85, longitude: 2.35}))",
    );
    let d_3d = db.scalar(
        "RETURN distance(\
            point({latitude: 52.37, longitude: 4.89, height: 5000.0}), \
            point({latitude: 48.85, longitude: 2.35, height: 5000.0})\
         )",
    );
    assert!(
        (d_2d.as_f64().unwrap() - d_3d.as_f64().unwrap()).abs() < 1.0,
        "expected 2D and 3D haversine to agree (height ignored): 2d={d_2d}, 3d={d_3d}"
    );
}

// --- Round-trip through projection + storage -------------------------------

#[test]
fn point_3d_round_trip_through_return_json() {
    let v = TestDb::new().scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0}) AS p");
    assert_eq!(
        v,
        json!({
            "srid": 9157,
            "x": 1.0,
            "y": 2.0,
            "z": 3.0,
        })
    );
}

#[test]
fn point_3d_stored_on_node_and_read_back() {
    let db = TestDb::new();
    db.run(
        "CREATE (:Marker {name: 'alpha', \
                          pos: point({x: 1.0, y: 2.0, z: 3.0})})",
    );
    let rows = db.run("MATCH (m:Marker) RETURN m.pos AS pos");
    assert_eq!(rows.len(), 1);
    let pos = &rows[0]["pos"];
    assert_eq!(pos["srid"], 9157);
    assert_eq!(pos["z"], 3.0);
}

#[test]
fn point_3d_equality() {
    let db = TestDb::new();
    assert_eq!(
        db.scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0}) = point({x: 1.0, y: 2.0, z: 3.0})"),
        true
    );
    assert_eq!(
        db.scalar("RETURN point({x: 1.0, y: 2.0, z: 3.0}) = point({x: 1.0, y: 2.0, z: 4.0})"),
        false
    );
    // 2D and 3D with same x/y differ via SRID.
    assert_eq!(
        db.scalar("RETURN point({x: 1.0, y: 2.0}) = point({x: 1.0, y: 2.0, z: 0.0})"),
        false
    );
}
