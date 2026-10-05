//! An aggregate nested inside a larger expression (`size(collect(x))`,
//! `count(*) + 1`, `{c: count(*)}`, `CASE WHEN count(*) > 2 …`) is folded
//! per group like a bare one; the expression around it is evaluated on the
//! grouped row. Inside that expression a restated grouping key reads the
//! key; any other per-row value is rejected (openCypher's implicit
//! grouping key error).
//!
//! Regression (E16): the aggregation operator only folded an item that was
//! itself an aggregate call, so a nested one read null, and the shapes the
//! planner didn't recognise as aggregating (`collect(x)[0]`, a list
//! comprehension over `collect`) ran once per input row. Every case runs on
//! each execution path (`execute`, the pull pipeline, both transaction
//! modes).

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
    db.run("UNWIND [1, 2, 3] AS i CREATE (:N {i: i, g: i % 2})");
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
fn an_aggregate_inside_a_function_is_folded() {
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN head(collect(x)) AS v",
        json!([{"v": 1}]),
    );
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN size(collect(x)) AS v",
        json!([{"v": 3}]),
    );
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN toString(max(x)) AS v",
        json!([{"v": "3"}]),
    );
    assert_read(
        "MATCH (n:N) RETURN size(collect(DISTINCT n.g)) AS v",
        json!([{"v": 2}]),
    );
}

#[test]
fn an_aggregate_inside_an_operator_or_literal_is_folded() {
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN count(x) + 1 AS v",
        json!([{"v": 4}]),
    );
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN sum(x) * 2 AS v",
        json!([{"v": 12}]),
    );
    assert_read(
        "MATCH (n:N) RETURN {c: count(*)} AS v",
        json!([{"v": {"c": 3}}]),
    );
    assert_read(
        "MATCH (n:N) RETURN [min(n.i), max(n.i)] AS v",
        json!([{"v": [1, 3]}]),
    );
    assert_read(
        "MATCH (n:N) RETURN CASE WHEN count(*) > 2 THEN 'many' ELSE 'few' END AS v",
        json!([{"v": "many"}]),
    );
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN count(x) > 0 AS any, sum(x) - count(x) AS rest",
        json!([{"any": true, "rest": 3}]),
    );
}

#[test]
fn indexing_or_filtering_a_collect_aggregates_once() {
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN collect(x)[0] AS v",
        json!([{"v": 1}]),
    );
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN collect(x)[1..] AS v",
        json!([{"v": [2, 3]}]),
    );
    assert_read(
        "MATCH (n:N) RETURN [x IN collect(n.i) WHERE x > 1 | x * 10] AS v",
        json!([{"v": [20, 30]}]),
    );
    assert_read(
        "MATCH (n:N) RETURN reduce(t = 0, x IN collect(n.i) | t + x) AS v",
        json!([{"v": 6}]),
    );
}

#[test]
fn nested_aggregates_fold_per_group() {
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, size(collect(n.i)) AS v ORDER BY g",
        json!([{"g": 0, "v": 1}, {"g": 1, "v": 2}]),
    );
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, head(collect(n.i)) + count(*) AS v ORDER BY g",
        json!([{"g": 0, "v": 3}, {"g": 1, "v": 3}]),
    );
    // A restated grouping key reads the key's value.
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, n.g * 10 + count(*) AS v ORDER BY v DESC",
        json!([{"g": 1, "v": 12}, {"g": 0, "v": 1}]),
    );
}

#[test]
fn nested_aggregates_through_with() {
    assert_read(
        "MATCH (n:N) WITH n.g AS g, size(collect(n)) AS v WHERE v > 1 RETURN g, v",
        json!([{"g": 1, "v": 2}]),
    );
    assert_read(
        "MATCH (n:N) WITH count(n) + 1 AS c RETURN c * 2 AS v",
        json!([{"v": 8}]),
    );
    assert_read(
        "MATCH (n:N) RETURN DISTINCT size(collect(n.i)) > 0 AS v",
        json!([{"v": true}]),
    );
}

#[test]
fn an_empty_input_still_aggregates_to_one_row() {
    assert_read(
        "MATCH (m:Missing) RETURN count(m) + 1 AS v",
        json!([{"v": 1}]),
    );
    assert_read(
        "MATCH (m:Missing) RETURN size(collect(m)) AS v",
        json!([{"v": 0}]),
    );
    assert_read(
        "OPTIONAL MATCH (m:Missing) RETURN head(collect(m)) AS v",
        json!([{"v": null}]),
    );
}

#[test]
fn columns_keep_their_order_and_names() {
    assert_read(
        "MATCH (n:N) RETURN count(*) + 0 AS a, n.g AS g, max(n.i) AS b ORDER BY g",
        json!([{"a": 1, "g": 0, "b": 2}, {"a": 2, "g": 1, "b": 3}]),
    );
}

#[test]
fn a_per_row_value_beside_an_aggregate_is_rejected() {
    let db = seeded();
    let err = db.run_err("MATCH (n:N) RETURN n.i + count(*) AS v");
    assert!(err.contains("grouping keys"), "{err}");
}

#[test]
fn nested_aggregates_after_a_write() {
    for path in WRITE_PATHS {
        let db = seeded();
        let rows = run_on(
            &db.service,
            path,
            "UNWIND [1, 2] AS x CREATE (m:M {v: x}) RETURN sum(m.v) * 10 AS v",
        );
        assert_eq!(rows, vec![json!({"v": 30})], "{path:?}");
    }
}
