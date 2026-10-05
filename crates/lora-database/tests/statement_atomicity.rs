//! A statement that fails partway leaves the graph as it was: none of the
//! rows it wrote before the failing one are kept.
//!
//! Regression: auto-commit writes simple enough for the live fast path
//! (one node-only `CREATE`, one `SET` of literal properties, a `DELETE`)
//! mutate the graph in place with no rollback, on the grounds that such a
//! plan can't fail midway. A constraint can: `UNWIND [1, 2] AS i CREATE
//! (:U {key: 'x'})` kept the first node, `MATCH (u:U) SET u.key = 'same'`
//! kept the first rename, and a plain `DELETE` kept the nodes it deleted
//! before reaching one that still had relationships. Those plans now take
//! the staged path whenever a constraint or a relationship can reject them.
//! Every case runs on each write path (`execute`, the pull pipeline, an
//! explicit transaction).

mod test_helpers;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat, Row, TransactionMode};
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

#[derive(Clone, Copy, Debug)]
enum Path {
    Execute,
    Stream,
    Tx,
}

const WRITE_PATHS: [Path; 3] = [Path::Execute, Path::Stream, Path::Tx];

fn rows_json(rows: Vec<Row>) -> Vec<JsonValue> {
    rows.into_iter()
        .map(|row| serde_json::to_value(row).unwrap())
        .collect()
}

/// Run `query` on `path`, returning its error message if it fails. An
/// explicit transaction runs one statement before it and commits after, so
/// the failed statement must not take the earlier one down with it either.
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
        Path::Tx => {
            let mut tx = db
                .begin_transaction(TransactionMode::ReadWrite)
                .map_err(|e| e.to_string())?;
            tx.execute_rows("CREATE (:Before)")
                .map_err(|e| e.to_string())?;
            let result = tx
                .execute_rows(query)
                .map(|_| ())
                .map_err(|e| e.to_string());
            tx.commit().map_err(|e| e.to_string())?;
            result
        }
    }
}

fn read(db: &TestDb, query: &str) -> JsonValue {
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadOnly)
        .unwrap();
    let rows = tx.execute_rows(query).unwrap();
    tx.commit().unwrap();
    JsonValue::Array(rows_json(rows))
}

fn constrained() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT uq FOR (u:U) REQUIRE u.key IS UNIQUE");
    db.run("CREATE CONSTRAINT nn FOR (n:N) REQUIRE n.v IS NOT NULL");
    db.run("CREATE (:U {key: 'a'}), (:U {key: 'b'})");
    db
}

/// `query` fails with `code` on every write path and leaves `check`
/// answering `expected`.
fn assert_rejected_whole(
    setup: fn() -> TestDb,
    query: &str,
    code: &str,
    check: &str,
    expected: JsonValue,
) {
    for path in WRITE_PATHS {
        let db = setup();
        let err = try_on(&db.service, path, query).expect_err(query);
        assert!(err.contains(code), "{path:?}: `{query}`: {err}");
        assert_eq!(read(&db, check), expected, "{path:?}: `{query}`");
        if matches!(path, Path::Tx) {
            // The statement before it in the transaction is kept.
            assert_eq!(
                read(&db, "MATCH (b:Before) RETURN count(b) AS c"),
                json!([{"c": 1}]),
                "{path:?}: `{query}`"
            );
        }
    }
}

#[test]
fn a_duplicate_key_on_a_later_row_keeps_no_row() {
    assert_rejected_whole(
        constrained,
        "UNWIND ['x', 'x'] AS k CREATE (:U {key: k})",
        "22N79",
        "MATCH (u:U) RETURN u.key AS k ORDER BY k",
        json!([{"k": "a"}, {"k": "b"}]),
    );
    assert_rejected_whole(
        constrained,
        "CREATE (:U {key: 'c'}), (:U {key: 'c'})",
        "22N79",
        "MATCH (u:U) RETURN u.key AS k ORDER BY k",
        json!([{"k": "a"}, {"k": "b"}]),
    );
}

#[test]
fn a_set_that_collides_on_a_later_node_changes_none() {
    assert_rejected_whole(
        constrained,
        "MATCH (u:U) SET u.key = 'same'",
        "22N79",
        "MATCH (u:U) RETURN u.key AS k ORDER BY k",
        json!([{"k": "a"}, {"k": "b"}]),
    );
}

#[test]
fn a_missing_required_property_on_a_later_row_keeps_no_row() {
    assert_rejected_whole(
        constrained,
        "UNWIND [1, null] AS v CREATE (:N {v: v})",
        "22N77",
        "MATCH (n:N) RETURN count(n) AS c",
        json!([{"c": 0}]),
    );
}

#[test]
fn a_delete_that_meets_a_relationship_deletes_nothing() {
    fn graph() -> TestDb {
        let db = TestDb::new();
        // The node without relationships comes first, so it would be
        // deleted before the second one fails.
        db.run("CREATE (:D {i: 1}), (a:D {i: 2}), (a)-[:R]->(:E)");
        db
    }
    assert_rejected_whole(
        graph,
        "MATCH (d:D) DELETE d",
        "DETACH DELETE",
        "MATCH (d:D) RETURN d.i AS i ORDER BY i",
        json!([{"i": 1}, {"i": 2}]),
    );
}

#[test]
fn writes_that_cannot_fail_still_succeed() {
    for path in WRITE_PATHS {
        let db = constrained();
        for q in [
            "UNWIND ['x', 'y'] AS k CREATE (:U {key: k})",
            "UNWIND [1, 2] AS i CREATE (:Free {i: i})",
            "MATCH (f:Free) SET f.seen = true",
            "MATCH (f:Free {i: 2}) DETACH DELETE f",
        ] {
            try_on(&db.service, path, q).unwrap_or_else(|e| panic!("{path:?}: `{q}`: {e}"));
        }
        assert_eq!(
            read(&db, "MATCH (u:U) RETURN count(u) AS c"),
            json!([{"c": 4}]),
            "{path:?}"
        );
        assert_eq!(
            read(&db, "MATCH (f:Free) RETURN f.i AS i, f.seen AS seen"),
            json!([{"i": 1, "seen": true}]),
            "{path:?}"
        );
    }
}
