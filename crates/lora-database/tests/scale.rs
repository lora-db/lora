//! Super-large database correctness test.
//!
//! Builds a half-million-node graph with relationships to a smaller
//! category dimension, then exercises a handful of query shapes
//! (count, label scan, property filter, aggregation, traversal,
//! snapshot round-trip) to verify the engine produces correct results
//! at scale — not just at the 1k–10k sizes the rest of the suite uses.
//!
//! Marked `#[ignore]`: run explicitly with
//!
//! ```ignore
//! cargo test -p lora-database --test scale -- --ignored --nocapture
//! ```
//!
//! Expected wall time on a developer machine: ~30–90 seconds for the
//! build, plus a few seconds per query.

use std::time::Instant;

use lora_database::{Database, ExecuteOptions, ResultFormat};

const ITEM_COUNT: usize = 500_000;
const CATEGORY_COUNT: usize = 50_000;
/// One BELONGS_TO edge per item, so total edges == ITEM_COUNT.
const EDGE_COUNT: usize = ITEM_COUNT;
/// Bulk-insert chunk size — matches the bench fixtures. Small enough
/// to avoid pathological compile-time blowup on huge UNWIND ranges,
/// large enough that per-query overhead stays amortized.
const BULK_BATCH: usize = 2_000;

fn rows() -> Option<ExecuteOptions> {
    Some(ExecuteOptions {
        format: ResultFormat::Rows,
    })
}

fn first_row_int(json: &serde_json::Value, key: &str) -> i64 {
    let value = &json["rows"][0][key];
    value
        .as_i64()
        .unwrap_or_else(|| panic!("expected integer at rows[0].{key}, got {value}"))
}

fn time<R>(label: &str, f: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    eprintln!("  [{label}] {:?}", elapsed);
    result
}

fn build_super_large_graph(db: &Database<lora_database::InMemoryGraph>) {
    // 1. Categories: a small dimension table the items reference.
    let mut i = 0;
    while i < CATEGORY_COUNT {
        let end = (i + BULK_BATCH).min(CATEGORY_COUNT);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS k \
                 CREATE (:Category {{idx: k, name: 'cat_' + toString(k)}})",
                end - 1
            ),
            rows(),
        )
        .unwrap();
        i = end;
    }

    // 2. Items: the bulk of the graph, with an id, a value, and a kind
    // bucket (0..9) for aggregation tests.
    let mut i = 0;
    while i < ITEM_COUNT {
        let end = (i + BULK_BATCH).min(ITEM_COUNT);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS k \
                 CREATE (:Item {{id: k, value: k, kind: k % 10}})",
                end - 1
            ),
            rows(),
        )
        .unwrap();
        i = end;
    }

    // 3. Edges: each Item belongs to exactly one Category, picked by
    // item.id % CATEGORY_COUNT, so categories have ~ITEM_COUNT/CATEGORY_COUNT
    // (= 10) items each.
    let mut i = 0;
    while i < EDGE_COUNT {
        let end = (i + BULK_BATCH).min(EDGE_COUNT);
        db.execute(
            &format!(
                "UNWIND range({i}, {}) AS k \
                 MATCH (it:Item {{id: k}}), (c:Category {{idx: k % {CATEGORY_COUNT}}}) \
                 CREATE (it)-[:BELONGS_TO]->(c)",
                end - 1
            ),
            rows(),
        )
        .unwrap();
        i = end;
    }
}

#[test]
#[ignore = "super-large database build (~minute); run with --ignored"]
fn super_large_database_query_correctness() {
    let db = Database::in_memory();

    eprintln!(
        "building super-large graph: {} items, {} categories, {} edges",
        ITEM_COUNT, CATEGORY_COUNT, EDGE_COUNT
    );
    time("build", || build_super_large_graph(&db));

    // ---- counts via the storage surface (lock-free, no Cypher) ----
    assert_eq!(db.node_count(), ITEM_COUNT + CATEGORY_COUNT);
    assert_eq!(db.relationship_count(), EDGE_COUNT);

    // ---- count(*) via Cypher matches the storage surface ----
    let total_nodes = time("MATCH (n) RETURN count(n)", || {
        let result = db
            .execute("MATCH (n) RETURN count(n) AS c", rows())
            .unwrap();
        first_row_int(&serde_json::to_value(&result).unwrap(), "c")
    });
    assert_eq!(total_nodes as usize, ITEM_COUNT + CATEGORY_COUNT);

    // ---- label scan: count Items via label index ----
    let item_count = time("MATCH (n:Item) RETURN count(n)", || {
        let result = db
            .execute("MATCH (n:Item) RETURN count(n) AS c", rows())
            .unwrap();
        first_row_int(&serde_json::to_value(&result).unwrap(), "c")
    });
    assert_eq!(item_count as usize, ITEM_COUNT);

    // ---- property equality: exactly one item per id ----
    let target = ITEM_COUNT - 1; // last id, exercises the full id space
    let by_id = time("MATCH (n:Item {id: target}) RETURN n.value", || {
        db.execute(
            &format!("MATCH (n:Item {{id: {target}}}) RETURN n.value AS v"),
            rows(),
        )
        .unwrap()
    });
    let json = serde_json::to_value(&by_id).unwrap();
    assert_eq!(json["rows"].as_array().unwrap().len(), 1);
    assert_eq!(first_row_int(&json, "v") as usize, target);

    // ---- range filter: count items in a 10% window ----
    let lower = ITEM_COUNT / 10;
    let upper = lower * 2;
    let in_range = time("range filter [10%, 20%]", || {
        let result = db
            .execute(
                &format!(
                    "MATCH (n:Item) WHERE n.value >= {lower} AND n.value < {upper} \
                     RETURN count(n) AS c"
                ),
                rows(),
            )
            .unwrap();
        first_row_int(&serde_json::to_value(&result).unwrap(), "c")
    });
    assert_eq!(in_range as usize, upper - lower);

    // ---- aggregation: 10 buckets via kind = id % 10, each ~ITEM_COUNT/10 ----
    let buckets = time("aggregation by kind", || {
        db.execute(
            "MATCH (n:Item) RETURN n.kind AS kind, count(n) AS c ORDER BY kind",
            rows(),
        )
        .unwrap()
    });
    let json = serde_json::to_value(&buckets).unwrap();
    let bucket_rows = json["rows"].as_array().unwrap();
    assert_eq!(bucket_rows.len(), 10, "expected exactly 10 kind buckets");
    let mut sum = 0i64;
    for (i, row) in bucket_rows.iter().enumerate() {
        assert_eq!(
            row["kind"].as_i64().unwrap() as usize,
            i,
            "buckets must come back in ORDER BY kind ascending"
        );
        let count = row["c"].as_i64().unwrap();
        sum += count;
        // Each bucket must be exactly ITEM_COUNT / 10 since ITEM_COUNT
        // is divisible by 10 in this test.
        assert_eq!(count as usize, ITEM_COUNT / 10);
    }
    assert_eq!(sum as usize, ITEM_COUNT);

    // ---- traversal: pick one category, count items belonging to it ----
    let cat_idx = CATEGORY_COUNT / 2; // arbitrary inner index
    let expected_per_cat = ITEM_COUNT / CATEGORY_COUNT;
    let traversal_count = time("incoming-edge count for one category", || {
        let result = db
            .execute(
                &format!(
                    "MATCH (it:Item)-[:BELONGS_TO]->(c:Category {{idx: {cat_idx}}}) \
                     RETURN count(it) AS c"
                ),
                rows(),
            )
            .unwrap();
        first_row_int(&serde_json::to_value(&result).unwrap(), "c")
    });
    assert_eq!(
        traversal_count as usize, expected_per_cat,
        "each category should have exactly {expected_per_cat} items"
    );

    // ---- top-N over the whole item set: a sort that touches every row ----
    let top = time("ORDER BY value DESC LIMIT 5", || {
        db.execute(
            "MATCH (n:Item) RETURN n.id AS id ORDER BY n.value DESC LIMIT 5",
            rows(),
        )
        .unwrap()
    });
    let json = serde_json::to_value(&top).unwrap();
    let top_rows = json["rows"].as_array().unwrap();
    assert_eq!(top_rows.len(), 5);
    for (i, row) in top_rows.iter().enumerate() {
        // Top 5 by value descending: id ITEM_COUNT-1 .. ITEM_COUNT-5
        assert_eq!(
            row["id"].as_i64().unwrap() as usize,
            ITEM_COUNT - 1 - i,
            "top-N row {i} should be id {}",
            ITEM_COUNT - 1 - i
        );
    }

    eprintln!(
        "all assertions passed against {}-node / {}-edge graph",
        db.node_count(),
        db.relationship_count()
    );
}
