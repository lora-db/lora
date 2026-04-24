//! VECTOR value tests: construction, persistence, vector math, and
//! ordering/grouping behaviour. Covers the public surface documented in
//! `apps/loradb.com/docs/data-types/vectors.md` plus edge cases for
//! coordinate-type coercion.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::LoraValue;
use lora_store::{LoraVector, RawCoordinate, VectorCoordinateType};
use serde_json::json;
use test_helpers::TestDb;

// --- Construction: shape & coordinate types --------------------------------

#[test]
fn vector_integer_construction() {
    let v = TestDb::new().scalar("RETURN vector([1, 2, 3], 3, INTEGER) AS v");
    assert_eq!(v["kind"], "vector");
    assert_eq!(v["dimension"], 3);
    assert_eq!(v["coordinateType"], "INTEGER");
    assert_eq!(v["values"], json!([1, 2, 3]));
}

#[test]
fn vector_float_construction() {
    let v = TestDb::new().scalar("RETURN vector([1.05, 0.123, 5], 3, FLOAT32) AS v");
    assert_eq!(v["coordinateType"], "FLOAT32");
    assert_eq!(v["dimension"], 3);
    let arr = v["values"].as_array().unwrap();
    assert!((arr[0].as_f64().unwrap() - 1.05).abs() < 1e-5);
    assert!((arr[1].as_f64().unwrap() - 0.123).abs() < 1e-5);
    assert!((arr[2].as_f64().unwrap() - 5.0).abs() < 1e-5);
}

#[test]
fn vector_from_string_with_scientific_notation() {
    let v = TestDb::new().scalar("RETURN vector('[1.05e+00, 0.123, 5]', 3, FLOAT) AS v");
    assert_eq!(v["coordinateType"], "FLOAT64");
    let arr = v["values"].as_array().unwrap();
    assert!((arr[0].as_f64().unwrap() - 1.05).abs() < 1e-9);
}

#[test]
fn vector_with_string_coordinate_type() {
    let v = TestDb::new().scalar("RETURN vector([1, 2, 3], 3, 'INTEGER8') AS v");
    assert_eq!(v["coordinateType"], "INTEGER8");
    assert_eq!(v["values"], json!([1, 2, 3]));
}

#[test]
fn vector_accepts_signed_integer_alias_as_string() {
    let v = TestDb::new().scalar("RETURN vector([10], 1, 'SIGNED INTEGER') AS v");
    assert_eq!(v["coordinateType"], "INTEGER");
}

#[test]
fn vector_from_parameter_list() {
    let mut params = BTreeMap::new();
    params.insert(
        "values".into(),
        LoraValue::List(vec![
            LoraValue::Int(1),
            LoraValue::Int(2),
            LoraValue::Int(3),
            LoraValue::Int(4),
            LoraValue::Int(5),
        ]),
    );
    let rows = TestDb::new().run_with_params("RETURN vector($values, 5, INTEGER8) AS v", params);
    assert_eq!(rows[0]["v"]["dimension"], 5);
    assert_eq!(rows[0]["v"]["coordinateType"], "INTEGER8");
}

#[test]
fn vector_value_type_reports_vector() {
    let v = TestDb::new().scalar("RETURN valueType(vector([1,2,3], 3, INTEGER)) AS t");
    assert_eq!(v.as_str().unwrap(), "VECTOR<INTEGER>(3)");
}

// --- Validation ------------------------------------------------------------

#[test]
fn vector_dimension_zero_errors() {
    let err = TestDb::new().run_err("RETURN vector([], 0, INTEGER) AS v");
    assert!(err.contains("dimension"), "got: {err}");
}

#[test]
fn vector_dimension_over_max_errors() {
    let err = TestDb::new().run_err("RETURN vector([1], 5000, INTEGER) AS v");
    assert!(err.contains("dimension"), "got: {err}");
}

#[test]
fn vector_dimension_mismatch_errors() {
    let err = TestDb::new().run_err("RETURN vector([1,2,3], 2, INTEGER) AS v");
    assert!(err.contains("dimension"), "got: {err}");
}

#[test]
fn vector_int8_overflow_errors() {
    let err = TestDb::new().run_err("RETURN vector([128], 1, INT8) AS v");
    assert!(
        err.contains("range") || err.contains("INTEGER8"),
        "got: {err}"
    );
}

#[test]
fn vector_float_to_int_truncates() {
    let v = TestDb::new().scalar("RETURN vector([1.2, -2.9], 2, INT) AS v");
    assert_eq!(v["values"], json!([1, -2]));
}

#[test]
fn vector_int_to_float_is_allowed() {
    let v = TestDb::new().scalar("RETURN vector([3, 4], 2, FLOAT32) AS v");
    let arr = v["values"].as_array().unwrap();
    assert_eq!(arr[0].as_f64().unwrap(), 3.0);
    assert_eq!(arr[1].as_f64().unwrap(), 4.0);
}

#[test]
fn vector_nested_list_errors() {
    let err = TestDb::new().run_err("RETURN vector([[1,2]], 1, INTEGER) AS v");
    assert!(err.contains("nested") || err.contains("list"), "got: {err}");
}

#[test]
fn vector_unknown_coordinate_type_errors() {
    let err = TestDb::new().run_err("RETURN vector([1], 1, 'FLOAT128') AS v");
    assert!(err.contains("coordinate type"), "got: {err}");
}

#[test]
fn vector_null_value_returns_null() {
    let v = TestDb::new().scalar("RETURN vector(null, 3, FLOAT32) AS v");
    assert!(v.is_null());
}

#[test]
fn vector_null_dimension_returns_null() {
    let v = TestDb::new().scalar("RETURN vector([1,2,3], null, INTEGER8) AS v");
    assert!(v.is_null());
}

// --- Property storage -------------------------------------------------------

#[test]
fn vector_on_node_property_persists_and_returns() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, embedding: vector([1,2,3], 3, INTEGER)})");
    let v = db.scalar("MATCH (d:Doc {id: 1}) RETURN d.embedding AS e");
    assert_eq!(v["kind"], "vector");
    assert_eq!(v["coordinateType"], "INTEGER");
    assert_eq!(v["values"], json!([1, 2, 3]));
}

#[test]
fn vector_on_relationship_property_persists() {
    let db = TestDb::new();
    db.run("CREATE (:A {id: 1}), (:A {id: 2})");
    db.run(
        "MATCH (a:A {id:1}), (b:A {id:2}) \
         CREATE (a)-[:SIM {score: vector([0.1,0.2], 2, FLOAT32)}]->(b)",
    );
    let v = db.scalar("MATCH (:A)-[r:SIM]->(:A) RETURN r.score AS s");
    assert_eq!(v["dimension"], 2);
    assert_eq!(v["coordinateType"], "FLOAT32");
}

#[test]
fn vector_set_updates_property() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, embedding: vector([0,0], 2, INTEGER)})");
    db.run("MATCH (d:Doc {id:1}) SET d.embedding = vector([0.1, 0.2], 2, FLOAT32)");
    let v = db.scalar("MATCH (d:Doc {id:1}) RETURN d.embedding AS e");
    assert_eq!(v["dimension"], 2);
    assert_eq!(v["coordinateType"], "FLOAT32");
}

#[test]
fn vector_nested_in_list_property_rejected() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE (:Doc {embeddings: [vector([1,2,3], 3, INTEGER), vector([4,5,6], 3, INTEGER)]})",
    );
    assert!(
        err.contains("VECTOR") || err.contains("vector"),
        "got: {err}"
    );
}

// --- Conversion functions ---------------------------------------------------

#[test]
fn to_integer_list_roundtrip() {
    let v = TestDb::new().scalar("RETURN toIntegerList(vector([1.9, -1.9, 3], 3, FLOAT32)) AS l");
    assert_eq!(v, json!([1, -1, 3]));
}

#[test]
fn to_float_list_roundtrip() {
    let v = TestDb::new().scalar("RETURN toFloatList(vector([1, 2, 3], 3, INT8)) AS l");
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0].as_f64().unwrap(), 1.0);
}

#[test]
fn vector_dimension_count_matches_dimension() {
    let v = TestDb::new()
        .scalar("RETURN vector_dimension_count(vector([1, 2, 3], 3, INTEGER8)) AS size");
    assert_eq!(v, json!(3));
}

#[test]
fn size_of_vector_equals_dimension() {
    let v = TestDb::new().scalar("RETURN size(vector([1, 2, 3, 4], 4, FLOAT32)) AS s");
    assert_eq!(v, json!(4));
}

// --- Equality, grouping, and sorting ---------------------------------------

#[test]
fn vector_equality() {
    let v = TestDb::new()
        .scalar("RETURN vector([1,2,3], 3, INTEGER) = vector([1,2,3], 3, INTEGER) AS eq");
    assert_eq!(v, json!(true));
}

#[test]
fn vector_distinct_collapses_duplicates() {
    let db = TestDb::new();
    db.run("CREATE (:V {id: 1, e: vector([1,2], 2, INT)})");
    db.run("CREATE (:V {id: 2, e: vector([1,2], 2, INT)})");
    db.run("CREATE (:V {id: 3, e: vector([1,3], 2, INT)})");
    let rows = db.run("MATCH (v:V) RETURN DISTINCT v.e AS e");
    assert_eq!(rows.len(), 2);
}

// --- Vector math -----------------------------------------------------------

#[test]
fn cosine_similarity_identical_vectors() {
    let v = TestDb::new().scalar(
        "RETURN vector.similarity.cosine(vector([1,0,0], 3, FLOAT32), vector([1,0,0], 3, FLOAT32)) AS s",
    );
    assert!((v.as_f64().unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn cosine_similarity_accepts_list_inputs() {
    let v = TestDb::new().scalar("RETURN vector.similarity.cosine([1,0,0], [0,1,0]) AS s");
    // orthogonal => raw 0 => bounded 0.5
    assert!((v.as_f64().unwrap() - 0.5).abs() < 1e-6);
}

#[test]
fn cosine_similarity_on_zero_vector_is_null() {
    let v = TestDb::new().scalar(
        "RETURN vector.similarity.cosine(vector([0,0,0], 3, FLOAT32), vector([1,0,0], 3, FLOAT32)) AS s",
    );
    assert!(v.is_null());
}

#[test]
fn euclidean_similarity_matches_documented_example() {
    // From the docs: d² = (4-2)² + (5-8)² + (6-3)² = 22 ⇒ 1/23 ≈ 0.043478
    let v = TestDb::new().scalar(
        "RETURN vector.similarity.euclidean([4.0,5.0,6.0], vector([2.0,8.0,3.0], 3, FLOAT32)) AS s",
    );
    let sim = v.as_f64().unwrap();
    assert!((sim - 1.0 / 23.0).abs() < 1e-6, "got {sim}");
}

#[test]
fn vector_distance_euclidean() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1.0, 5.0, 3.0, 6.7], 4, FLOAT32), \
                                vector([5.0, 2.5, 3.1, 9.0], 4, FLOAT32), EUCLIDEAN) AS d",
    );
    let d = v.as_f64().unwrap();
    // sqrt(4² + 2.5² + 0.1² + 2.3²) = sqrt(16+6.25+0.01+5.29) = sqrt(27.55) ≈ 5.249
    assert!((d - 5.2488).abs() < 1e-3, "got {d}");
}

#[test]
fn vector_distance_euclidean_squared() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,0,0], 3, INTEGER8), \
                                vector([0,1,0], 3, INTEGER8), EUCLIDEAN_SQUARED) AS d",
    );
    assert!((v.as_f64().unwrap() - 2.0).abs() < 1e-6);
}

#[test]
fn vector_distance_manhattan() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,2,3], 3, INTEGER), \
                                vector([4,0,1], 3, INTEGER), MANHATTAN) AS d",
    );
    // |1-4| + |2-0| + |3-1| = 3 + 2 + 2 = 7
    assert!((v.as_f64().unwrap() - 7.0).abs() < 1e-6);
}

#[test]
fn vector_distance_cosine() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,2,3], 3, INTEGER8), \
                                vector([1,2,3], 3, INTEGER8), COSINE) AS d",
    );
    assert!((v.as_f64().unwrap() - 0.0).abs() < 1e-5);
}

#[test]
fn vector_distance_dot() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,2,3], 3, INTEGER), \
                                vector([4,5,6], 3, INTEGER), DOT) AS d",
    );
    // dot = 1*4 + 2*5 + 3*6 = 32 → DOT distance is -32
    assert!((v.as_f64().unwrap() - (-32.0)).abs() < 1e-5);
}

#[test]
fn vector_distance_hamming() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,2,3,4], 4, INTEGER8), \
                                vector([1,0,3,0], 4, INTEGER8), HAMMING) AS d",
    );
    assert!((v.as_f64().unwrap() - 2.0).abs() < 1e-6);
}

#[test]
fn vector_distance_requires_matching_dimensions() {
    let err = TestDb::new().run_err(
        "RETURN vector_distance(vector([1,2], 2, INTEGER), \
                                vector([1,2,3], 3, INTEGER), EUCLIDEAN) AS d",
    );
    assert!(err.contains("dimension"), "got: {err}");
}

#[test]
fn vector_norm_euclidean() {
    let v = TestDb::new()
        .scalar("RETURN vector_norm(vector([1.0, 5.0, 3.0, 6.7], 4, FLOAT32), EUCLIDEAN) AS n");
    // sqrt(1 + 25 + 9 + 44.89) = sqrt(79.89) ≈ 8.938
    let n = v.as_f64().unwrap();
    assert!((n - 8.938).abs() < 1e-3, "got {n}");
}

#[test]
fn vector_norm_manhattan() {
    let v = TestDb::new()
        .scalar("RETURN vector_norm(vector([1.0, -5.0, 3.0, -6.7], 4, FLOAT32), MANHATTAN) AS n");
    assert!((v.as_f64().unwrap() - 15.7).abs() < 1e-3);
}

// --- Exhaustive kNN via ORDER BY LIMIT --------------------------------------

#[test]
fn exhaustive_knn_ranking() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, embedding: vector([1.0, 0.0, 0.0], 3, FLOAT32)})");
    db.run("CREATE (:Doc {id: 2, embedding: vector([0.9, 0.1, 0.0], 3, FLOAT32)})");
    db.run("CREATE (:Doc {id: 3, embedding: vector([0.0, 1.0, 0.0], 3, FLOAT32)})");
    db.run("CREATE (:Doc {id: 4, embedding: vector([-1.0, 0.0, 0.0], 3, FLOAT32)})");

    let mut params = BTreeMap::new();
    params.insert(
        "query".into(),
        LoraValue::Vector(
            LoraVector::try_new(
                vec![
                    RawCoordinate::Float(1.0),
                    RawCoordinate::Float(0.0),
                    RawCoordinate::Float(0.0),
                ],
                3,
                VectorCoordinateType::Float32,
            )
            .unwrap(),
        ),
    );

    let rows = db.run_with_params(
        "MATCH (d:Doc) \
         RETURN d.id AS id \
         ORDER BY vector.similarity.cosine(d.embedding, $query) DESC LIMIT 3",
        params,
    );
    assert_eq!(rows.len(), 3);
    // Top match is the identical vector; second is the near-neighbour.
    assert_eq!(rows[0]["id"], 1);
    assert_eq!(rows[1]["id"], 2);
}

#[test]
fn exhaustive_knn_with_euclidean_similarity() {
    // Mirrors the documented Euclidean similarity example.
    let db = TestDb::new();
    db.run("CREATE (:Node {id: 1, vec: vector([4.0, 5.0, 6.0], 3, FLOAT32)})");
    db.run("CREATE (:Node {id: 2, vec: vector([2.0, 8.0, 3.0], 3, FLOAT32)})");
    db.run("CREATE (:Node {id: 3, vec: vector([10.0, 10.0, 10.0], 3, FLOAT32)})");

    let mut params = BTreeMap::new();
    params.insert(
        "query".into(),
        LoraValue::List(vec![
            LoraValue::Float(4.0),
            LoraValue::Float(5.0),
            LoraValue::Float(6.0),
        ]),
    );

    let rows = db.run_with_params(
        "MATCH (n:Node) \
         WITH n, vector.similarity.euclidean($query, n.vec) AS score \
         RETURN n.id AS id, score \
         ORDER BY score DESC LIMIT 2",
        params,
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], 1);
    assert!(rows[0]["score"].as_f64().unwrap() > rows[1]["score"].as_f64().unwrap());
}

// --- Additional construction coverage --------------------------------------

#[test]
fn vector_int8_construction_happy_path() {
    let v = TestDb::new().scalar("RETURN vector([-128, 0, 127], 3, INT8) AS v");
    assert_eq!(v["coordinateType"], "INTEGER8");
    assert_eq!(v["values"], json!([-128, 0, 127]));
}

#[test]
fn vector_string_with_nan_errors() {
    let err = TestDb::new().run_err("RETURN vector('[1.0, NaN, 3.0]', 3, FLOAT32) AS v");
    assert!(
        err.contains("NaN") || err.contains("finite") || err.contains("numeric"),
        "got: {err}"
    );
}

#[test]
fn vector_string_with_infinity_errors() {
    let err = TestDb::new().run_err("RETURN vector('[1.0, Infinity, 3.0]', 3, FLOAT32) AS v");
    assert!(
        err.contains("Infinity") || err.contains("finite") || err.contains("numeric"),
        "got: {err}"
    );
}

#[test]
fn vector_non_numeric_coordinate_errors() {
    let err = TestDb::new().run_err("RETURN vector([1, 'two', 3], 3, INTEGER) AS v");
    assert!(
        err.contains("numeric") || err.contains("string"),
        "got: {err}"
    );
}

#[test]
fn vector_rejects_double_alias() {
    // DOUBLE is not part of the public syntax — reject it explicitly so
    // callers don't assume a behaviour that isn't supported.
    let err = TestDb::new().run_err("RETURN vector([1.0, 2.0], 2, 'DOUBLE') AS v");
    assert!(err.contains("coordinate type"), "got: {err}");
}

// --- Additional property-storage coverage ---------------------------------

#[test]
fn vector_nested_in_map_list_property_rejected() {
    // A vector nested in a list that is itself buried inside a map must
    // still be rejected — the property validator walks the whole tree.
    let db = TestDb::new();
    let err = db.run_err("CREATE (:Doc {meta: {embeddings: [vector([1,2,3], 3, INTEGER)]}})");
    assert!(
        err.contains("VECTOR") || err.contains("vector"),
        "got: {err}"
    );
}

#[test]
fn map_containing_vector_directly_is_allowed_as_property() {
    // A map can contain a vector directly — it's only lists of vectors
    // that are rejected. (This is deliberate: the map value itself is
    // still stored as a structured property.)
    let db = TestDb::new();
    db.run("CREATE (:Doc {meta: {embedding: vector([1,2,3], 3, INTEGER)}})");
    let v = db.scalar("MATCH (d:Doc) RETURN d.meta AS m");
    let embedding = &v["embedding"];
    assert_eq!(embedding["kind"], "vector");
    assert_eq!(embedding["dimension"], 3);
}

// --- Cosine-similarity boundary behaviour ---------------------------------

#[test]
fn cosine_similarity_orthogonal_vectors_is_one_half() {
    let v = TestDb::new().scalar(
        "RETURN vector.similarity.cosine(vector([1,0,0], 3, FLOAT32), vector([0,1,0], 3, FLOAT32)) AS s",
    );
    assert!((v.as_f64().unwrap() - 0.5).abs() < 1e-6);
}

#[test]
fn cosine_similarity_opposite_vectors_is_zero() {
    let v = TestDb::new().scalar(
        "RETURN vector.similarity.cosine(vector([1,0,0], 3, FLOAT32), vector([-1,0,0], 3, FLOAT32)) AS s",
    );
    assert!(v.as_f64().unwrap().abs() < 1e-6);
}

#[test]
fn cosine_similarity_null_input_returns_null() {
    let v = TestDb::new()
        .scalar("RETURN vector.similarity.cosine(null, vector([1,0,0], 3, FLOAT32)) AS s");
    assert!(v.is_null());
}

#[test]
fn euclidean_similarity_null_input_returns_null() {
    let v = TestDb::new()
        .scalar("RETURN vector.similarity.euclidean(vector([1,0,0], 3, FLOAT32), null) AS s");
    assert!(v.is_null());
}

// --- Distance / metric plumbing -------------------------------------------

#[test]
fn vector_distance_accepts_string_metric() {
    // Bare identifier vs. quoted string must behave identically.
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,0,0], 3, INTEGER8), vector([0,1,0], 3, INTEGER8), 'EUCLIDEAN_SQUARED') AS d",
    );
    assert!((v.as_f64().unwrap() - 2.0).abs() < 1e-6);
}

#[test]
fn vector_distance_rejects_list_input() {
    // Unlike similarity functions, `vector_distance` requires VECTOR
    // values on both sides — passing a list should error loudly.
    let err = TestDb::new()
        .run_err("RETURN vector_distance([1,2,3], vector([1,2,3], 3, INTEGER), EUCLIDEAN) AS d");
    assert!(err.contains("VECTOR"), "got: {err}");
}

#[test]
fn vector_distance_unknown_metric_errors() {
    let err = TestDb::new().run_err(
        "RETURN vector_distance(vector([1,2,3], 3, INTEGER), vector([1,2,3], 3, INTEGER), BOGUS) AS d",
    );
    assert!(err.contains("metric"), "got: {err}");
}

#[test]
fn vector_distance_null_input_returns_null() {
    let v = TestDb::new()
        .scalar("RETURN vector_distance(null, vector([1,2,3], 3, INTEGER), EUCLIDEAN) AS d");
    assert!(v.is_null());
}

#[test]
fn vector_norm_unknown_metric_errors() {
    let err = TestDb::new().run_err("RETURN vector_norm(vector([1,2,3], 3, FLOAT32), COSINE) AS n");
    assert!(err.contains("metric"), "got: {err}");
}

#[test]
fn vector_norm_accepts_string_metric() {
    let v = TestDb::new()
        .scalar("RETURN vector_norm(vector([3.0, 4.0], 2, FLOAT32), 'EUCLIDEAN') AS n");
    assert!((v.as_f64().unwrap() - 5.0).abs() < 1e-4);
}

// --- Conversion functions: error paths ------------------------------------

#[test]
fn to_integer_list_rejects_non_vector() {
    let err = TestDb::new().run_err("RETURN toIntegerList([1, 2, 3]) AS l");
    assert!(err.contains("VECTOR"), "got: {err}");
}

#[test]
fn to_float_list_null_returns_null() {
    let v = TestDb::new().scalar("RETURN toFloatList(null) AS l");
    assert!(v.is_null());
}

#[test]
fn vector_dimension_count_rejects_non_vector() {
    let err = TestDb::new().run_err("RETURN vector_dimension_count([1, 2, 3]) AS n");
    assert!(err.contains("VECTOR"), "got: {err}");
}

// --- Exhaustive coordinate-type round-trip --------------------------------

#[test]
fn every_coordinate_type_round_trips_via_vector_function() {
    let db = TestDb::new();
    let cases: &[(&str, &str)] = &[
        ("FLOAT", "FLOAT64"),
        ("FLOAT64", "FLOAT64"),
        ("FLOAT32", "FLOAT32"),
        ("INTEGER", "INTEGER"),
        ("INT", "INTEGER"),
        ("INT64", "INTEGER"),
        ("INTEGER64", "INTEGER"),
        ("INTEGER32", "INTEGER32"),
        ("INT32", "INTEGER32"),
        ("INTEGER16", "INTEGER16"),
        ("INT16", "INTEGER16"),
        ("INTEGER8", "INTEGER8"),
        ("INT8", "INTEGER8"),
    ];
    for (alias, canonical) in cases {
        let q = format!("RETURN vector([1, 2, 3], 3, {alias}) AS v");
        let v = db.scalar(&q);
        assert_eq!(
            v["coordinateType"], *canonical,
            "alias {alias} should resolve to {canonical}, got {v}"
        );
        assert_eq!(v["dimension"], 3);
    }
}

#[test]
fn string_coordinate_aliases_are_case_and_whitespace_tolerant() {
    let db = TestDb::new();
    for alias in [
        "integer",
        " INTEGER ",
        "signed integer",
        "SIGNED  INTEGER",
        "Integer64",
    ] {
        let q = format!("RETURN vector([1], 1, '{alias}') AS v");
        let v = db.scalar(&q);
        assert_eq!(v["coordinateType"], "INTEGER", "alias {alias:?}");
    }
}

// --- Additional invalid construction --------------------------------------

#[test]
fn vector_negative_dimension_errors() {
    let err = TestDb::new().run_err("RETURN vector([1], -1, INTEGER) AS v");
    assert!(err.contains("dimension"), "got: {err}");
}

#[test]
fn vector_non_integer_dimension_errors() {
    let err = TestDb::new().run_err("RETURN vector([1], 1.5, INTEGER) AS v");
    assert!(err.contains("dimension"), "got: {err}");
}

#[test]
fn vector_null_coordinate_type_errors() {
    // A null coordinate type is never ambiguous; reject loudly rather
    // than silently returning null like value/dimension do.
    let err = TestDb::new().run_err("RETURN vector([1], 1, null) AS v");
    assert!(
        err.contains("coordinateType") || err.contains("coordinate type"),
        "got: {err}"
    );
}

#[test]
fn vector_value_of_wrong_type_errors() {
    let db = TestDb::new();
    let cases = [
        ("RETURN vector(true, 1, INTEGER) AS v", "LIST"),
        ("RETURN vector({x: 1}, 1, INTEGER) AS v", "LIST"),
        (
            "RETURN vector(vector([1], 1, INTEGER), 1, INTEGER) AS v",
            "LIST",
        ),
    ];
    for (q, needle) in cases {
        let err = db.run_err(q);
        assert!(
            err.contains(needle) || err.contains("STRING"),
            "query {q:?} got: {err}"
        );
    }
}

// --- List-of-vectors in queries (allowed in query, rejected in properties)

#[test]
fn list_literal_of_vectors_is_allowed_as_query_value() {
    let db = TestDb::new();
    let v = db.scalar("RETURN [vector([1], 1, INTEGER), vector([2], 1, INTEGER)] AS vectors");
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["kind"], "vector");
    assert_eq!(arr[1]["values"], json!([2]));
}

#[test]
fn collect_over_vector_properties_returns_list_of_vectors() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, embedding: vector([1,2,3], 3, INTEGER)})");
    db.run("CREATE (:Doc {id: 2, embedding: vector([4,5,6], 3, INTEGER)})");
    let rows = db.run("MATCH (d:Doc) RETURN collect(d.embedding) AS embs");
    let embs = rows[0]["embs"].as_array().unwrap();
    assert_eq!(embs.len(), 2);
    for e in embs {
        assert_eq!(e["kind"], "vector");
        assert_eq!(e["dimension"], 3);
    }
}

#[test]
fn unwind_vector_list_yields_vectors() {
    let db = TestDb::new();
    let rows = db.run(
        "UNWIND [vector([1], 1, INTEGER), vector([2], 1, INTEGER)] AS v \
         RETURN v",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["v"]["kind"], "vector");
    assert_eq!(rows[1]["v"]["values"], json!([2]));
}

#[test]
fn bulk_insert_via_unwind_of_parameter_list_of_maps() {
    // The canonical bulk-insert pattern: the application pre-builds a
    // list of maps, each with scalar fields + a tagged VECTOR, and feeds
    // it as a single parameter. UNWIND fans the list into per-row
    // CREATEs. Each embedding flows through property conversion as a
    // standalone vector (not a list entry), so the property rule is
    // satisfied.
    let db = TestDb::new();
    let make_vec = |vals: &[i64]| {
        LoraValue::Vector(
            LoraVector::try_new(
                vals.iter().map(|v| RawCoordinate::Int(*v)).collect(),
                vals.len() as i64,
                VectorCoordinateType::Integer8,
            )
            .unwrap(),
        )
    };

    let mut row1 = BTreeMap::new();
    row1.insert("id".into(), LoraValue::Int(1));
    row1.insert("title".into(), LoraValue::String("Onboarding".into()));
    row1.insert("embedding".into(), make_vec(&[1, 2, 3]));
    let mut row2 = BTreeMap::new();
    row2.insert("id".into(), LoraValue::Int(2));
    row2.insert("title".into(), LoraValue::String("Runbook".into()));
    row2.insert("embedding".into(), make_vec(&[4, 5, 6]));

    let mut params = BTreeMap::new();
    params.insert(
        "batch".into(),
        LoraValue::List(vec![LoraValue::Map(row1), LoraValue::Map(row2)]),
    );

    db.run_with_params(
        "UNWIND $batch AS row \
         CREATE (:Doc {id: row.id, title: row.title, embedding: row.embedding})",
        params,
    );

    let rows = db.run("MATCH (d:Doc) RETURN d.id AS id, d.embedding AS e ORDER BY id");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["e"]["coordinateType"], "INTEGER8");
    assert_eq!(rows[0]["e"]["values"], json!([1, 2, 3]));
    assert_eq!(rows[1]["e"]["values"], json!([4, 5, 6]));
}

// --- Property write paths -------------------------------------------------

#[test]
fn set_plus_equals_with_vector_is_stored() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1})");
    db.run("MATCH (d:Doc {id: 1}) SET d += {embedding: vector([1,2], 2, FLOAT32)}");
    let v = db.scalar("MATCH (d:Doc {id: 1}) RETURN d.embedding AS e");
    assert_eq!(v["coordinateType"], "FLOAT32");
    assert_eq!(v["dimension"], 2);
}

#[test]
fn set_replace_with_vector_map_is_stored() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, old: 'stale'})");
    // `SET d = {...}` replaces every property — include `id` in the new
    // map so we can still locate the node after the replacement.
    db.run("MATCH (d:Doc {id: 1}) SET d = {id: 1, embedding: vector([0.1, 0.2], 2, FLOAT64)}");
    let v = db.scalar("MATCH (d:Doc {id: 1}) RETURN d.embedding AS e");
    assert_eq!(v["coordinateType"], "FLOAT64");
    // `old` should no longer be a key on the node — inspect the full
    // property map via `properties()` so we don't trip the analyzer's
    // unknown-property check.
    let props = db.scalar("MATCH (d:Doc {id: 1}) RETURN properties(d) AS p");
    assert!(props.get("old").is_none(), "old should be gone: {props}");
    assert!(props.get("embedding").is_some());
}

#[test]
fn set_relationship_property_with_vector() {
    let db = TestDb::new();
    db.run("CREATE (:A {id: 1})-[:R]->(:A {id: 2})");
    db.run(
        "MATCH (:A {id: 1})-[r:R]->(:A {id: 2}) \
         SET r.score = vector([0.9, 0.1], 2, FLOAT32)",
    );
    let v = db.scalar("MATCH ()-[r:R]->() RETURN r.score AS s");
    assert_eq!(v["coordinateType"], "FLOAT32");
}

#[test]
fn vector_parameter_stored_as_node_property() {
    let db = TestDb::new();
    let mut params = BTreeMap::new();
    params.insert(
        "embedding".into(),
        LoraValue::Vector(
            LoraVector::try_new(
                vec![
                    lora_store::RawCoordinate::Float(0.5),
                    lora_store::RawCoordinate::Float(0.25),
                ],
                2,
                VectorCoordinateType::Float32,
            )
            .unwrap(),
        ),
    );
    db.run_with_params("CREATE (:Doc {id: 1, embedding: $embedding})", params);
    let v = db.scalar("MATCH (d:Doc {id: 1}) RETURN d.embedding AS e");
    assert_eq!(v["coordinateType"], "FLOAT32");
    assert_eq!(v["dimension"], 2);
}

#[test]
fn map_parameter_with_vector_value_is_stored() {
    // A map param that holds a vector directly (not in a list) is
    // allowed as a property value — mirrors the in-query behaviour.
    let db = TestDb::new();
    let vec_val = LoraValue::Vector(
        LoraVector::try_new(
            vec![
                lora_store::RawCoordinate::Int(1),
                lora_store::RawCoordinate::Int(2),
            ],
            2,
            VectorCoordinateType::Integer8,
        )
        .unwrap(),
    );
    let mut inner = BTreeMap::new();
    inner.insert("embedding".to_string(), vec_val);
    let mut params = BTreeMap::new();
    params.insert("meta".into(), LoraValue::Map(inner));
    db.run_with_params("CREATE (:Doc {id: 1, meta: $meta})", params);
    let m = db.scalar("MATCH (d:Doc {id: 1}) RETURN d.meta AS m");
    assert_eq!(m["embedding"]["coordinateType"], "INTEGER8");
}

#[test]
fn list_parameter_containing_vector_is_rejected_on_write() {
    let db = TestDb::new();
    let vec_val = LoraValue::Vector(
        LoraVector::try_new(
            vec![lora_store::RawCoordinate::Int(1)],
            1,
            VectorCoordinateType::Integer8,
        )
        .unwrap(),
    );
    let mut params = BTreeMap::new();
    params.insert("list".into(), LoraValue::List(vec![vec_val]));

    // Using run_with_params directly so we can assert the error.
    let res = db
        .service
        .execute_with_params(
            "CREATE (:Doc {id: 1, embeddings: $list})",
            Some(lora_database::ExecuteOptions {
                format: lora_database::ResultFormat::Rows,
            }),
            params,
        )
        .expect_err("should reject list-of-vectors property");
    let msg = res.to_string();
    assert!(
        msg.contains("VECTOR") || msg.contains("vector"),
        "got: {msg}"
    );
}

// --- Equality, DISTINCT, predicates ---------------------------------------

#[test]
fn vectors_with_same_values_but_different_coord_types_are_not_equal() {
    let v = TestDb::new()
        .scalar("RETURN vector([1,2,3], 3, INTEGER) = vector([1,2,3], 3, INTEGER8) AS eq");
    assert_eq!(v, json!(false));
}

#[test]
fn vectors_with_different_dimension_are_not_equal() {
    let v = TestDb::new()
        .scalar("RETURN vector([1,2], 2, INTEGER) = vector([1,2,3], 3, INTEGER) AS eq");
    assert_eq!(v, json!(false));
}

#[test]
fn where_equals_matches_stored_vector() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, e: vector([1,2,3], 3, INTEGER)})");
    db.run("CREATE (:Doc {id: 2, e: vector([4,5,6], 3, INTEGER)})");
    let rows = db.run("MATCH (d:Doc) WHERE d.e = vector([1,2,3], 3, INTEGER) RETURN d.id AS id");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], 1);
}

#[test]
fn where_not_equals_on_stored_vectors() {
    let db = TestDb::new();
    db.run("CREATE (:Doc {id: 1, e: vector([1,2], 2, INTEGER)})");
    db.run("CREATE (:Doc {id: 2, e: vector([3,4], 2, INTEGER)})");
    let mut ids: Vec<i64> = db
        .run("MATCH (d:Doc) WHERE d.e <> vector([1,2], 2, INTEGER) RETURN d.id AS id")
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    ids.sort();
    assert_eq!(ids, vec![2]);
}

#[test]
fn distinct_does_not_collapse_different_coord_types() {
    let db = TestDb::new();
    db.run("CREATE (:V {e: vector([1,2], 2, INTEGER)})");
    db.run("CREATE (:V {e: vector([1,2], 2, INTEGER8)})");
    let rows = db.run("MATCH (v:V) RETURN DISTINCT v.e AS e");
    assert_eq!(rows.len(), 2);
}

#[test]
fn order_by_on_vector_column_is_stable_and_does_not_panic() {
    // We don't assert a specific ordering (the order is implementation-
    // defined) — only that the engine emits a deterministic set of rows.
    let db = TestDb::new();
    db.run("CREATE (:V {id: 1, e: vector([1,2], 2, INTEGER)})");
    db.run("CREATE (:V {id: 2, e: vector([3,4], 2, INTEGER)})");
    db.run("CREATE (:V {id: 3, e: vector([1,2], 2, INTEGER)})");
    let rows = db.run("MATCH (v:V) RETURN v.id AS id ORDER BY v.e");
    assert_eq!(rows.len(), 3);
}

// --- Function null/error + metric edge cases ------------------------------

#[test]
fn to_integer_list_null_returns_null() {
    let v = TestDb::new().scalar("RETURN toIntegerList(null) AS l");
    assert!(v.is_null());
}

#[test]
fn vector_dimension_count_null_returns_null() {
    let v = TestDb::new().scalar("RETURN vector_dimension_count(null) AS n");
    assert!(v.is_null());
}

#[test]
fn size_null_returns_null() {
    // The existing contract for size() is null-propagating.
    let v = TestDb::new().scalar("RETURN size(null) AS s");
    assert!(v.is_null());
}

#[test]
fn vector_norm_null_input_returns_null() {
    let v = TestDb::new().scalar("RETURN vector_norm(null, EUCLIDEAN) AS n");
    assert!(v.is_null());
}

#[test]
fn vector_norm_null_metric_returns_null() {
    let v = TestDb::new().scalar("RETURN vector_norm(vector([1,2,3], 3, FLOAT32), null) AS n");
    assert!(v.is_null());
}

#[test]
fn vector_distance_null_in_second_slot_returns_null() {
    let v = TestDb::new()
        .scalar("RETURN vector_distance(vector([1,2,3], 3, INTEGER), null, EUCLIDEAN) AS d");
    assert!(v.is_null());
}

#[test]
fn vector_distance_null_metric_returns_null() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,2,3], 3, INTEGER), vector([1,2,3], 3, INTEGER), null) AS d",
    );
    assert!(v.is_null());
}

#[test]
fn vector_distance_metric_of_wrong_type_errors() {
    let db = TestDb::new();
    for bad in ["1", "[1,2]", "{k: 1}", "true"] {
        let q = format!(
            "RETURN vector_distance(vector([1,2], 2, INTEGER), vector([1,2], 2, INTEGER), {bad}) AS d"
        );
        let err = db.run_err(&q);
        assert!(err.contains("metric"), "query {q:?} got: {err}");
    }
}

#[test]
fn vector_norm_metric_is_case_insensitive() {
    // Lower-case metric string must be accepted — matches the similar
    // case-insensitive parsing used for coordinate-type strings.
    let v = TestDb::new()
        .scalar("RETURN vector_norm(vector([3.0, 4.0], 2, FLOAT32), 'euclidean') AS n");
    assert!((v.as_f64().unwrap() - 5.0).abs() < 1e-4);
}

#[test]
fn vector_distance_metric_is_case_insensitive() {
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1,0,0], 3, INTEGER), vector([0,1,0], 3, INTEGER), 'euclidean_squared') AS d",
    );
    assert!((v.as_f64().unwrap() - 2.0).abs() < 1e-6);
}

// --- Similarity: list rejection and coerced inputs ------------------------

#[test]
fn similarity_rejects_empty_list() {
    let err = TestDb::new().run_err("RETURN vector.similarity.cosine([], []) AS s");
    assert!(
        err.contains("empty") || err.contains("dimension") || err.contains("numeric"),
        "got: {err}"
    );
}

#[test]
fn similarity_rejects_non_numeric_list_entries() {
    let db = TestDb::new();
    for bad in [
        "vector.similarity.cosine([1, 'two', 3], [1, 2, 3])",
        "vector.similarity.cosine([1, null, 3], [1, 2, 3])",
        "vector.similarity.cosine([1, true, 3], [1, 2, 3])",
        "vector.similarity.cosine([1, [2], 3], [1, 2, 3])",
        "vector.similarity.cosine([1, {x: 1}, 3], [1, 2, 3])",
    ] {
        let q = format!("RETURN {bad} AS s");
        let err = db.run_err(&q);
        assert!(
            err.contains("numeric")
                || err.contains("nested")
                || err.contains("null")
                || err.contains("string"),
            "query {q:?} got: {err}"
        );
    }
}

#[test]
fn similarity_mixed_vector_and_list_input() {
    // The left side is a stored VECTOR, the right is a plain LIST — both
    // must feed the same similarity function.
    let db = TestDb::new();
    db.run("CREATE (:Doc {e: vector([1.0, 0.0, 0.0], 3, FLOAT32)})");
    let v = db.scalar("MATCH (d:Doc) RETURN vector.similarity.cosine(d.e, [1.0, 0.0, 0.0]) AS s");
    assert!((v.as_f64().unwrap() - 1.0).abs() < 1e-6);
}

// --- Math: additional distance metric coverage ----------------------------

#[test]
fn vector_distance_mixed_sign_floats() {
    // a = [1.5, -2.5, 0.5], b = [-0.5, 1.5, -1.5]
    // diffs = [2.0, -4.0, 2.0] → |diff| sum = 8, sum sq = 24
    let db = TestDb::new();
    let manhattan = db.scalar(
        "RETURN vector_distance(vector([1.5, -2.5, 0.5], 3, FLOAT32), vector([-0.5, 1.5, -1.5], 3, FLOAT32), MANHATTAN) AS d",
    );
    assert!((manhattan.as_f64().unwrap() - 8.0).abs() < 1e-4);
    let squared = db.scalar(
        "RETURN vector_distance(vector([1.5, -2.5, 0.5], 3, FLOAT32), vector([-0.5, 1.5, -1.5], 3, FLOAT32), EUCLIDEAN_SQUARED) AS d",
    );
    assert!((squared.as_f64().unwrap() - 24.0).abs() < 1e-3);
}

#[test]
fn vector_distance_hamming_on_float_vectors() {
    // 2 of 3 positions differ.
    let v = TestDb::new().scalar(
        "RETURN vector_distance(vector([1.0, 2.0, 3.0], 3, FLOAT32), vector([1.0, 2.5, 3.5], 3, FLOAT32), HAMMING) AS d",
    );
    assert!((v.as_f64().unwrap() - 2.0).abs() < 1e-9);
}

#[test]
fn vector_norm_on_integer_vector() {
    // [3, 4] → L2 = 5, L1 = 7.
    let db = TestDb::new();
    let l2 = db.scalar("RETURN vector_norm(vector([3, 4], 2, INTEGER), EUCLIDEAN) AS n");
    assert!((l2.as_f64().unwrap() - 5.0).abs() < 1e-4);
    let l1 = db.scalar("RETURN vector_norm(vector([3, 4], 2, INTEGER), MANHATTAN) AS n");
    assert!((l1.as_f64().unwrap() - 7.0).abs() < 1e-4);
}
