//! The bit-shift functions (`bits.shift_left`, `bits.shift_right`,
//! `number.bitop(a, 'shl'|'shr', b)`) take a shift of 0 to 63 bits; any
//! other amount is an error. A left shift is `a * 2^b`, so a result that
//! doesn't fit a 64-bit integer is an error, as for the arithmetic
//! operators; a right shift keeps the sign and can't overflow.
//!
//! Regression: both used wrapping shifts, so the shift amount was taken
//! modulo 64 (`bits.shift_left(1, 64)` was `1`, a negative amount a huge
//! one) and bits shifted out of a left shift vanished silently.

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
    let db = TestDb::new();
    let err = db.run_err(query);
    assert!(err.contains(needle), "`{query}`: {err}");
}

#[test]
fn shifts_in_range_still_work() {
    assert_read(
        "RETURN bits.shift_left(3, 2) AS l, bits.shift_right(12, 2) AS r, \
                bits.shift_left(5, 0) AS z, bits.shift_right(-8, 1) AS neg",
        json!([{"l": 12, "r": 3, "z": 5, "neg": -4}]),
    );
    assert_read(
        "RETURN bits.shift_left(1, 62) AS top, bits.shift_left(-1, 63) AS min, \
                bits.shift_right(-1, 63) AS ones, bits.shift_right(1, 63) AS zero",
        json!([{"top": 4611686018427387904_i64, "min": i64::MIN, "ones": -1, "zero": 0}]),
    );
    assert_read(
        "RETURN number.bitop(3, 'shl', 2) AS l, number.bitop(12, 'shr', 2) AS r",
        json!([{"l": 12, "r": 3}]),
    );
}

#[test]
fn a_left_shift_that_overflows_is_an_error() {
    assert_error("RETURN bits.shift_left(1, 63) AS v", "overflowed");
    assert_error("RETURN bits.shift_left(3, 62) AS v", "overflowed");
    assert_error("RETURN bits.shift_left(-3, 62) AS v", "overflowed");
    assert_error("RETURN number.bitop(1, 'shl', 63) AS v", "overflowed");
}

#[test]
fn a_shift_outside_0_to_63_is_an_error() {
    for query in [
        "RETURN bits.shift_left(1, 64) AS v",
        "RETURN bits.shift_left(1, -1) AS v",
        "RETURN bits.shift_right(1, 64) AS v",
        "RETURN bits.shift_right(1, -1) AS v",
        "RETURN number.bitop(1, 'shr', 70) AS v",
    ] {
        assert_error(query, "between 0 and 63");
    }
}

#[test]
fn null_operands_still_give_null() {
    assert_read(
        "RETURN bits.shift_left(null, 2) AS a, bits.shift_right(4, null) AS b",
        json!([{"a": null, "b": null}]),
    );
}
