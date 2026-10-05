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
