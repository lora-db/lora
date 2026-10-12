//! ORDER BY after an aggregating or DISTINCT WITH / RETURN may read any
//! expression over the grouping keys: a property of a grouped node,
//! relationship or map sorts by that property. Whatever else a key reads
//! from before the projection has no single value per group and is
//! rejected.
//!
//! Regression: `WITH n, count(m) AS c ORDER BY c DESC, n.key` evaluated
//! `n.key` on the grouped row, where the pre-aggregation `n` no longer
//! exists, so the key was null for every row and the sort did nothing. A
//! ranked list came back with its ties in creation order. Every case runs on
//! each execution path (`execute`, the pull pipeline, both transaction
//! modes).

mod test_helpers;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat, Row, TransactionMode};
use serde_json::Value as JsonValue;
use test_helpers::TestDb;

#[derive(Clone, Copy, Debug)]
enum Path {
    Execute,
    Stream,
    TxReadWrite,
    TxReadOnly,
}

const READ_PATHS: [Path; 4] = [
    Path::Execute,
    Path::Stream,
    Path::TxReadWrite,
    Path::TxReadOnly,
];

fn rows_json(rows: Vec<Row>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}

fn run_on(db: &Database<InMemoryGraph>, path: Path, query: &str) -> Result<Vec<JsonValue>, String> {
    match path {
        Path::Execute => {
            let result = db
                .execute(
                    query,
                    Some(ExecuteOptions {
                        format: ResultFormat::Rows,
                    }),
                )
                .map_err(|e| e.to_string())?;
            let json = serde_json::to_value(result).unwrap();
            Ok(json
                .get("rows")
                .and_then(JsonValue::as_array)
                .cloned()
                .unwrap_or_default())
        }
        Path::Stream => {
            let mut stream = db.stream(query).map_err(|e| e.to_string())?;
            let mut rows = Vec::new();
            while let Some(row) = stream.next_row().map_err(|e| e.to_string())? {
                rows.push(row);
            }
            stream.finish().map_err(|e| e.to_string())?;
            Ok(rows_json(rows))
        }
        Path::TxReadWrite | Path::TxReadOnly => {
            let mode = if matches!(path, Path::TxReadOnly) {
                TransactionMode::ReadOnly
            } else {
                TransactionMode::ReadWrite
            };
            let mut tx = db.begin_transaction(mode).map_err(|e| e.to_string())?;
            let rows = tx.execute_rows(query).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(rows_json(rows))
        }
    }
}

/// Four `:N` created in the order d, a, c, b, each with one `:R` to an `:M`.
fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(
        "UNWIND ['d', 'a', 'c', 'b'] AS k \
         CREATE (:N {key: k, g: 1})-[:R {since: k}]->(:M {key: k})",
    );
    db
}

fn assert_keys(query: &str, expected: &str) {
    for path in READ_PATHS {
        let db = seeded();
        let got = run_on(&db.service, path, query)
            .unwrap_or_else(|e| panic!("{path:?}: `{query}` failed: {e}"));
        let keys: String = got
            .iter()
            .map(|row| row["key"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(keys, expected, "{path:?}: `{query}`");
    }
}

fn assert_rejected(query: &str, needle: &str) {
    for path in READ_PATHS {
        let db = seeded();
        let err = run_on(&db.service, path, query)
            .expect_err(&format!("{path:?}: `{query}` should fail"));
        assert!(err.contains(needle), "{path:?}: `{query}`: {err}");
    }
}

#[test]
fn a_property_of_a_grouping_node_breaks_ties() {
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) WITH n, count(m) AS c ORDER BY c DESC, n.key ASC \
         RETURN n.key AS key",
        "abcd",
    );
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) WITH n, count(m) AS c ORDER BY n.key DESC \
         RETURN n.key AS key",
        "dcba",
    );
}

#[test]
fn the_same_through_return() {
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) RETURN n, n.key AS key, count(m) AS c ORDER BY n.g, n.key DESC",
        "dcba",
    );
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) RETURN n.key AS key, count(m) AS c ORDER BY toUpper(n.key)",
        "abcd",
    );
}

#[test]
fn a_property_of_a_grouping_relationship_or_map() {
    assert_keys(
        "MATCH (:N)-[r:R]->(m:M) WITH r, count(m) AS c ORDER BY r.since \
         RETURN r.since AS key",
        "abcd",
    );
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) WITH {key: n.key} AS mp, count(m) AS c ORDER BY mp.key DESC \
         RETURN mp.key AS key",
        "dcba",
    );
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) WITH m, count(n) AS c ORDER BY c, m.key DESC \
         RETURN m.key AS key",
        "dcba",
    );
}

#[test]
fn a_key_mixing_a_grouping_property_and_an_aggregate() {
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) WITH n, count(m) AS c ORDER BY n.key + toString(count(m)) DESC \
         RETURN n.key AS key",
        "dcba",
    );
}

#[test]
fn distinct_sorts_by_a_property_of_a_projected_node() {
    assert_keys(
        "MATCH (n:N) WITH DISTINCT n ORDER BY n.key RETURN n.key AS key",
        "abcd",
    );
    assert_keys(
        "MATCH (n:N) RETURN DISTINCT n, n.key AS key ORDER BY n.key DESC",
        "dcba",
    );
}

#[test]
fn a_key_reading_an_ungrouped_binding_is_rejected() {
    assert_rejected(
        "MATCH (n:N)-[:R]->(m:M) WITH n, count(m) AS c ORDER BY m.key RETURN n.key AS key",
        "grouping keys",
    );
    assert_rejected(
        "MATCH (n:N)-[:R]->(m:M) RETURN DISTINCT n.key AS key ORDER BY m.key",
        "after DISTINCT",
    );
}

#[test]
fn an_alias_named_like_an_earlier_binding_sorts_by_the_alias() {
    // `n` in ORDER BY is the projected key, not the node it was read from.
    assert_keys(
        "MATCH (n:N)-[:R]->(m:M) RETURN n.key AS n, n.key AS key, count(m) AS c ORDER BY c DESC, n",
        "abcd",
    );
    assert_keys(
        "MATCH (n:N) RETURN DISTINCT n.key AS n, n.key AS key ORDER BY n DESC",
        "dcba",
    );
}

#[test]
fn a_plain_projection_still_sorts_by_earlier_bindings() {
    assert_keys(
        "MATCH (n:N) WITH n.key AS key ORDER BY n.key DESC RETURN key",
        "dcba",
    );
    assert_keys(
        "MATCH (n:N) RETURN n.g AS key2, n.key AS key ORDER BY n.key",
        "abcd",
    );
}
