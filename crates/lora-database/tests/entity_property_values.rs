//! Nodes, relationships and paths are not property values: storing one,
//! at any depth (inside a list or map too), through CREATE, MERGE, SET
//! (`n.p =`, `n =`, `n +=`) or on a relationship, is an error and leaves
//! the graph unchanged. A lookup by such a value matches no stored
//! property (a guard: index seeks convert through the same function).
//!
//! Regression: a node was stored as the string `"node:<id>"`, a
//! relationship as `"rel:<id>"` and a path as null, so `CREATE (:Y {v: a})`
//! succeeded.

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
    db.run("CREATE (:A {k: 1})-[:R]->(:B {k: 2})");
    db
}

fn try_on(db: &Database<InMemoryGraph>, path: Path, query: &str) -> Result<(), String> {
    match path {
        Path::Execute => db
            .execute(
                query,
                Some(ExecuteOptions {
                    format: ResultFormat::Rows,
                }),
            )
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Path::Stream => {
            let mut stream = db.stream(query).map_err(|e| e.to_string())?;
            while stream.next_row().map_err(|e| e.to_string())?.is_some() {}
            stream.finish().map_err(|e| e.to_string())
        }
        Path::TxReadWrite | Path::TxReadOnly => {
            let mut tx = db
                .begin_transaction(TransactionMode::ReadWrite)
                .map_err(|e| e.to_string())?;
            tx.execute_rows(query).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())
        }
    }
}

fn assert_rejected(query: &str, kind: &str) {
    for path in WRITE_PATHS {
        let db = seeded();
        let err = try_on(&db.service, path, query).expect_err(query);
        assert!(
            err.contains(&format!("cannot store {kind} as a property")),
            "{path:?}: `{query}`: {err}"
        );
        // Nothing the statement did is kept.
        let state = run_on(
            &db.service,
            Path::Execute,
            "MATCH (n) RETURN count(n) AS nodes, \
                    sum(size(keys(n))) AS props",
        );
        assert_eq!(
            state,
            vec![json!({"nodes": 2, "props": 2})],
            "{path:?}: `{query}`"
        );
    }
}

#[test]
fn a_node_is_not_a_property_value() {
    for query in [
        "MATCH (a:A) CREATE (:Y {v: a})",
        "MATCH (a:A) SET a.v = a",
        "MATCH (a:A), (b:B) SET b.v = a",
        "MATCH (a:A) SET a = {k: 1, v: a}",
        "MATCH (a:A) SET a += {v: a}",
        "MATCH (a:A) MERGE (:Y {v: a})",
        "MATCH (a:A) MERGE (y:Y {k: 9}) ON CREATE SET y.v = a",
        "MATCH (a:A) CREATE (:Y {v: [a]})",
        "MATCH (a:A) CREATE (:Y {v: {inner: a}})",
        "MATCH (a:A), (b:B) CREATE (a)-[:S {v: b}]->(b)",
    ] {
        assert_rejected(query, "a node");
    }
}

#[test]
fn a_relationship_or_path_is_not_a_property_value() {
    assert_rejected("MATCH ()-[r:R]->() CREATE (:Y {v: r})", "a relationship");
    assert_rejected("MATCH ()-[r:R]->(b) SET b.v = [r]", "a relationship");
    assert_rejected("MATCH p = (:A)-[:R]->(:B) CREATE (:Y {v: p})", "a path");
    assert_rejected("MATCH p = (a:A)-[:R]->(:B) SET a.v = p", "a path");
}

#[test]
fn values_that_are_properties_are_still_stored() {
    for path in WRITE_PATHS {
        let db = seeded();
        let rows = run_on(
            &db.service,
            path,
            "MATCH (a:A) CREATE (y:Y {id: id(a), k: a.k, m: {k: a.k}, ks: [a.k]}) \
             RETURN y.k AS k, y.m AS m, y.ks AS ks",
        );
        assert_eq!(
            rows,
            vec![json!({"k": 1, "m": {"k": 1}, "ks": [1]})],
            "{path:?}"
        );
    }
}

#[test]
fn a_node_matches_no_stored_property() {
    for path in READ_PATHS {
        let db = seeded();
        // A property holding the string the node used to be stored as.
        let id = db.run("MATCH (a:A) RETURN id(a) AS id")[0]["id"].clone();
        db.run(&format!("CREATE (:Z {{v: 'node:{id}'}})"));
        db.run("CREATE INDEX z IF NOT EXISTS FOR (z:Z) ON (z.v)");
        let rows = run_on(
            &db.service,
            path,
            "MATCH (a:A) MATCH (z:Z) WHERE z.v = a RETURN count(z) AS n",
        );
        assert_eq!(rows, vec![json!({"n": 0})], "{path:?}");
    }
}
