//! CREATE VECTOR INDEX + db.index.vector.queryNodes / queryRelationships.

mod test_helpers;
use test_helpers::TestDb;

use std::collections::BTreeMap;

use lora_database::LoraValue;
use lora_store::{LoraVector, RawCoordinate, VectorCoordinateType};
use serde_json::Value as JsonValue;

fn index_named<'a>(rows: &'a [JsonValue], name: &str) -> Option<&'a JsonValue> {
    rows.iter()
        .find(|r| r.get("name").and_then(|v| v.as_str()) == Some(name))
}

fn names(rows: &[JsonValue]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect()
}

fn ordered_node_ids(rows: &[JsonValue]) -> Vec<i64> {
    rows.iter()
        .filter_map(|r| {
            r.get("node")
                .and_then(|n| n.get("id"))
                .and_then(|i| i.as_i64())
        })
        .collect()
}

// ---------- CREATE VECTOR INDEX DDL ----------

#[test]
fn create_vector_index_node_round_trip() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX movie_emb FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "movie_emb").expect("listed");
    assert_eq!(entry["type"], JsonValue::String("VECTOR".into()));
    assert_eq!(entry["entityType"], JsonValue::String("NODE".into()));
    assert_eq!(
        entry["labelsOrTypes"],
        JsonValue::Array(vec![JsonValue::String("Movie".into())])
    );
    assert_eq!(
        entry["properties"],
        JsonValue::Array(vec![JsonValue::String("embedding".into())])
    );
}

#[test]
fn create_vector_index_relationship_round_trip() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX rel_emb FOR ()-[r:CONTAINS]-() ON (r.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 3, `vector.similarity_function`: 'euclidean'}}",
    );
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "rel_emb").unwrap();
    assert_eq!(entry["type"], JsonValue::String("VECTOR".into()));
    assert_eq!(
        entry["entityType"],
        JsonValue::String("RELATIONSHIP".into())
    );
}

#[test]
fn show_vector_indexes_filter_now_returns_entries() {
    let db = TestDb::new();
    db.run("CREATE RANGE INDEX rng FOR (n:N) ON (n.x)");
    db.run(
        "CREATE VECTOR INDEX v1 FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    let listed = db.run("SHOW VECTOR INDEXES");
    assert_eq!(names(&listed), vec!["v1"]);
}

#[test]
fn vector_index_requires_index_config_options() {
    let db = TestDb::new();
    let err = db.run_err("CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.embedding)");
    assert!(
        err.contains("indexConfig"),
        "expected indexConfig error, got: {err}"
    );
}

#[test]
fn vector_index_rejects_invalid_dimensions() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 0, `vector.similarity_function`: 'cosine'}}",
    );
    assert!(
        err.contains("1..=4096"),
        "expected dimension bound error, got: {err}"
    );
}

#[test]
fn vector_index_rejects_unknown_similarity() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'manhattan'}}",
    );
    assert!(
        err.contains("similarity_function"),
        "expected similarity error, got: {err}"
    );
}

#[test]
fn vector_index_rejects_composite_properties() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.a, m.b) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    assert!(
        err.contains("single-property"),
        "expected single-property error, got: {err}"
    );
}

#[test]
fn vector_index_if_not_exists_is_idempotent() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX v FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    let rows = db.run(
        "CREATE VECTOR INDEX v IF NOT EXISTS FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    assert!(rows.is_empty());
}

#[test]
fn drop_vector_index() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX v FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'cosine'}}",
    );
    db.run("DROP INDEX v");
    let listed = db.run("SHOW INDEXES");
    assert!(index_named(&listed, "v").is_none());
}

// ---------- Procedure: db.index.vector.queryNodes ----------

fn create_index(db: &TestDb, sim: &str) {
    db.run(&format!(
        "CREATE VECTOR INDEX movie_emb FOR (m:Movie) ON (m.embedding) \
         OPTIONS {{indexConfig: {{`vector.dimensions`: 3, `vector.similarity_function`: '{sim}'}}}}",
    ));
}

fn seed_movies(db: &TestDb) {
    // Three vectors in 3D. Target [1,0,0] — closest first under cosine
    // similarity: identical, then near-axis, then perpendicular.
    db.run("CREATE (:Movie {title: 'A', embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {title: 'B', embedding: [0.9, 0.1, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {title: 'C', embedding: [0.0, 1.0, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {title: 'D'})"); // no embedding — should be ignored
    db.run("CREATE (:Other {embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)})");
    // wrong label
}

#[test]
fn vector_query_returns_top_k_in_descending_similarity() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    seed_movies(&db);
    let rows = db
        .run("CALL db.index.vector.queryNodes('movie_emb', 2, [1.0, 0.0, 0.0]) YIELD node, score");
    assert_eq!(rows.len(), 2, "expected top-2, got {rows:?}");
    let scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|v| v.as_f64()))
        .collect();
    assert!(scores[0] >= scores[1], "scores not descending: {scores:?}");
    // First hit is the identical vector.
    assert!((scores[0] - 1.0).abs() < 1e-6, "first score should be 1.0");
}

#[test]
fn vector_query_skips_entities_without_indexed_property() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    seed_movies(&db);
    let rows = db
        .run("CALL db.index.vector.queryNodes('movie_emb', 10, [1.0, 0.0, 0.0]) YIELD node, score");
    // 3 movies with embedding; the 4th Movie has no embedding, the Other
    // label is out of scope. So exactly 3 rows.
    assert_eq!(rows.len(), 3, "got {rows:?}");
}

#[test]
fn vector_query_supports_euclidean_similarity() {
    let db = TestDb::new();
    create_index(&db, "euclidean");
    seed_movies(&db);
    let rows = db
        .run("CALL db.index.vector.queryNodes('movie_emb', 1, [1.0, 0.0, 0.0]) YIELD node, score");
    assert_eq!(rows.len(), 1);
    let score = rows[0]["score"].as_f64().unwrap();
    assert!(
        (score - 1.0).abs() < 1e-6,
        "euclidean self-score = 1.0; got {score}"
    );
}

#[test]
fn vector_query_rejects_dimension_mismatch() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    seed_movies(&db);
    let err =
        db.run_err("CALL db.index.vector.queryNodes('movie_emb', 1, [1.0, 0.0]) YIELD node, score");
    assert!(
        err.contains("dimension"),
        "expected dimension mismatch, got: {err}"
    );
}

#[test]
fn vector_query_rejects_unknown_index() {
    let db = TestDb::new();
    let err =
        db.run_err("CALL db.index.vector.queryNodes('nope', 1, [1.0, 0.0, 0.0]) YIELD node, score");
    assert!(
        err.contains("no vector index"),
        "expected unknown-index error, got: {err}"
    );
}

#[test]
fn vector_query_rejects_wrong_index_kind() {
    let db = TestDb::new();
    db.run("CREATE RANGE INDEX rng FOR (n:N) ON (n.x)");
    let err =
        db.run_err("CALL db.index.vector.queryNodes('rng', 1, [1.0, 0.0, 0.0]) YIELD node, score");
    assert!(
        err.contains("not a VECTOR"),
        "expected wrong-kind error, got: {err}"
    );
}

#[test]
fn vector_query_rejects_unknown_yield_column() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    seed_movies(&db);
    let err =
        db.run_err("CALL db.index.vector.queryNodes('movie_emb', 1, [1.0, 0.0, 0.0]) YIELD title");
    assert!(
        err.contains("unknown column `title`"),
        "expected unknown YIELD column error, got: {err}"
    );
}

#[test]
fn vector_query_relationships_top_k() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX rel_emb FOR ()-[r:CONTAINS]-() ON (r.embedding) \
         OPTIONS {indexConfig: {`vector.dimensions`: 3, `vector.similarity_function`: 'cosine'}}",
    );
    db.run(
        "CREATE (a:Doc), (b:Doc), (c:Doc), \
         (a)-[:CONTAINS {embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)}]->(b), \
         (b)-[:CONTAINS {embedding: [0.9, 0.1, 0.0]::VECTOR<FLOAT32>(3)}]->(c)",
    );
    let rows = db.run(
        "CALL db.index.vector.queryRelationships('rel_emb', 2, [1.0, 0.0, 0.0]) \
         YIELD relationship, score",
    );
    assert_eq!(rows.len(), 2);
    let s0 = rows[0]["score"].as_f64().unwrap();
    let s1 = rows[1]["score"].as_f64().unwrap();
    assert!(s0 >= s1);
}

#[test]
fn vector_query_k_zero_rejected() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    seed_movies(&db);
    let err = db.run_err(
        "CALL db.index.vector.queryNodes('movie_emb', 0, [1.0, 0.0, 0.0]) YIELD node, score",
    );
    assert!(err.contains("k must be positive"), "got: {err}");
}

#[test]
fn vector_query_accepts_vector_arg() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    seed_movies(&db);
    let mut params = BTreeMap::new();
    params.insert(
        "q".to_string(),
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
        "CALL db.index.vector.queryNodes('movie_emb', 1, $q) YIELD node, score",
        params,
    );
    assert_eq!(rows.len(), 1);
    // Still ranks the identical vector first.
    assert!((rows[0]["score"].as_f64().unwrap() - 1.0).abs() < 1e-6);
}

#[test]
fn vector_query_top_k_orders_correctly_across_many() {
    let db = TestDb::new();
    create_index(&db, "cosine");
    // 5 vectors at decreasing similarity to [1,0,0].
    db.run("CREATE (:Movie {id: 1, embedding: [1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {id: 2, embedding: [0.8, 0.6, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {id: 3, embedding: [0.6, 0.8, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {id: 4, embedding: [0.0, 1.0, 0.0]::VECTOR<FLOAT32>(3)})");
    db.run("CREATE (:Movie {id: 5, embedding: [-1.0, 0.0, 0.0]::VECTOR<FLOAT32>(3)})");
    let rows = db
        .run("CALL db.index.vector.queryNodes('movie_emb', 5, [1.0, 0.0, 0.0]) YIELD node, score");
    // Read node ids by score order (descending).
    let ids = ordered_node_ids(&rows);
    assert!(!ids.is_empty(), "expected 5 rows, got {rows:?}");
}
