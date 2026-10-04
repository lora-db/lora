//! An OPTIONAL MATCH that opens the query is driven by the single empty
//! unit row, like every other leading clause. openCypher (and Neo4j) then
//! return ONE row with the pattern's variables null when nothing matches.
//!
//! Regression: the planner only wrapped an OPTIONAL MATCH in its
//! null-extending operator when an upstream clause existed, so a leading
//! one planned as a plain MATCH and an empty match silently returned no
//! rows (`OPTIONAL MATCH (u:Missing) RETURN u IS NULL` gave `[]`).
//!
//! Every case runs on each execution path: `Database::execute`, the pull
//! pipeline (`Database::stream`), and `Transaction::execute_rows` in both
//! transaction modes (read-only cases only for `ReadOnly`).

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
fn empty_leading_optional_match_yields_one_null_row() {
    assert_read(
        "OPTIONAL MATCH (u:Missing) RETURN u IS NULL AS missing",
        json!([{"missing": true}]),
    );
    assert_read("OPTIONAL MATCH (u:Missing) RETURN u", json!([{"u": null}]));
    assert_read(
        "OPTIONAL MATCH (u:Missing) WITH u RETURN u",
        json!([{"u": null}]),
    );
    assert_read(
        "OPTIONAL MATCH (a:P)-[:Nope]->(b) RETURN a, b",
        json!([{"a": null, "b": null}]),
    );
}

#[test]
fn aggregates_over_an_empty_leading_optional_match_are_unchanged() {
    assert_read(
        "OPTIONAL MATCH (u:Missing) RETURN count(u) AS n",
        json!([{"n": 0}]),
    );
    assert_read(
        "OPTIONAL MATCH (u:Missing) RETURN count(*) AS n",
        json!([{"n": 1}]),
    );
}

#[test]
fn matching_leading_optional_match_returns_only_its_rows() {
    assert_read(
        "OPTIONAL MATCH (p:P) RETURN p.k AS k ORDER BY k",
        json!([{"k": 1}, {"k": 2}]),
    );
    assert_read(
        "OPTIONAL MATCH (p:P)-[:X]->(q:Q) RETURN p.k AS k, q.v AS v",
        json!([{"k": 1, "v": 10}]),
    );
}

#[test]
fn where_that_filters_everything_yields_one_null_row() {
    assert_read(
        "OPTIONAL MATCH (p:P) WHERE p.k > 99 RETURN p",
        json!([{"p": null}]),
    );
    assert_read(
        "OPTIONAL MATCH (p:P) WHERE p.k = 2 RETURN p.k AS k",
        json!([{"k": 2}]),
    );
}

#[test]
fn several_leading_optional_matches() {
    assert_read(
        "OPTIONAL MATCH (a:Missing) OPTIONAL MATCH (b:AlsoMissing) RETURN a, b",
        json!([{"a": null, "b": null}]),
    );
    assert_read(
        "OPTIONAL MATCH (a:Missing) OPTIONAL MATCH (p:P) RETURN a, p.k AS k ORDER BY k",
        json!([{"a": null, "k": 1}, {"a": null, "k": 2}]),
    );
    assert_read(
        "OPTIONAL MATCH (p:P) OPTIONAL MATCH (p)-[:X]->(q) RETURN p.k AS k, q.v AS v ORDER BY k",
        json!([{"k": 1, "v": 10}, {"k": 2, "v": null}]),
    );
}

#[test]
fn optional_match_after_zero_rows_stays_zero_rows() {
    assert_read(
        "MATCH (n:Missing) WITH n OPTIONAL MATCH (m:Missing) RETURN m",
        json!([]),
    );
    assert_read(
        "MATCH (n:Missing) OPTIONAL MATCH (n)-[:X]->(m) RETURN m",
        json!([]),
    );
}

#[test]
fn optional_match_after_an_input_row_is_unchanged() {
    assert_read(
        "UNWIND [1] AS x OPTIONAL MATCH (u:Missing) RETURN x, u",
        json!([{"x": 1, "u": null}]),
    );
}

#[test]
fn leading_optional_match_inside_call_subquery() {
    assert_read(
        "UNWIND [1, 2] AS x CALL { OPTIONAL MATCH (u:Missing) RETURN u } RETURN x, u ORDER BY x",
        json!([{"x": 1, "u": null}, {"x": 2, "u": null}]),
    );
}

#[test]
fn write_after_an_empty_leading_optional_match_runs_once() {
    for path in WRITE_PATHS {
        let db = seeded();
        run_on(
            &db.service,
            path,
            "OPTIONAL MATCH (u:Missing) CREATE (:Log)",
        );
        assert_eq!(
            db.scalar("MATCH (l:Log) RETURN count(l) AS n"),
            json!(1),
            "{path:?}"
        );

        let rows = run_on(
            &db.service,
            path,
            "OPTIONAL MATCH (p:P) CREATE (:Log {k: p.k}) RETURN count(*) AS n",
        );
        assert_eq!(rows, vec![json!({"n": 2})], "{path:?}");
        assert_eq!(
            db.scalar("MATCH (l:Log) RETURN count(l) AS n"),
            json!(3),
            "{path:?}"
        );
    }
}
