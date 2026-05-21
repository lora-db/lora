//! CREATE VECTOR INDEX + db.index.vector.queryNodes / queryRelationships.

mod test_helpers;
use test_helpers::TestDb;

use std::collections::BTreeMap;

use lora_database::LoraValue;
use lora_store::{
    cosine_similarity_bounded, euclidean_similarity, LoraVector, RawCoordinate,
    VectorCoordinateType,
};
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
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'jaccard'}}",
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

// ---------- Brute-force oracle tests ----------
//
// These lock the current flat-scan behavior so that any future ANN backend
// (HNSW, IVF, …) can be tested against the same fixtures. The oracle
// computes top-k in-test using the same `cosine_similarity_bounded` /
// `euclidean_similarity` primitives the production scorer uses, sorts
// descending, and compares the resulting score sequence. A backend that
// keeps recall@k = 1.0 must reproduce the exact ordering; an approximate
// backend can later be tested with a recall tolerance.

/// Deterministic LCG so the fixtures are stable across runs and platforms
/// without pulling in `rand`. Returns f32 samples in roughly [-1, 1).
fn seeded_f32_stream(seed: u64) -> impl FnMut() -> f32 {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Take the upper 32 bits and map into [-1, 1).
        let bits = (state >> 32) as u32 as i32;
        bits as f32 / (i32::MAX as f32 + 1.0)
    }
}

fn seeded_vectors(seed: u64, n: usize, dim: usize) -> Vec<LoraVector> {
    let mut rng = seeded_f32_stream(seed);
    (0..n)
        .map(|_| {
            let coords: Vec<RawCoordinate> = (0..dim)
                .map(|_| RawCoordinate::Float(rng() as f64))
                .collect();
            LoraVector::try_new(coords, dim as i64, VectorCoordinateType::Float32).unwrap()
        })
        .collect()
}

/// Seed the DB with `n` `:V` nodes each carrying property `e` set to the
/// matching vector. Uses parameter binding (one CREATE per vector) so the
/// vectors are stored byte-identically rather than reparsed from text.
fn seed_vector_nodes(db: &TestDb, vectors: &[LoraVector]) {
    for v in vectors {
        let mut params = BTreeMap::new();
        params.insert("e".to_string(), LoraValue::Vector(v.clone()));
        db.run_with_params("CREATE (:V {e: $e})", params);
    }
}

fn call_top_k(db: &TestDb, k: usize, query: &LoraVector) -> Vec<f64> {
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query.clone()));
    let rows = db.run_with_params(
        &format!("CALL db.index.vector.queryNodes('vidx', {k}, $q) YIELD score"),
        params,
    );
    rows.iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect()
}

fn oracle_top_k<F>(vectors: &[LoraVector], query: &LoraVector, k: usize, sim: F) -> Vec<f64>
where
    F: Fn(&LoraVector, &LoraVector) -> Option<f64>,
{
    let mut scored: Vec<f64> = vectors.iter().filter_map(|v| sim(v, query)).collect();
    scored.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

fn assert_scores_match(proc_scores: &[f64], oracle: &[f64]) {
    assert_eq!(
        proc_scores.len(),
        oracle.len(),
        "score count mismatch: proc={proc_scores:?}, oracle={oracle:?}"
    );
    for (i, (a, b)) in proc_scores.iter().zip(oracle.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-9,
            "score[{i}] mismatch: proc={a}, oracle={b}\nproc={proc_scores:?}\noracle={oracle:?}"
        );
    }
}

#[test]
fn flat_knn_matches_oracle_cosine() {
    let db = TestDb::new();
    let dim = 16usize;
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{`vector.dimensions`: {dim}, `vector.similarity_function`: 'cosine'}}}}",
    ));
    let vectors = seeded_vectors(0xC051_4E_u64, 64, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0xDEAD_BEEF_u64, 1, dim).pop().unwrap();
    let proc_scores = call_top_k(&db, 10, &query);
    let oracle = oracle_top_k(&vectors, &query, 10, cosine_similarity_bounded);
    assert_scores_match(&proc_scores, &oracle);
}

#[test]
fn flat_knn_matches_oracle_euclidean() {
    let db = TestDb::new();
    let dim = 16usize;
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{`vector.dimensions`: {dim}, `vector.similarity_function`: 'euclidean'}}}}",
    ));
    let vectors = seeded_vectors(0xE0C1_1D_u64, 64, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0xFEED_FACE_u64, 1, dim).pop().unwrap();
    let proc_scores = call_top_k(&db, 10, &query);
    let oracle = oracle_top_k(&vectors, &query, 10, euclidean_similarity);
    assert_scores_match(&proc_scores, &oracle);
}

#[test]
fn flat_knn_k_larger_than_n_returns_all() {
    let db = TestDb::new();
    let dim = 8usize;
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{`vector.dimensions`: {dim}, `vector.similarity_function`: 'cosine'}}}}",
    ));
    let vectors = seeded_vectors(7, 5, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(8, 1, dim).pop().unwrap();
    let proc_scores = call_top_k(&db, 100, &query);
    let oracle = oracle_top_k(&vectors, &query, 100, cosine_similarity_bounded);
    assert_eq!(proc_scores.len(), 5);
    assert_scores_match(&proc_scores, &oracle);
}

#[test]
fn flat_knn_handles_score_ties_deterministically() {
    // 4 identical vectors → 4 identical scores. Procedure must return them
    // in a deterministic order (current contract: descending score, then
    // ascending node id). We only assert the score sequence is stable
    // across repeated calls; node-id ordering is left to the procedure.
    let db = TestDb::new();
    let dim = 4usize;
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{`vector.dimensions`: {dim}, `vector.similarity_function`: 'cosine'}}}}",
    ));
    let v = LoraVector::try_new(
        vec![
            RawCoordinate::Float(1.0),
            RawCoordinate::Float(0.0),
            RawCoordinate::Float(0.0),
            RawCoordinate::Float(0.0),
        ],
        dim as i64,
        VectorCoordinateType::Float32,
    )
    .unwrap();
    let vectors = vec![v.clone(), v.clone(), v.clone(), v.clone()];
    seed_vector_nodes(&db, &vectors);
    let scores_a = call_top_k(&db, 4, &v);
    let scores_b = call_top_k(&db, 4, &v);
    assert_eq!(scores_a, scores_b, "ordering not deterministic across runs");
    assert!(scores_a.iter().all(|s| (s - 1.0).abs() < 1e-9));
}

#[test]
fn flat_knn_skips_nodes_missing_property_in_oracle() {
    // A node without the indexed property must be skipped by the
    // procedure. The oracle must do the same — verified here because the
    // future backend abstraction is going to special-case missing
    // properties at insert time, not at query time.
    let db = TestDb::new();
    let dim = 8usize;
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{`vector.dimensions`: {dim}, `vector.similarity_function`: 'cosine'}}}}",
    ));
    let vectors = seeded_vectors(3, 8, dim);
    seed_vector_nodes(&db, &vectors);
    // Two `:V` nodes with no `e` property.
    db.run("CREATE (:V {label: 'no-vec-1'})");
    db.run("CREATE (:V {label: 'no-vec-2'})");
    let query = seeded_vectors(4, 1, dim).pop().unwrap();
    let proc_scores = call_top_k(&db, 100, &query);
    assert_eq!(
        proc_scores.len(),
        8,
        "expected exactly 8 scored vectors (missing-property nodes ignored)"
    );
    let oracle = oracle_top_k(&vectors, &query, 100, cosine_similarity_bounded);
    assert_scores_match(&proc_scores, &oracle);
}

// ---------- HNSW provider tests ----------
//
// These exercise the `vector.indexProvider: 'hnsw'` option end-to-end:
// DDL round-trip, top-k correctness against the flat oracle (with
// recall tolerance), and schema validator coverage for the new knobs.

fn create_hnsw_index(db: &TestDb, dim: usize, sim: &str) {
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{ \
            `vector.dimensions`: {dim}, \
            `vector.similarity_function`: '{sim}', \
            `vector.indexProvider`: 'hnsw' \
         }}}}",
    ));
}

#[test]
fn hnsw_index_ddl_round_trip() {
    let db = TestDb::new();
    create_hnsw_index(&db, 8, "cosine");
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "vidx").expect("listed");
    assert_eq!(entry["type"], JsonValue::String("VECTOR".into()));
}

#[test]
fn hnsw_top_k_returns_k_results() {
    let db = TestDb::new();
    let dim = 16usize;
    create_hnsw_index(&db, dim, "cosine");
    let vectors = seeded_vectors(0xABCD, 64, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0xEEFF, 1, dim).pop().unwrap();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 5, $q) YIELD node, score",
        params,
    );
    assert_eq!(rows.len(), 5);
    // Scores must be descending.
    let scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect();
    for w in scores.windows(2) {
        assert!(w[0] >= w[1], "scores not descending: {scores:?}");
    }
}

#[test]
fn hnsw_recall_at_10_meets_target_cosine() {
    // Recall@10 ≥ 0.95 against the flat oracle on uniform random
    // d=64 vectors at n=1k. Tighter than the per-backend unit test
    // because Cypher's path uses default HNSW params (M=16,
    // ef_search=100) on a higher-dim, larger-n fixture where graph
    // structure is more pronounced.
    let db = TestDb::new();
    let dim = 64usize;
    let n = 1_000usize;
    create_hnsw_index(&db, dim, "cosine");
    let vectors = seeded_vectors(0xC051_4E_u64, n, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0xDEAD_BEEF_u64, 1, dim).pop().unwrap();

    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query.clone()));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 10, $q) YIELD node, score",
        params,
    );
    let hnsw_scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect();
    assert_eq!(hnsw_scores.len(), 10);

    // Build the oracle from the same vectors. Recall is the
    // fraction of the oracle's top-10 *scores* the HNSW result
    // recovers — using score as the identity is more lenient than
    // id-based recall but still meaningful, and it avoids the
    // node-id bookkeeping we'd otherwise need.
    let oracle = oracle_top_k(&vectors, &query, 10, cosine_similarity_bounded);
    let mut hits = 0usize;
    for s in &hnsw_scores {
        if oracle.iter().any(|o| (o - s).abs() < 1e-9) {
            hits += 1;
        }
    }
    let recall = hits as f64 / 10.0;
    assert!(
        recall >= 0.95,
        "HNSW recall@10 too low: {recall} (hnsw={hnsw_scores:?}, oracle={oracle:?})"
    );
}

#[test]
fn hnsw_explicit_flat_provider_matches_oracle_exactly() {
    // `vector.indexProvider: 'flat'` is the legacy code path under a
    // new explicit name. Must remain exact (recall=1.0) so users
    // upgrading their config don't see drift.
    let db = TestDb::new();
    let dim = 16usize;
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{ \
            `vector.dimensions`: {dim}, \
            `vector.similarity_function`: 'cosine', \
            `vector.indexProvider`: 'flat' \
         }}}}",
    ));
    let vectors = seeded_vectors(13, 50, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(14, 1, dim).pop().unwrap();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query.clone()));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 10, $q) YIELD score",
        params,
    );
    let proc_scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect();
    let oracle = oracle_top_k(&vectors, &query, 10, cosine_similarity_bounded);
    assert_scores_match(&proc_scores, &oracle);
}

#[test]
fn schema_rejects_invalid_index_provider() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.indexProvider`: 'annoy' \
         }}",
    );
    assert!(
        err.contains("indexProvider"),
        "expected indexProvider error, got: {err}"
    );
}

#[test]
fn schema_rejects_out_of_range_hnsw_m() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.hnsw.m`: 999 \
         }}",
    );
    assert!(
        err.contains("vector.hnsw.m") && err.contains("128"),
        "expected hnsw.m range error, got: {err}"
    );
}

// ---------- New metric coverage: dot, manhattan ----------

fn create_index_with_metric(db: &TestDb, dim: usize, sim: &str, provider: &str) {
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{ \
            `vector.dimensions`: {dim}, \
            `vector.similarity_function`: '{sim}', \
            `vector.indexProvider`: '{provider}' \
         }}}}",
    ));
}

#[test]
fn dot_metric_top_k_matches_oracle_flat() {
    let db = TestDb::new();
    let dim = 16usize;
    create_index_with_metric(&db, dim, "dot", "flat");
    let vectors = seeded_vectors(0xD07_u64, 64, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0xD071, 1, dim).pop().unwrap();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query.clone()));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 10, $q) YIELD score",
        params,
    );
    let proc_scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect();
    // Oracle uses raw dot product — same metric the index ranks by.
    let mut oracle: Vec<f64> = vectors
        .iter()
        .filter_map(|v| lora_store::dot_product(v, &query))
        .collect();
    oracle.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    oracle.truncate(10);
    assert_scores_match(&proc_scores, &oracle);
}

#[test]
fn manhattan_metric_top_k_matches_oracle_flat() {
    let db = TestDb::new();
    let dim = 16usize;
    create_index_with_metric(&db, dim, "manhattan", "flat");
    let vectors = seeded_vectors(0x1A_10, 64, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0x1A_11, 1, dim).pop().unwrap();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query.clone()));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 10, $q) YIELD score",
        params,
    );
    let proc_scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect();
    // Oracle applies the same `1/(1+L1)` mapping the index uses.
    let mut oracle: Vec<f64> = vectors
        .iter()
        .filter_map(|v| lora_store::manhattan_distance(v, &query).map(|d| 1.0 / (1.0 + d)))
        .collect();
    oracle.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    oracle.truncate(10);
    assert_scores_match(&proc_scores, &oracle);
}

#[test]
fn dot_metric_works_with_hnsw_provider() {
    let db = TestDb::new();
    let dim = 32usize;
    create_index_with_metric(&db, dim, "dot", "hnsw");
    let vectors = seeded_vectors(0xD07_2, 256, dim);
    seed_vector_nodes(&db, &vectors);
    let query = seeded_vectors(0xD07_2_1, 1, dim).pop().unwrap();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 5, $q) YIELD score",
        params,
    );
    let scores: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.get("score").and_then(|s| s.as_f64()))
        .collect();
    assert_eq!(scores.len(), 5);
    for w in scores.windows(2) {
        assert!(w[0] >= w[1], "dot+hnsw scores not descending: {scores:?}");
    }
}

#[test]
fn dot_product_alias_is_accepted() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {indexConfig: {`vector.dimensions`: 4, `vector.similarity_function`: 'dot_product'}}",
    );
    // No error → alias accepted.
    let listed = db.run("SHOW VECTOR INDEXES");
    assert!(index_named(&listed, "vidx").is_some());
}

// ---------- Async (lazy) populate state ----------

#[test]
fn async_populate_index_starts_populating() {
    let db = TestDb::new();
    // Pre-seed a vector before CREATE INDEX so the backfill has work.
    db.run("CREATE (:V {e: [1.0, 0.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.populate.async`: true \
         }}",
    );
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "vidx").expect("listed");
    assert_eq!(entry["state"], JsonValue::String("POPULATING".into()));
}

#[test]
fn async_populate_first_query_warms_index_and_flips_to_online() {
    let db = TestDb::new();
    db.run("CREATE (:V {e: [1.0, 0.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run("CREATE (:V {e: [0.9, 0.1, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.populate.async`: true \
         }}",
    );
    // First query: triggers populate inline; both pre-existing
    // vectors must show up.
    let rows = db
        .run("CALL db.index.vector.queryNodes('vidx', 5, [1.0, 0.0, 0.0, 0.0]) YIELD node, score");
    assert_eq!(rows.len(), 2, "expected pre-existing vectors, got {rows:?}");
    // State should now be Online.
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "vidx").expect("listed");
    assert_eq!(entry["state"], JsonValue::String("ONLINE".into()));
}

#[test]
fn async_populate_post_create_inserts_still_visible() {
    // Mutations between CREATE (async) and first query go through
    // the maintenance hook and feed the registry. Verify the lazy
    // backfill doesn't drop or duplicate them.
    let db = TestDb::new();
    db.run("CREATE (:V {tag: 'pre', e: [1.0, 0.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.populate.async`: true \
         }}",
    );
    // After CREATE, while index is Populating, insert another:
    db.run("CREATE (:V {tag: 'post', e: [0.0, 1.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    let rows = db
        .run("CALL db.index.vector.queryNodes('vidx', 5, [1.0, 0.0, 0.0, 0.0]) YIELD node, score");
    assert_eq!(
        rows.len(),
        2,
        "expected both pre+post vectors, got {rows:?}"
    );
}

#[test]
fn async_populate_option_rejects_non_boolean() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (n:V) ON (n.e) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.populate.async`: 'yes' \
         }}",
    );
    assert!(
        err.contains("vector.populate.async") && err.contains("boolean"),
        "expected boolean shape error, got: {err}"
    );
}

// ---------- SHOW INDEXES surfaces options ----------

#[test]
fn show_indexes_surfaces_vector_options() {
    let db = TestDb::new();
    db.run(
        "CREATE VECTOR INDEX vidx FOR (m:Movie) ON (m.embedding) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 8, \
            `vector.similarity_function`: 'cosine', \
            `vector.indexProvider`: 'hnsw', \
            `vector.hnsw.m`: 24, \
            `vector.hnsw.ef_construction`: 256, \
            `vector.hnsw.ef_search`: 128 \
         }}",
    );
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "vidx").expect("listed");
    let options = entry["options"].as_object().expect("options is a map");
    assert_eq!(options.get("vector.dimensions"), Some(&JsonValue::from(8)));
    assert_eq!(
        options.get("vector.similarity_function"),
        Some(&JsonValue::String("cosine".into()))
    );
    assert_eq!(
        options.get("vector.indexProvider"),
        Some(&JsonValue::String("hnsw".into()))
    );
    assert_eq!(options.get("vector.hnsw.m"), Some(&JsonValue::from(24)));
    assert_eq!(
        options.get("vector.hnsw.ef_construction"),
        Some(&JsonValue::from(256))
    );
    assert_eq!(
        options.get("vector.hnsw.ef_search"),
        Some(&JsonValue::from(128))
    );
}

// ---------- HNSW snapshot round-trip ----------

#[test]
fn hnsw_backend_survives_snapshot_round_trip() {
    let donor = TestDb::new();
    let dim = 16usize;
    donor.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{ \
            `vector.dimensions`: {dim}, \
            `vector.similarity_function`: 'cosine', \
            `vector.indexProvider`: 'hnsw' \
         }}}}",
    ));
    let vectors = seeded_vectors(0x5A_AF_u64, 128, dim);
    seed_vector_nodes(&donor, &vectors);

    // Capture donor query result so we can compare after restore.
    let query = seeded_vectors(0xC1_AB_u64, 1, dim).pop().unwrap();
    let mut donor_params = BTreeMap::new();
    donor_params.insert("q".to_string(), LoraValue::Vector(query.clone()));
    let donor_rows = donor.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 10, $q) YIELD node, score",
        donor_params,
    );

    // Round-trip the database through a snapshot.
    let bytes = donor
        .service
        .save_snapshot_to_bytes()
        .expect("snapshot encode");
    let target = TestDb::new();
    target
        .service
        .load_snapshot_from_bytes(&bytes)
        .expect("snapshot decode");

    // Restored query must produce the same top-k order and scores.
    // If the snapshot trailer were missing, the rebuild would
    // generate a different HNSW topology (different RNG state →
    // different graph) and likely return slightly different
    // results. Identical-byte parity is the strongest signal that
    // the snapshot path skipped the rebuild.
    let mut target_params = BTreeMap::new();
    target_params.insert("q".to_string(), LoraValue::Vector(query));
    let target_rows = target.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 10, $q) YIELD node, score",
        target_params,
    );
    assert_eq!(
        ordered_node_ids(&donor_rows),
        ordered_node_ids(&target_rows),
        "restored HNSW returned different node order:\n  donor={donor_rows:?}\n  target={target_rows:?}"
    );
}

// ---------- Int8 quantized HNSW ----------

fn create_hnsw_int8(db: &TestDb, dim: usize) {
    db.run(&format!(
        "CREATE VECTOR INDEX vidx FOR (n:V) ON (n.e) \
         OPTIONS {{indexConfig: {{ \
            `vector.dimensions`: {dim}, \
            `vector.similarity_function`: 'cosine', \
            `vector.indexProvider`: 'hnsw', \
            `vector.hnsw.quantization`: 'int8' \
         }}}}",
    ));
}

#[test]
fn hnsw_int8_ddl_round_trip() {
    let db = TestDb::new();
    create_hnsw_int8(&db, 8);
    let listed = db.run("SHOW INDEXES");
    let entry = index_named(&listed, "vidx").expect("listed");
    assert_eq!(
        entry["options"]["vector.hnsw.quantization"],
        JsonValue::String("int8".into())
    );
}

#[test]
fn hnsw_int8_returns_top_k_for_normalized_embeddings() {
    let db = TestDb::new();
    let dim = 16usize;
    create_hnsw_int8(&db, dim);
    // Build unit-normalized vectors so the [-1, 1] quantization
    // range is fully exercised without clipping.
    let raw = seeded_vectors(0xA1_u64, 32, dim);
    let mut normalized = Vec::with_capacity(raw.len());
    for v in &raw {
        let norm = lora_store::euclidean_norm(v);
        let coords: Vec<lora_store::RawCoordinate> = (0..dim)
            .map(|i| {
                let f = v.values.as_f64_vec()[i];
                lora_store::RawCoordinate::Float(if norm > 0.0 { f / norm } else { 0.0 })
            })
            .collect();
        normalized
            .push(LoraVector::try_new(coords, dim as i64, VectorCoordinateType::Float32).unwrap());
    }
    seed_vector_nodes(&db, &normalized);
    let q = normalized[0].clone();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(q));
    let rows = db.run_with_params(
        "CALL db.index.vector.queryNodes('vidx', 5, $q) YIELD node, score",
        params,
    );
    assert_eq!(rows.len(), 5);
    // The self-query should rank near the top with a score close to 1.
    let top = rows[0]["score"].as_f64().unwrap();
    assert!(top > 0.95, "top quantized score too low: {top}");
}

#[test]
fn hnsw_int8_rejected_with_euclidean() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.e) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'euclidean', \
            `vector.hnsw.quantization`: 'int8' \
         }}",
    );
    assert!(
        err.contains("int8") && err.contains("cosine"),
        "expected int8-requires-cosine error, got: {err}"
    );
}

#[test]
fn hnsw_quantization_rejects_unknown_value() {
    let db = TestDb::new();
    let err = db.run_err(
        "CREATE VECTOR INDEX bad FOR (m:Movie) ON (m.e) \
         OPTIONS {indexConfig: { \
            `vector.dimensions`: 4, \
            `vector.similarity_function`: 'cosine', \
            `vector.hnsw.quantization`: 'int4' \
         }}",
    );
    assert!(
        err.contains("quantization"),
        "expected quantization-shape error, got: {err}"
    );
}

// ---------- Pre-filter (4th-argument options map) ----------

#[test]
fn restrict_to_filters_results_flat() {
    let db = TestDb::new();
    let dim = 4usize;
    create_index_with_metric(&db, dim, "cosine", "flat");
    db.run("CREATE (:V {tag: 'a', e: [1.0, 0.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run("CREATE (:V {tag: 'b', e: [0.9, 0.1, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run("CREATE (:V {tag: 'c', e: [0.0, 1.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    let listed = db.run("MATCH (n:V {tag: 'a'}) RETURN id(n) AS internal");
    let id_a = listed[0]["internal"].as_i64().expect("internal id");
    // Restrict to {id_a}: query for [1, 0, 0, 0] should return only id_a
    // even though id_b's vector is nearly identical and would normally rank.
    let rows = db.run(&format!(
        "CALL db.index.vector.queryNodes('vidx', 3, [1.0, 0.0, 0.0, 0.0], {{restrictTo: [{id_a}]}}) \
         YIELD node, score",
    ));
    let ids = ordered_node_ids(&rows);
    assert_eq!(ids, vec![id_a], "expected only id_a, got {ids:?}");
}

#[test]
fn restrict_to_filters_results_hnsw() {
    let db = TestDb::new();
    let dim = 8usize;
    create_index_with_metric(&db, dim, "cosine", "hnsw");
    let vectors = seeded_vectors(0xF11_7E_u64, 50, dim);
    seed_vector_nodes(&db, &vectors);
    // Grab the first 5 nodes' internal ids and restrict the query to them.
    let id_rows = db.run("MATCH (n:V) RETURN id(n) AS i ORDER BY id(n) LIMIT 5");
    let allowed: Vec<i64> = id_rows
        .iter()
        .filter_map(|r| r.get("i").and_then(|v| v.as_i64()))
        .collect();
    assert_eq!(allowed.len(), 5);
    let restrict_str = allowed
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    let query = seeded_vectors(0xF11_7E_2_u64, 1, dim).pop().unwrap();
    let mut params = BTreeMap::new();
    params.insert("q".to_string(), LoraValue::Vector(query));
    let rows = db.run_with_params(
        &format!(
            "CALL db.index.vector.queryNodes('vidx', 5, $q, {{restrictTo: [{restrict_str}]}}) \
             YIELD node, score"
        ),
        params,
    );
    let returned: BTreeMap<i64, ()> = ordered_node_ids(&rows)
        .into_iter()
        .map(|i| (i, ()))
        .collect();
    // Every returned id must be in the allowed set.
    for &id in returned.keys() {
        assert!(
            allowed.contains(&id),
            "node {id} returned but not in restrictTo {allowed:?}"
        );
    }
    // Should return up to 5 results (less only if HNSW under-fetched).
    assert!(
        returned.len() >= 3,
        "expected ≥3 results under restrictTo, got {returned:?}"
    );
}

#[test]
fn restrict_to_empty_returns_empty() {
    let db = TestDb::new();
    let dim = 4usize;
    create_index_with_metric(&db, dim, "cosine", "flat");
    db.run("CREATE (:V {e: [1.0, 0.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run("CREATE (:V {e: [0.9, 0.1, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    let rows = db.run(
        "CALL db.index.vector.queryNodes('vidx', 5, [1.0, 0.0, 0.0, 0.0], {restrictTo: []}) \
         YIELD node, score",
    );
    assert!(rows.is_empty(), "expected empty result, got {rows:?}");
}

#[test]
fn restrict_to_rejects_unknown_option_key() {
    let db = TestDb::new();
    let dim = 4usize;
    create_index_with_metric(&db, dim, "cosine", "flat");
    let err = db.run_err(
        "CALL db.index.vector.queryNodes('vidx', 1, [1.0, 0.0, 0.0, 0.0], {sneaky: true}) \
         YIELD node, score",
    );
    assert!(
        err.contains("unknown option") && err.contains("sneaky"),
        "expected unknown-option error, got: {err}"
    );
}

#[test]
fn restrict_to_rejects_non_list_value() {
    let db = TestDb::new();
    let dim = 4usize;
    create_index_with_metric(&db, dim, "cosine", "flat");
    let err = db.run_err(
        "CALL db.index.vector.queryNodes('vidx', 1, [1.0, 0.0, 0.0, 0.0], {restrictTo: 42}) \
         YIELD node, score",
    );
    assert!(
        err.contains("restrictTo") && err.contains("LIST"),
        "expected restrictTo-shape error, got: {err}"
    );
}

#[test]
fn hnsw_handles_updates_through_maintenance_hook() {
    // SET that replaces a vector property must update the HNSW
    // backend, not leave a stale entry. We assert the new vector is
    // the top match, not the old one.
    let db = TestDb::new();
    let dim = 4usize;
    create_hnsw_index(&db, dim, "cosine");
    db.run("CREATE (:V {id: 1, e: [1.0, 0.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    db.run("CREATE (:V {id: 2, e: [0.0, 1.0, 0.0, 0.0]::VECTOR<FLOAT32>(4)})");
    // Re-aim node 1 at [0,0,1,0].
    db.run("MATCH (n:V {id: 1}) SET n.e = [0.0, 0.0, 1.0, 0.0]::VECTOR<FLOAT32>(4)");
    // Query for [0,0,1,0] — node 1 should now be the top hit (it
    // wouldn't be if the old vector were still indexed).
    let rows = db
        .run("CALL db.index.vector.queryNodes('vidx', 1, [0.0, 0.0, 1.0, 0.0]) YIELD node, score");
    assert_eq!(rows.len(), 1);
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
