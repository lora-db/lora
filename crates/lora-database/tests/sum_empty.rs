//! `sum()` over no values is `0` (openCypher): an empty input, or a group
//! whose values are all null. `avg()` over no values stays `null`, since
//! there is nothing to average.
//!
//! Regression: sum() returned null when it had nothing to add.

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

fn assert_read(query: &str, expected: JsonValue) {
    for path in READ_PATHS {
        let db = TestDb::new();
        db.run("CREATE (:N {g: 1, v: 2}), (:N {g: 2})");
        let got = JsonValue::Array(run_on(&db.service, path, query));
        assert_eq!(got, expected, "{path:?}: `{query}`");
    }
}

#[test]
fn sum_of_nothing_is_zero() {
    assert_read("UNWIND [] AS x RETURN sum(x) AS s", json!([{"s": 0}]));
    assert_read("MATCH (m:Missing) RETURN sum(m.v) AS s", json!([{"s": 0}]));
    assert_read(
        "UNWIND [null, null] AS x RETURN sum(x) AS s",
        json!([{"s": 0}]),
    );
    assert_read(
        "UNWIND [] AS x RETURN sum(DISTINCT x) AS s",
        json!([{"s": 0}]),
    );
}

#[test]
fn a_group_with_only_nulls_sums_to_zero() {
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, sum(n.v) AS s ORDER BY g",
        json!([{"g": 1, "s": 2}, {"g": 2, "s": 0}]),
    );
}

#[test]
fn avg_of_nothing_stays_null() {
    assert_read(
        "UNWIND [] AS x RETURN avg(x) AS a, sum(x) AS s",
        json!([{"a": null, "s": 0}]),
    );
    assert_read(
        "MATCH (n:N) RETURN n.g AS g, avg(n.v) AS a ORDER BY g",
        json!([{"g": 1, "a": 2.0}, {"g": 2, "a": null}]),
    );
}

#[test]
fn a_sum_of_nothing_inside_an_expression_is_zero() {
    assert_read(
        "MATCH (m:Missing) RETURN sum(m.v) + 1 AS s",
        json!([{"s": 1}]),
    );
}
