//! What `min`/`max`, `sum`/`avg`, `stdev*` and `percentile*` return, on
//! each execution path (`execute`, the pull pipeline, both transaction
//! modes). `min`/`max` without DISTINCT fold in streaming form and with
//! DISTINCT in buffered form; both must agree with each other and with
//! ORDER BY.

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

/// `query` with and without DISTINCT on its aggregates (the two fold
/// shapes) must give `expected`.
fn assert_both_shapes(query: &str, expected: JsonValue) {
    assert_read(query, expected.clone());
    assert_read(&query.replace("(x)", "(DISTINCT x)"), expected);
}

// ---------------------------------------------------------------------------
// min / max
// ---------------------------------------------------------------------------

#[test]
fn min_max_order_temporal_values() {
    assert_both_shapes(
        "UNWIND [date('2024-01-02'), date('2023-01-01'), date('2024-06-01')] AS x \
         RETURN min(x) AS mn, max(x) AS mx",
        json!([{"mn": "2023-01-01", "mx": "2024-06-01"}]),
    );
    assert_both_shapes(
        "UNWIND [localdatetime('2024-01-02T00:00'), localdatetime('2024-01-01T00:00'), \
                 localdatetime('2024-03-01T00:00')] AS x \
         RETURN min(x) AS mn, max(x) AS mx",
        json!([{"mn": "2024-01-01T00:00:00", "mx": "2024-03-01T00:00:00"}]),
    );
    assert_both_shapes(
        "UNWIND [localtime('10:00'), localtime('09:00'), localtime('11:30')] AS x \
         RETURN min(x) AS mn, max(x) AS mx",
        json!([{"mn": "09:00:00", "mx": "11:30:00"}]),
    );
    assert_both_shapes(
        "UNWIND [time('10:00Z'), time('09:00Z'), time('12:00Z')] AS x \
         RETURN min(x) AS mn, max(x) AS mx",
        json!([{"mn": "09:00:00Z", "mx": "12:00:00Z"}]),
    );
    assert_both_shapes(
        "UNWIND [datetime('2024-01-02T00:00Z'), datetime('2023-01-01T00:00Z')] AS x \
         RETURN min(x) AS mn, max(x) AS mx",
        json!([{"mn": "2023-01-01T00:00:00Z", "mx": "2024-01-02T00:00:00Z"}]),
    );
}

#[test]
fn min_max_order_durations() {
    assert_both_shapes(
        "UNWIND [duration('P1D'), duration('PT1H'), duration('P2D')] AS x \
         RETURN min(x) AS mn, max(x) AS mx",
        json!([{"mn": "PT1H", "mx": "P2D"}]),
    );
}

#[test]
fn min_max_agree_with_order_by() {
    for list in [
        "[date('2024-01-02'), date('2023-01-01')]",
        "[duration('P1D'), duration('PT1H')]",
        "[1, 'a', 2.5]",
    ] {
        let db = TestDb::new();
        let agg = db.run(&format!(
            "UNWIND {list} AS x RETURN min(x) AS mn, max(x) AS mx"
        ));
        let first = db.run(&format!("UNWIND {list} AS x RETURN x ORDER BY x LIMIT 1"));
        let last = db.run(&format!(
            "UNWIND {list} AS x RETURN x ORDER BY x DESC LIMIT 1"
        ));
        assert_eq!(agg[0]["mn"], first[0]["x"], "{list}");
        assert_eq!(agg[0]["mx"], last[0]["x"], "{list}");
    }
}

#[test]
fn grouped_min_max_of_temporal_values() {
    assert_read(
        "UNWIND [{g: 1, d: date('2024-05-01')}, {g: 1, d: date('2024-01-01')}, \
                 {g: 2, d: date('2020-01-01')}] AS r \
         RETURN r.g AS g, min(r.d) AS mn, max(r.d) AS mx ORDER BY g",
        json!([
            {"g": 1, "mn": "2024-01-01", "mx": "2024-05-01"},
            {"g": 2, "mn": "2020-01-01", "mx": "2020-01-01"}
        ]),
    );
}

// ---------------------------------------------------------------------------
// sum / avg
// ---------------------------------------------------------------------------

fn assert_read_err(query: &str, needle: &str) {
    for path in READ_PATHS {
        let db = TestDb::new();
        let run = std::panic::AssertUnwindSafe(|| run_on(&db.service, path, query));
        let err =
            std::panic::catch_unwind(run).expect_err(&format!("{path:?}: `{query}` should fail"));
        let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains(needle), "{path:?}: `{query}`: {msg}");
    }
}

#[test]
fn integer_sums_are_exact() {
    // 2^53 + 1 is not a float: an f64 accumulator loses the 1.
    assert_both_shapes(
        "UNWIND [9007199254740993, 2] AS x RETURN sum(x) AS v",
        json!([{"v": 9007199254740995_i64}]),
    );
    assert_both_shapes(
        "UNWIND [1, 2, 3] AS x RETURN sum(x) AS v",
        json!([{"v": 6}]),
    );
}

#[test]
fn integer_sum_overflow_is_an_error() {
    assert_read_err(
        "UNWIND [9223372036854775807, 1] AS x RETURN sum(x) AS v",
        "sum() overflowed",
    );
    assert_read_err(
        "UNWIND [9223372036854775807, 1] AS x RETURN sum(DISTINCT x) AS v",
        "sum() overflowed",
    );
}

#[test]
fn a_float_sum_is_a_float() {
    assert_both_shapes(
        "UNWIND [2.0, 3.0] AS x RETURN sum(x) AS v",
        json!([{"v": 5.0}]),
    );
    assert_both_shapes(
        "UNWIND [1, 2.5] AS x RETURN sum(x) AS v",
        json!([{"v": 3.5}]),
    );
    assert_both_shapes(
        "UNWIND [1, 2.0] AS x RETURN sum(x) AS v",
        json!([{"v": 3.0}]),
    );
}

#[test]
fn avg_of_numbers() {
    assert_both_shapes("UNWIND [1, 2] AS x RETURN avg(x) AS v", json!([{"v": 1.5}]));
    assert_both_shapes(
        "UNWIND [1.0, 2.0, null] AS x RETURN avg(x) AS v",
        json!([{"v": 1.5}]),
    );
    assert_both_shapes(
        "UNWIND [null] AS x RETURN avg(x) AS v",
        json!([{"v": null}]),
    );
}

#[test]
fn sum_and_avg_of_durations() {
    assert_both_shapes(
        "UNWIND [duration('P1D'), duration('PT12H')] AS x RETURN sum(x) AS s, avg(x) AS a",
        json!([{"s": "P1DT12H", "a": "PT18H"}]),
    );
    assert_read(
        "UNWIND [{g: 1, d: duration('PT1H')}, {g: 1, d: duration('PT3H')}] AS r \
         RETURN r.g AS g, sum(r.d) AS s, avg(r.d) AS a",
        json!([{"g": 1, "s": "PT4H", "a": "PT2H"}]),
    );
    assert_read_err(
        "UNWIND [duration('P1D'), 1] AS x RETURN sum(x) AS v",
        "can't add durations and numbers",
    );
}

// ---------------------------------------------------------------------------
// stdev / percentile
// ---------------------------------------------------------------------------

#[test]
fn stdev_honours_distinct() {
    // [1, 3]: sample stdev sqrt(2), population stdev 1.
    assert_read(
        "UNWIND [1, 1, 3] AS x RETURN stdev(DISTINCT x) AS s, stdevp(DISTINCT x) AS p",
        json!([{"s": std::f64::consts::SQRT_2, "p": 1.0}]),
    );
    assert_read(
        "UNWIND [2, 2] AS x RETURN stdev(DISTINCT x) AS s, stdev(x) AS all",
        json!([{"s": 0.0, "all": 0.0}]),
    );
}

#[test]
fn percentiles_honour_distinct() {
    // Without DISTINCT the median of [1, 1, 1, 5] is 1; of [1, 5] it is 3.
    assert_read(
        "UNWIND [1, 1, 1, 5] AS x \
         RETURN percentileCont(x, 0.5) AS c, percentileCont(DISTINCT x, 0.5) AS cd, \
                percentileDisc(x, 1.0) AS d, percentileDisc(DISTINCT x, 0.0) AS dd",
        json!([{"c": 1.0, "cd": 3.0, "d": 5, "dd": 1}]),
    );
}

#[test]
fn percentile_disc_returns_the_value_itself() {
    assert_read(
        "UNWIND [1, 2, 3] AS x RETURN percentileDisc(x, 0.5) AS v",
        json!([{"v": 2}]),
    );
    assert_read(
        "UNWIND [1.5, 2.5, 3.5] AS x RETURN percentileDisc(x, 0.5) AS v",
        json!([{"v": 2.5}]),
    );
    assert_read(
        "UNWIND [1, 2.5] AS x RETURN percentileDisc(x, 0.0) AS lo, percentileDisc(x, 1.0) AS hi",
        json!([{"lo": 1, "hi": 2.5}]),
    );
}
