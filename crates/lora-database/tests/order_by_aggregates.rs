//! ORDER BY after an aggregating RETURN / WITH sorts by aggregate values.
//! A key that restates a projected item (as written) sorts by that item's
//! column; an aggregate in any other key is folded per group into a hidden
//! column that lives until the sort and never reaches the output. ORDER BY
//! can't aggregate when the projection doesn't, and after DISTINCT it can
//! only sort by an aggregate that is projected (as in openCypher).
//!
//! Regression: a restated nested aggregate (`ORDER BY size(collect(x))`)
//! was evaluated on the grouped row and sorted nothing, and
//! `RETURN n.g ORDER BY count(*)` silently skipped aggregation and
//! returned every input row. Every case runs on each execution path
//! (`execute`, the pull pipeline, both transaction modes).

mod test_helpers;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat, Row, TransactionMode};
use serde_json::{json, Value as JsonValue};
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

/// Groups by `g`: g=0 has 1 node (i=1), g=1 has 3 (i=2,3,4), g=2 has 2 (i=5,6).
fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(
        "UNWIND [[1, 0], [2, 1], [3, 1], [4, 1], [5, 2], [6, 2]] AS p \
         CREATE (:N {i: p[0], g: p[1]})",
    );
    db
}

fn assert_read(query: &str, expected: JsonValue) {
    for path in READ_PATHS {
        let db = seeded();
        let got = run_on(&db.service, path, query)
            .unwrap_or_else(|e| panic!("{path:?}: `{query}` failed: {e}"));
        assert_eq!(JsonValue::Array(got), expected, "{path:?}: `{query}`");
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
fn a_restated_nested_aggregate_sorts_by_its_column() {
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, size(collect(n.i)) AS c ORDER BY size(collect(n.i)) DESC",
        json!([{"g": 1, "c": 3}, {"g": 2, "c": 2}, {"g": 0, "c": 1}]),
    );
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, count(*) + 0 AS c ORDER BY count(*) + 0",
        json!([{"g": 0, "c": 1}, {"g": 2, "c": 2}, {"g": 1, "c": 3}]),
    );
}

#[test]
fn an_aggregate_only_in_order_by_is_folded_and_hidden() {
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, count(*) AS c ORDER BY sum(n.i) DESC",
        json!([{"g": 2, "c": 2}, {"g": 1, "c": 3}, {"g": 0, "c": 1}]),
    );
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, count(*) AS c ORDER BY max(n.i) - min(n.i), g",
        json!([{"g": 0, "c": 1}, {"g": 2, "c": 2}, {"g": 1, "c": 3}]),
    );
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, count(*) AS c ORDER BY sum(n.i) DESC LIMIT 1",
        json!([{"g": 2, "c": 2}]),
    );
}

#[test]
fn order_by_aggregates_through_with() {
    assert_read(
        "MATCH (n:N) WITH n.g AS g, count(*) AS c ORDER BY sum(n.i) DESC LIMIT 2 \
         RETURN collect(g) AS gs",
        json!([{"gs": [2, 1]}]),
    );
}

#[test]
fn a_restated_grouping_key_sorts_by_its_column() {
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, count(*) AS c ORDER BY n.g DESC",
        json!([{"g": 2, "c": 2}, {"g": 1, "c": 3}, {"g": 0, "c": 1}]),
    );
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, count(*) AS c ORDER BY n.g * 10 + count(*) DESC",
        json!([{"g": 2, "c": 2}, {"g": 1, "c": 3}, {"g": 0, "c": 1}]),
    );
}

#[test]
fn distinct_sorts_by_a_projected_aggregate() {
    assert_read(
        "MATCH (n:N) RETURN DISTINCT n.g AS g, count(*) AS c ORDER BY count(*) DESC",
        json!([{"g": 1, "c": 3}, {"g": 2, "c": 2}, {"g": 0, "c": 1}]),
    );
}

#[test]
fn order_by_cannot_aggregate_alone() {
    assert_rejected(
        "MATCH (n:N) RETURN n.g AS g ORDER BY count(*) DESC",
        "the projection doesn't aggregate",
    );
    assert_rejected(
        "MATCH (n:N) WITH n.g AS g ORDER BY count(*) RETURN g",
        "the projection doesn't aggregate",
    );
    assert_rejected(
        "MATCH (n:N) RETURN DISTINCT n.g AS g, count(*) AS c ORDER BY sum(n.i)",
        "after DISTINCT",
    );
    assert_rejected(
        "MATCH (n:N) RETURN n.g AS g, count(*) AS c ORDER BY n.i + count(*)",
        "grouping keys",
    );
}
