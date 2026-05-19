//! UNWIND-driven bulk-ingestion tests.
//!
//! Covers the playground's canonical "fill the graph from a range" pattern:
//! `UNWIND range(...) AS id CREATE (n {id, computed properties...})`. The
//! shapes here are exactly what a user pastes into the playground when
//! seeding a graph for exploration — Cypher-style function names, a CASE
//! expression in the property map, and a non-trivial per-row workload.

mod test_helpers;

use serde_json::Value as JsonValue;
use test_helpers::TestDb;

// ============================================================
// Cypher-compat aliases — these are the names a Neo4j user reaches for first
// and must resolve to the canonical namespaced builtin without the analyzer
// raising UnknownFunction.
// ============================================================

#[test]
fn range_alias_resolves_to_list_range() {
    let db = TestDb::new();
    let rows = db.run("UNWIND range(1, 5) AS k RETURN k");
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows.iter()
            .map(|r| r["k"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5],
    );
}

#[test]
fn range_alias_supports_step_argument() {
    let db = TestDb::new();
    let rows = db.run("UNWIND range(0, 10, 2) AS k RETURN k");
    let values: Vec<i64> = rows.iter().map(|r| r["k"].as_i64().unwrap()).collect();
    assert_eq!(values, vec![0, 2, 4, 6, 8, 10]);
}

#[test]
fn rand_alias_resolves_to_math_random() {
    let db = TestDb::new();
    let rows = db.run("RETURN rand() AS r");
    let r = rows[0]["r"].as_f64().unwrap();
    assert!((0.0..1.0).contains(&r), "rand() outside [0,1): {r}");
}

#[test]
fn datetime_alias_resolves_to_temporal_now() {
    let db = TestDb::new();
    let rows = db.run("RETURN datetime() AS t");
    // temporal.now() is rendered as a typed datetime object; the shape
    // and exact representation belong to temporal.rs — here we just assert
    // the alias resolved (no error) and produced a non-null value.
    assert!(!rows[0]["t"].is_null(), "datetime() returned null");
}

// ============================================================
// The playground's canonical UNWIND-CREATE pattern.
// ============================================================

#[test]
fn unwind_range_creates_nodes_with_properties() {
    let db = TestDb::new();
    db.run(
        "WITH range(1, 100) AS ids \
         UNWIND ids AS id \
         CREATE (n:TestRecord { \
            id: id, \
            name: 'Record ' + toString(id), \
            createdAt: datetime(), \
            randomValue: rand(), \
            status: CASE \
                WHEN id % 3 = 0 THEN 'ACTIVE' \
                WHEN id % 3 = 1 THEN 'PENDING' \
                ELSE 'ARCHIVED' \
            END \
         })",
    );

    db.assert_count("MATCH (n:TestRecord) RETURN n", 100);
}

#[test]
fn unwind_range_case_expression_distributes_across_buckets() {
    let db = TestDb::new();
    db.run(
        "UNWIND range(1, 30) AS id \
         CREATE (:Bucketed { \
            id: id, \
            status: CASE \
                WHEN id % 3 = 0 THEN 'ACTIVE' \
                WHEN id % 3 = 1 THEN 'PENDING' \
                ELSE 'ARCHIVED' \
            END \
         })",
    );

    // 30 ids, divisible by 3 → 10 in each bucket.
    let active = db
        .exec_count("MATCH (n:Bucketed {status:'ACTIVE'}) RETURN n")
        .unwrap();
    let pending = db
        .exec_count("MATCH (n:Bucketed {status:'PENDING'}) RETURN n")
        .unwrap();
    let archived = db
        .exec_count("MATCH (n:Bucketed {status:'ARCHIVED'}) RETURN n")
        .unwrap();
    assert_eq!((active, pending, archived), (10, 10, 10));
}

#[test]
fn unwind_range_tostring_concatenation_produces_unique_names() {
    let db = TestDb::new();
    db.run(
        "UNWIND range(1, 50) AS id \
         CREATE (:Named { id: id, name: 'Record ' + toString(id) })",
    );

    // Properties are unique per id, so name distinct count == row count.
    let names: Vec<JsonValue> = db.column("MATCH (n:Named) RETURN n.name AS name", "name");
    let mut set = std::collections::BTreeSet::new();
    for n in &names {
        set.insert(n.as_str().unwrap().to_string());
    }
    assert_eq!(set.len(), 50);
    assert!(set.contains("Record 1"));
    assert!(set.contains("Record 50"));
}

#[test]
fn unwind_range_returning_created_nodes_yields_one_row_per_id() {
    let db = TestDb::new();
    let rows = db.run(
        "UNWIND range(1, 25) AS id \
         CREATE (n:Returned { id: id }) \
         RETURN n.id AS id ORDER BY id",
    );
    assert_eq!(rows.len(), 25);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["id"].as_i64().unwrap() as usize, i + 1);
    }
}

// ============================================================
// Larger scale: the upper end of what the playground will plausibly accept
// in a single statement. Kept modest (10k) because the parser/analyzer
// recompile the entire UNWIND body per query — for true bulk loads the
// playground should batch in ~2k chunks, see scale.rs.
// ============================================================

#[test]
fn unwind_range_ten_thousand_nodes_single_statement() {
    let db = TestDb::new();
    db.run(
        "UNWIND range(1, 10000) AS id \
         CREATE (:BulkNode { id: id, kind: id % 7 })",
    );

    assert_eq!(db.service.node_count(), 10_000);

    // Spot-check a value in the middle and at the boundaries.
    let middle = db.scalar("MATCH (n:BulkNode {id: 5000}) RETURN n.kind");
    assert_eq!(middle.as_i64().unwrap(), 5000 % 7);
    let last = db.scalar("MATCH (n:BulkNode {id: 10000}) RETURN n.kind");
    assert_eq!(last.as_i64().unwrap(), 10_000 % 7);
}

#[test]
#[ignore = "100k-in-one-statement is slow; run with --ignored. Real bulk loads should batch."]
fn unwind_range_one_hundred_thousand_nodes_single_statement() {
    let db = TestDb::new();
    db.run(
        "WITH range(1, 100000) AS ids \
         UNWIND ids AS id \
         CREATE (n:TestRecord { \
            id: id, \
            name: 'Record ' + toString(id), \
            randomValue: rand(), \
            status: CASE \
                WHEN id % 3 = 0 THEN 'ACTIVE' \
                WHEN id % 3 = 1 THEN 'PENDING' \
                ELSE 'ARCHIVED' \
            END \
         })",
    );
    assert_eq!(db.service.node_count(), 100_000);
}

#[test]
fn batched_unwind_range_one_hundred_thousand_nodes() {
    // The pattern the playground should actually use for large ingest:
    // many small UNWIND statements rather than one huge one. Mirrors
    // scale.rs's approach but at a size that's reasonable for the
    // default `cargo test` run.
    let db = TestDb::new();
    const TOTAL: usize = 100_000;
    const BATCH: usize = 2_000;

    let mut i = 0;
    while i < TOTAL {
        let end = (i + BATCH).min(TOTAL);
        db.run(&format!(
            "UNWIND range({i}, {}) AS id \
             CREATE (:Bulk {{ id: id, kind: id % 5 }})",
            end - 1,
        ));
        i = end;
    }

    assert_eq!(db.service.node_count(), TOTAL);

    let buckets = db.run("MATCH (n:Bulk) RETURN n.kind AS k, count(n) AS c ORDER BY k");
    assert_eq!(buckets.len(), 5);
    for row in &buckets {
        assert_eq!(row["c"].as_i64().unwrap() as usize, TOTAL / 5);
    }
}
