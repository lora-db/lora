//! The smallest 64-bit integer can be written as a literal: a minus right
//! before an integer literal is parsed as part of it, so
//! `-9223372036854775808` (and its hex and octal forms) is i64::MIN rather
//! than the negation of an out-of-range positive literal.
//!
//! Regression: the minus was applied after parsing `9223372036854775808`,
//! which is one past i64::MAX, so the query failed with "invalid decimal
//! integer".

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
        let got = JsonValue::Array(run_on(&db.service, path, query));
        assert_eq!(got, expected, "{path:?}: `{query}`");
    }
}

#[test]
fn the_smallest_integer_is_a_literal() {
    assert_read(
        "RETURN -9223372036854775808 AS d, -0x8000000000000000 AS h, \
                -01000000000000000000000 AS o",
        json!([{"d": i64::MIN, "h": i64::MIN, "o": i64::MIN}]),
    );
    assert_read(
        "RETURN -9223372036854775808 = -9223372036854775807 - 1 AS same",
        json!([{"same": true}]),
    );
    assert_read(
        "UNWIND [-9223372036854775808, 9223372036854775807] AS x RETURN min(x) AS lo, max(x) AS hi",
        json!([{"lo": i64::MIN, "hi": i64::MAX}]),
    );
}

#[test]
fn negative_literals_still_behave_like_negation() {
    assert_read(
        "RETURN -1 AS a, - 1 AS b, --1 AS c, -(1) AS d, 2 - -1 AS e, 2 ^ -1 AS f",
        json!([{"a": -1, "b": -1, "c": 1, "d": -1, "e": 3, "f": 0.5}]),
    );
    assert_read("RETURN [1, 2, 3][-1] AS v", json!([{"v": 3}]));
}

#[test]
fn out_of_range_literals_are_still_rejected() {
    let db = TestDb::new();
    assert!(db
        .run_err("RETURN 9223372036854775808 AS v")
        .contains("invalid"));
    assert!(db
        .run_err("RETURN -9223372036854775809 AS v")
        .contains("invalid"));
    // Negating the smallest integer overflows, as the operators do.
    assert!(db
        .run_err("RETURN --9223372036854775808 AS v")
        .contains("overflow"));
}
