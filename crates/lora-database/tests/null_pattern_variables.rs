//! A pattern on a variable bound to `null` matches nothing, as in
//! openCypher: in a `MATCH`, an `OPTIONAL MATCH`, a pattern comprehension,
//! `EXISTS { }` and `COUNT { }` alike, whether the null is the start of the
//! pattern, its end, or a relationship.
//!
//! Regression: a pattern comprehension (and `EXISTS`) read a null variable
//! as unbound and matched the pattern for every node there is, so "the
//! viewer's genres" answered everybody's when there was no viewer; a
//! `MATCH` on the same null failed with `expected variable ... to contain a
//! node` where it should return no rows. Every case runs on each execution
//! path (`execute`, the pull pipeline, both transaction modes).

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

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE (:P {key: 'p'})-[:LIKES {w: 1}]->(:G {key: 'jazz'}), \
                (:P {key: 'q'})-[:LIKES {w: 2}]->(:G {key: 'rock'})",
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

/// Ways a variable ends up null: a literal, an OPTIONAL MATCH that found
/// nothing, and the head of an empty collection.
const NULL_V: [&str; 3] = [
    "WITH null AS v",
    "OPTIONAL MATCH (v:P) WHERE v.key = 'nobody' WITH v",
    "MATCH (x:P) WHERE x.key = 'nobody' WITH head(collect(x)) AS v",
];

#[test]
fn a_pattern_comprehension_from_a_null_matches_nothing() {
    for null_v in NULL_V {
        assert_read(
            &format!("{null_v} RETURN [(v)-[:LIKES]->(g:G) | g.key] AS liked"),
            json!([{"liked": []}]),
        );
        // The null at the end of the pattern.
        assert_read(
            &format!("{null_v} RETURN [(g:G)<-[:LIKES]-(v) | g.key] AS liked"),
            json!([{"liked": []}]),
        );
        assert_read(
            &format!("{null_v} RETURN [(v) | 1] AS ones"),
            json!([{"ones": []}]),
        );
        assert_read(
            &format!("{null_v} RETURN [(v)-[:LIKES*1..2]->(g) | g.key] AS liked"),
            json!([{"liked": []}]),
        );
    }
}

#[test]
fn exists_and_count_on_a_null_find_nothing() {
    for null_v in NULL_V {
        assert_read(
            &format!(
                "{null_v} RETURN EXISTS {{ (v)-[:LIKES]->(:G) }} AS e, \
                 EXISTS {{ (:G)<-[:LIKES]-(v) }} AS e2"
            ),
            json!([{"e": false, "e2": false}]),
        );
        assert_read(
            &format!("{null_v} RETURN size([(v)-[:LIKES]->(g:G) | g]) AS n"),
            json!([{"n": 0}]),
        );
    }
}

#[test]
fn match_on_a_null_returns_no_rows() {
    for null_v in NULL_V {
        for pattern in [
            "(v)",
            "(v:P)",
            "(v {key: 'p'})",
            "(v)-[:LIKES]->(g:G)",
            "(g:G)<-[:LIKES]-(v)",
            "(v)-[:LIKES*1..2]->(g)",
            "(v)-[:LIKES*0..1]->(g)",
            "(g:G)<-[:LIKES*1..2]-(v)",
            "(g:G), (v)",
        ] {
            assert_read(
                &format!("{null_v} MATCH {pattern} RETURN 1 AS one"),
                json!([]),
            );
        }
        assert_read(
            &format!("{null_v} MATCH (v) WHERE id(v) = 0 RETURN 1 AS one"),
            json!([]),
        );
        assert_read(
            &format!("{null_v} MATCH (v:P) WHERE v.key = 'p' RETURN 1 AS one"),
            json!([]),
        );
    }
}

#[test]
fn match_on_a_null_relationship_returns_no_rows() {
    assert_read(
        "WITH null AS r MATCH (a)-[r]->(b) RETURN a.key AS key",
        json!([]),
    );
    assert_read(
        "WITH null AS r MATCH (a)-[r:LIKES]->(b) WHERE r.w > 0 RETURN a.key AS key",
        json!([]),
    );
    assert_read(
        "WITH null AS r RETURN [(a)-[r]->(b) | a.key] AS keys, EXISTS { (a)-[r]->(b) } AS e",
        json!([{"keys": [], "e": false}]),
    );
}

#[test]
fn optional_match_on_a_null_keeps_the_row() {
    for null_v in NULL_V {
        assert_read(
            &format!("{null_v} OPTIONAL MATCH (v)-[:LIKES]->(g:G) RETURN g.key AS key"),
            json!([{"key": null}]),
        );
        assert_read(
            &format!("{null_v} OPTIONAL MATCH (g:G)<-[:LIKES]-(v) RETURN g.key AS key"),
            json!([{"key": null}]),
        );
        // A path binding makes the inner pattern run uncorrelated.
        assert_read(
            &format!("{null_v} OPTIONAL MATCH p = (v)-[:LIKES]->(g:G) RETURN g.key AS key"),
            json!([{"key": null}]),
        );
    }
}

#[test]
fn a_bound_variable_still_matches_on_its_node() {
    assert_read(
        "MATCH (v:P {key: 'p'}) RETURN [(v)-[:LIKES]->(g:G) | g.key] AS liked, \
         EXISTS { (v)-[:LIKES]->(:G {key: 'jazz'}) } AS e",
        json!([{"liked": ["jazz"], "e": true}]),
    );
    assert_read(
        "MATCH (v:P {key: 'p'}) MATCH (v)-[:LIKES]->(g:G) RETURN g.key AS key",
        json!([{"key": "jazz"}]),
    );
    assert_read(
        "MATCH (v:P {key: 'p'}) OPTIONAL MATCH (g:G)<-[:LIKES]-(v) RETURN g.key AS key",
        json!([{"key": "jazz"}]),
    );
    // Nulls and nodes in the same stream.
    assert_read(
        "UNWIND ['p', 'nobody', 'q'] AS k OPTIONAL MATCH (v:P {key: k}) \
         RETURN k, [(v)-[:LIKES]->(g:G) | g.key] AS liked",
        json!([
            {"k": "p", "liked": ["jazz"]},
            {"k": "nobody", "liked": []},
            {"k": "q", "liked": ["rock"]},
        ]),
    );
    assert_read(
        "UNWIND ['p', 'nobody', 'q'] AS k OPTIONAL MATCH (v:P {key: k}) \
         MATCH (v)-[:LIKES]->(g:G) RETURN g.key AS key",
        json!([{"key": "jazz"}, {"key": "rock"}]),
    );
}

#[test]
fn a_write_after_a_match_on_a_null_writes_nothing() {
    let db = seeded();
    let rows = db.run("WITH null AS v MATCH (v)-[:LIKES]->(g:G) SET g.seen = true RETURN g.key");
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(
        db.scalar("MATCH (g:G) WHERE g.seen RETURN count(g) AS n"),
        0
    );
}
