//! Existence constraints judge the graph as the statement leaves it
//! (E20 residual): a node created and deleted in one statement needs no
//! property, `SET n:Label, n.required = …` supplies the property after the
//! label, `REMOVE n.required SET n.required = …` puts it back, and so do
//! `SET n = {…}, n.required = …` and `SET n += {required: null},
//! n.required = …`, on nodes and relationships alike. A
//! statement that ends without the property is still rejected, and keeps
//! nothing it wrote.
//!
//! Regression: `CREATE (r:R) DELETE r` was checked at the CREATE, and
//! `SET q:R` checked existence the moment the label was added, so both
//! failed although the statement's result satisfies the constraint. Every
//! case runs on each write path (`execute`, the pull pipeline, an explicit
//! transaction).

mod test_helpers;

use lora_database::{Database, ExecuteOptions, InMemoryGraph, ResultFormat, TransactionMode};
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

#[derive(Clone, Copy, Debug)]
enum Path {
    Execute,
    Stream,
    Tx,
}

const WRITE_PATHS: [Path; 3] = [Path::Execute, Path::Stream, Path::Tx];

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
    JsonValue::Array(db.run(query))
}

fn constrained() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT rx FOR (r:R) REQUIRE r.x IS NOT NULL");
    db.run("CREATE CONSTRAINT ru FOR (r:R) REQUIRE r.u IS UNIQUE");
    db.run("CREATE (:R {x: 0, u: 'taken'}), (:Q {k: 1})");
    db
}

fn assert_accepted(query: &str, check: &str, expected: JsonValue) {
    for path in WRITE_PATHS {
        let db = constrained();
        try_on(&db.service, path, query).unwrap_or_else(|e| panic!("{path:?}: `{query}`: {e}"));
        assert_eq!(read(&db, check), expected, "{path:?}: `{query}`");
    }
}

fn assert_rejected(query: &str, code: &str) {
    for path in WRITE_PATHS {
        let db = constrained();
        let err = try_on(&db.service, path, query).expect_err(query);
        assert!(err.contains(code), "{path:?}: `{query}`: {err}");
        assert_eq!(
            read(
                &db,
                "MATCH (n) RETURN labels(n) AS l, properties(n) AS p ORDER BY p.k, p.x"
            ),
            json!([
                {"l": ["Q"], "p": {"k": 1}},
                {"l": ["R"], "p": {"x": 0, "u": "taken"}},
            ]),
            "{path:?}: `{query}` changed the graph"
        );
    }
}

#[test]
fn a_node_deleted_in_the_same_statement_needs_no_property() {
    assert_accepted(
        "CREATE (r:R) DELETE r",
        "MATCH (r:R) RETURN count(r) AS c",
        json!([{"c": 1}]),
    );
    assert_accepted(
        "UNWIND [1, 2] AS i CREATE (r:R {i: i}) DELETE r",
        "MATCH (r:R) RETURN count(r) AS c",
        json!([{"c": 1}]),
    );
}

#[test]
fn a_label_and_its_required_property_set_together() {
    assert_accepted(
        "MATCH (q:Q) SET q:R, q.x = 1",
        "MATCH (r:R) RETURN r.x AS x ORDER BY x",
        json!([{"x": 0}, {"x": 1}]),
    );
    assert_accepted(
        "MATCH (q:Q) SET q:R SET q.x = 2",
        "MATCH (r:R) RETURN r.x AS x ORDER BY x",
        json!([{"x": 0}, {"x": 2}]),
    );
    assert_accepted(
        "CREATE (q:Q) SET q:R, q.x = 3",
        "MATCH (r:R) RETURN r.x AS x ORDER BY x",
        json!([{"x": 0}, {"x": 3}]),
    );
}

#[test]
fn a_required_property_removed_and_set_again() {
    assert_accepted(
        "MATCH (r:R) REMOVE r.x SET r.x = 9",
        "MATCH (r:R) RETURN r.x AS x",
        json!([{"x": 9}]),
    );
}

#[test]
fn a_statement_that_ends_without_the_property_is_rejected_whole() {
    assert_rejected("MATCH (q:Q) SET q:R", "22N77");
    assert_rejected("MATCH (q:Q) SET q:R, q.other = 1", "22N77");
    assert_rejected("MATCH (r:R) REMOVE r.x", "22N77");
    assert_rejected("MATCH (r:R) SET r.x = null", "22N77");
    assert_rejected("CREATE (a:R), (b:R {x: 1}) DELETE b", "22N77");
}

#[test]
fn uniqueness_is_still_checked_when_the_label_is_added() {
    // Deferring existence doesn't defer uniqueness: the duplicate key is
    // rejected as soon as the label makes the constraint apply.
    assert_rejected("MATCH (q:Q) SET q.u = 'taken', q.x = 1, q:R", "22N79");
    assert_rejected("MATCH (q:Q) SET q:R, q.u = 'taken', q.x = 1", "22N79");
}

#[test]
fn a_map_replace_and_its_required_property_set_together() {
    assert_accepted(
        "MATCH (r:R) SET r = {y: 1}, r.x = 2",
        "MATCH (r:R) RETURN properties(r) AS p",
        json!([{"p": {"x": 2, "y": 1}}]),
    );
    assert_accepted(
        "MATCH (r:R) SET r = {y: 1} SET r.x = 3",
        "MATCH (r:R) RETURN properties(r) AS p",
        json!([{"p": {"x": 3, "y": 1}}]),
    );
    assert_accepted(
        "MATCH (r:R) SET r += {x: null}, r.x = 4",
        "MATCH (r:R) RETURN properties(r) AS p",
        json!([{"p": {"x": 4, "u": "taken"}}]),
    );
    assert_accepted(
        "MATCH (q:Q) SET q = {}, q:R, q.x = 5",
        "MATCH (r:R) RETURN r.x AS x ORDER BY x",
        json!([{"x": 0}, {"x": 5}]),
    );
}

#[test]
fn a_map_replace_that_ends_without_the_property_is_rejected_whole() {
    assert_rejected("MATCH (r:R) SET r = {y: 1}", "22N77");
    assert_rejected("MATCH (r:R) SET r = {y: 1} SET r.y = 2", "22N77");
    assert_rejected("MATCH (r:R) SET r += {x: null}", "22N77");
    // Uniqueness is still checked on the spot.
    assert_rejected("MATCH (q:Q) SET q:R, q = {u: 'taken', x: 1}", "22N79");
}

fn related() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT tw FOR ()-[t:T]-() REQUIRE t.w IS NOT NULL");
    db.run("CREATE (:A)-[:T {w: 0}]->(:B)");
    db
}

#[test]
fn a_relationship_map_replace_is_judged_at_the_end() {
    for path in WRITE_PATHS {
        let db = related();
        try_on(
            &db.service,
            path,
            "MATCH ()-[t:T]->() SET t = {z: 1}, t.w = 2",
        )
        .unwrap_or_else(|e| panic!("{path:?}: {e}"));
        assert_eq!(
            read(&db, "MATCH ()-[t:T]->() RETURN properties(t) AS p"),
            json!([{"p": {"w": 2, "z": 1}}]),
            "{path:?}"
        );

        let db = related();
        let err = try_on(&db.service, path, "MATCH ()-[t:T]->() SET t = {z: 1}")
            .expect_err("a relationship left without w");
        assert!(err.contains("22N77"), "{path:?}: {err}");
        assert_eq!(
            read(&db, "MATCH ()-[t:T]->() RETURN properties(t) AS p"),
            json!([{"p": {"w": 0}}]),
            "{path:?} changed the graph"
        );
    }
}
