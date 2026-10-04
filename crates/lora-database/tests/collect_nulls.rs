//! `collect()` ignores null values (openCypher): a null item is dropped,
//! so a group whose values are all null collects to `[]`. Only top-level
//! nulls go — a list or map that merely CONTAINS null is a value and is
//! kept as-is.
//!
//! Regression: collect() kept nulls, so `UNWIND [1, null] AS x RETURN
//! collect(x)` gave `[1, null]` and an unmatched OPTIONAL MATCH collected
//! to `[null]`. Every case runs on each execution path (`execute`, the
//! pull pipeline, both transaction modes).

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
const WRITE_PATHS: [Path; 3] = [Path::Execute, Path::Stream, Path::TxReadWrite];

fn rows_json(rows: Vec<Row>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}

fn run_on(db: &Database<InMemoryGraph>, path: Path, query: &str) -> Vec<JsonValue> {
    let run = || -> Result<Vec<JsonValue>, String> {
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
    };
    run().unwrap_or_else(|e| panic!("{path:?}: `{query}` failed: {e}"))
}

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (:P {k: 1})-[:X]->(:Q {v: 10}), (:P {k: 2})");
    db
}

fn assert_read(query: &str, expected: JsonValue) {
    for path in READ_PATHS {
        let db = seeded();
        let got = JsonValue::Array(run_on(&db.service, path, query));
        assert_eq!(got, expected, "{path:?}: `{query}`");
    }
}

#[test]
fn top_level_nulls_are_dropped() {
    assert_read(
        "UNWIND [1, null, 2] AS x RETURN collect(x) AS xs",
        json!([{"xs": [1, 2]}]),
    );
    assert_read(
        "UNWIND [null, null] AS x RETURN collect(x) AS xs",
        json!([{"xs": []}]),
    );
    assert_read(
        "UNWIND [1, null] AS x WITH collect(x) AS xs RETURN size(xs) AS n",
        json!([{"n": 1}]),
    );
}

#[test]
fn unmatched_optional_match_collects_to_empty() {
    assert_read(
        "OPTIONAL MATCH (u:Missing) RETURN collect(u) AS us",
        json!([{"us": []}]),
    );
    assert_read(
        "MATCH (p:P) OPTIONAL MATCH (p)-[:X]->(q:Q) \
         RETURN p.k AS k, collect(q.v) AS vs ORDER BY k",
        json!([{"k": 1, "vs": [10]}, {"k": 2, "vs": []}]),
    );
    assert_read(
        "MATCH (p:P) OPTIONAL MATCH (p)-[:X]->(q:Q) \
         WITH p, collect(q) AS qs RETURN p.k AS k, size(qs) AS n ORDER BY k",
        json!([{"k": 1, "n": 1}, {"k": 2, "n": 0}]),
    );
}

#[test]
fn grouped_collect_drops_nulls_per_group() {
    assert_read(
        "UNWIND [{g: 1, v: null}, {g: 1, v: 2}, {g: 2, v: null}] AS r \
         RETURN r.g AS g, collect(r.v) AS vs ORDER BY g",
        json!([{"g": 1, "vs": [2]}, {"g": 2, "vs": []}]),
    );
}

#[test]
fn collect_distinct_drops_nulls() {
    assert_read(
        "UNWIND [1, null, 1, null, 2] AS x RETURN collect(DISTINCT x) AS xs",
        json!([{"xs": [1, 2]}]),
    );
    assert_read(
        "UNWIND [null] AS x RETURN collect(DISTINCT x) AS xs",
        json!([{"xs": []}]),
    );
}

#[test]
fn collect_alongside_streaming_aggregates() {
    // count/sum fold in streaming form; collect stays buffered. Mixing
    // them in one projection exercises both shapes together.
    assert_read(
        "UNWIND [1, null, 3] AS x \
         RETURN count(x) AS c, count(*) AS all, sum(x) AS s, collect(x) AS xs",
        json!([{"c": 2, "all": 3, "s": 4, "xs": [1, 3]}]),
    );
}

#[test]
fn collect_inside_call_subquery() {
    assert_read(
        "UNWIND [1, 2] AS x \
         CALL { WITH x UNWIND [x, null] AS y RETURN collect(y) AS ys } \
         RETURN x, ys ORDER BY x",
        json!([{"x": 1, "ys": [1]}, {"x": 2, "ys": [2]}]),
    );
    assert_read(
        "CALL { OPTIONAL MATCH (u:Missing) RETURN collect(u) AS us } RETURN us",
        json!([{"us": []}]),
    );
}

#[test]
fn values_that_contain_null_are_kept() {
    assert_read(
        "UNWIND [[1, null], null, {a: null}, []] AS x RETURN collect(x) AS xs",
        json!([{"xs": [[1, null], {"a": null}, []]}]),
    );
    assert_read(
        "UNWIND [[null], [null], null] AS x RETURN collect(DISTINCT x) AS xs",
        json!([{"xs": [[null]]}]),
    );
}

#[test]
fn collect_after_a_write_drops_nulls() {
    for path in WRITE_PATHS {
        let db = seeded();
        let rows = run_on(
            &db.service,
            path,
            "UNWIND [1, null] AS x CREATE (n:N {v: x}) RETURN collect(n.v) AS vs",
        );
        assert_eq!(rows, vec![json!({"vs": [1]})], "{path:?}");
    }
}
