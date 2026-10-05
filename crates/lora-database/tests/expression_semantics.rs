//! Expression semantics that follow openCypher: integer / integer is
//! integer division, and dividing an integer by zero is an error; integer overflow in the
//! list builtins is an error like it is for the operators; `IN` is null
//! when a null element leaves the answer unknown; a negative slice bound
//! counts from the end. Every case runs
//! on each execution path (`execute`, the pull pipeline, both transaction
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

fn assert_error(query: &str, needle: &str) {
    let err = TestDb::new().run_err(query);
    assert!(err.contains(needle), "`{query}`: {err}");
}

#[test]
fn list_sum_and_product_error_on_integer_overflow() {
    assert_error("RETURN list.sum([9223372036854775807, 1]) AS v", "overflow");
    assert_error(
        "RETURN list.sum([-9223372036854775807, -2]) AS v",
        "overflow",
    );
    assert_error(
        "RETURN list.product([9223372036854775807, 2]) AS v",
        "overflow",
    );
    assert_error(
        "RETURN list.scan([9223372036854775807, 1], 'sum') AS v",
        "overflow",
    );
    // In range, and with a float, they still add up.
    assert_read(
        "RETURN list.sum([9223372036854775806, 1]) AS v",
        json!([{"v": 9223372036854775807i64}]),
    );
    assert_read("RETURN list.sum([1, 2.5]) AS v", json!([{"v": 3.5}]));
    assert_read("RETURN list.product([2, 3, null]) AS v", json!([{"v": 6}]));
}

#[test]
fn in_is_null_when_a_null_element_leaves_it_unknown() {
    assert_read("RETURN 5 IN [1, null] AS v", json!([{"v": null}]));
    assert_read("RETURN 1 IN [1, null] AS v", json!([{"v": true}]));
    assert_read("RETURN 5 IN [1, 2] AS v", json!([{"v": false}]));
    assert_read("RETURN 5 IN [] AS v", json!([{"v": false}]));
    assert_read("RETURN null IN [] AS v", json!([{"v": false}]));
    assert_read("RETURN null IN [1] AS v", json!([{"v": null}]));
    assert_read("RETURN NOT (5 IN [1, null]) AS v", json!([{"v": null}]));
    // As a filter, unknown keeps the row out, like false.
    assert_read(
        "UNWIND [1, 2, 5] AS x WITH x WHERE x IN [1, null] RETURN collect(x) AS v",
        json!([{"v": [1]}]),
    );
}

#[test]
fn negative_slice_bounds_count_from_the_end() {
    assert_read("RETURN [1, 2, 3][..-1] AS v", json!([{"v": [1, 2]}]));
    assert_read("RETURN [1, 2, 3][-2..] AS v", json!([{"v": [2, 3]}]));
    assert_read("RETURN [1, 2, 3][1..-1] AS v", json!([{"v": [2]}]));
    assert_read("RETURN [1, 2, 3][-1..1] AS v", json!([{"v": []}]));
    // Out of range bounds clamp to the list.
    assert_read("RETURN [1, 2, 3][-5..] AS v", json!([{"v": [1, 2, 3]}]));
    assert_read("RETURN [1, 2, 3][..-5] AS v", json!([{"v": []}]));
    assert_read("RETURN [1, 2, 3][1..10] AS v", json!([{"v": [2, 3]}]));
    // A null bound makes the slice null.
    assert_read("RETURN [1, 2, 3][null..] AS v", json!([{"v": null}]));
    assert_read("RETURN [1, 2, 3][..null] AS v", json!([{"v": null}]));
    // Single elements already counted from the end.
    assert_read(
        "RETURN [1, 2, 3][-1] AS v, [1, 2, 3][-4] AS w",
        json!([{"v": 3, "w": null}]),
    );
}

#[test]
fn integer_division_truncates_toward_zero() {
    assert_read("RETURN 7 / 2 AS v", json!([{"v": 3}]));
    assert_read("RETURN -7 / 2 AS v", json!([{"v": -3}]));
    assert_read("RETURN 7 / -2 AS v", json!([{"v": -3}]));
    assert_read("RETURN 6 / 3 AS v", json!([{"v": 2}]));
    // A float operand keeps float division.
    assert_read("RETURN 7.0 / 2 AS v", json!([{"v": 3.5}]));
    assert_read("RETURN 7 / 2.0 AS v", json!([{"v": 3.5}]));
    assert_read("RETURN toFloat(7) / 2 AS v", json!([{"v": 3.5}]));
    assert_read("RETURN null / 2 AS v", json!([{"v": null}]));
}

#[test]
fn modulo_keeps_the_dividends_sign_and_works_on_floats() {
    assert_read("RETURN 7 % 3 AS v, -7 % 3 AS w", json!([{"v": 1, "w": -1}]));
    assert_read(
        "RETURN 7.5 % 2 AS v, -7.5 % 2 AS w",
        json!([{"v": 1.5, "w": -1.5}]),
    );
    assert_read("RETURN 7 % 2.5 AS v", json!([{"v": 2.0}]));
}

#[test]
fn integer_division_by_zero_is_an_error() {
    assert_error("RETURN 7 / 0 AS v", "by zero");
    assert_error("RETURN 7 % 0 AS v", "by zero");
    assert_error("UNWIND [1, 0] AS d RETURN 10 / d AS v", "by zero");
    assert_error(
        "RETURN -9223372036854775807 - 1 / 1 AS v, (-9223372036854775807 - 1) / -1 AS w",
        "overflow",
    );
}
