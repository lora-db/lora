//! `vector(value, dimension, TYPE)` and one-argument `vector.coordinates`
//! (taste N-04): a vector built and taken apart inside Cypher, without a
//! binding parameter.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

#[test]
fn vector_builds_from_a_list_or_a_string_with_a_computed_dimension() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH [0.5, 1, 2] AS l \
         RETURN vector(l, size(l), FLOAT32) AS a, \
                vector('[1, 2, 3]', 3, INTEGER8) AS b, \
                vector.dimension(vector(l, size(l), 'FLOAT64')) AS d, \
                vector(l, 3, FLOAT32) = l::VECTOR<FLOAT32>(3) AS same_as_cast",
    );
    assert_eq!(rows[0]["d"], 3);
    assert_eq!(rows[0]["same_as_cast"], true);
    let coords = db.run("RETURN vector.coordinates(vector([0.5, 1, 2], 3, FLOAT32)) AS c");
    assert_eq!(coords[0]["c"], json!([0.5, 1.0, 2.0]));
}

#[test]
fn coordinates_without_a_type_keeps_the_vector_kind() {
    let db = TestDb::new();
    let rows = db.run(
        "RETURN vector.coordinates(vector([1, 2], 2, INT16)) AS ints, \
                vector.coordinates([1, 2]::VECTOR<FLOAT64>(2)) AS floats, \
                vector.coordinates(null) AS none",
    );
    assert_eq!(
        rows[0],
        json!({"ints": [1, 2], "floats": [1.0, 2.0], "none": null})
    );
}

#[test]
fn vector_round_trips_through_a_stored_property() {
    let db = TestDb::new();
    db.run("CREATE (:E {v: vector([0.25, 0.75], 2, FLOAT32)})");
    let rows = db.run(
        "MATCH (e:E) RETURN vector.coordinates(e.v) AS c, \
         vector.similarity(e.v, vector([0.25, 0.75], 2, FLOAT32)) AS s",
    );
    assert_eq!(rows[0]["c"], json!([0.25, 0.75]));
    assert_eq!(rows[0]["s"], 1.0);
}

#[test]
fn vector_nulls_and_errors() {
    let db = TestDb::new();
    let rows =
        db.run("RETURN vector(null, 3, FLOAT32) AS a, vector([1, 2, 3], null, FLOAT32) AS b");
    assert_eq!(rows[0], json!({"a": null, "b": null}));
    for bad in [
        "RETURN vector([1, 2, 3], 2, FLOAT32) AS v",
        "RETURN vector([1, 'x'], 2, FLOAT32) AS v",
        "RETURN vector([1, 2], 2, FLOAT16) AS v",
        "RETURN vector([1, 2], '2', FLOAT32) AS v",
        "RETURN vector([128], 1, INT8) AS v",
    ] {
        db.run_err(bad);
    }
}
