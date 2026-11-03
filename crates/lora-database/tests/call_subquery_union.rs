//! `UNION` inside `CALL { ... }` (G-79).
//!
//! Before the fix the analyzer refused any `CALL` body with a `UNION`
//! ("UNION inside CALL { ... } is not yet supported"), so a statement
//! reading every implementation of a GraphQL interface had to give each
//! one a shared second label. Each branch now sees the outer scope, runs
//! once per outer row, and returns the head branch's columns.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{Database, InMemoryGraph, LoraValue, PlanShape};
use serde_json::json;
use test_helpers::TestDb;

fn media() -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE (:ImageFile {key: 'i1', size: 10}), (:ImageFile {key: 'i2', size: 30}), \
                (:VideoFile {key: 'v1', size: 20}), (:AudioFile {key: 'a1', size: 40})",
    );
    db
}

#[test]
fn union_of_labels_answers_every_branch() {
    // The Festimap probe: one row per implementation.
    let db = TestDb::new();
    db.run("CREATE (:ImageFile {key: 'i'}), (:VideoFile {key: 'v'})");
    let rows = db.run(
        "CALL { MATCH (f:ImageFile) RETURN f UNION MATCH (f:VideoFile) RETURN f } \
         RETURN f.key AS key ORDER BY key",
    );
    assert_eq!(rows, vec![json!({"key": "i"}), json!({"key": "v"})]);
}

#[test]
fn union_pages_and_counts_across_branches() {
    let db = media();
    let page = db.run(
        "CALL { MATCH (f:ImageFile) RETURN f \
                UNION ALL MATCH (f:VideoFile) RETURN f \
                UNION ALL MATCH (f:AudioFile) RETURN f } \
         WITH f ORDER BY f.size DESC SKIP 1 LIMIT 2 RETURN f.key AS key",
    );
    assert_eq!(page, vec![json!({"key": "i2"}), json!({"key": "v1"})]);

    let total = db.scalar(
        "CALL { MATCH (f:ImageFile) RETURN f \
                UNION ALL MATCH (f:VideoFile) RETURN f \
                UNION ALL MATCH (f:AudioFile) RETURN f } \
         RETURN count(f) AS n",
    );
    assert_eq!(total, json!(4));
}

#[test]
fn union_deduplicates_and_union_all_keeps_duplicates() {
    let db = media();
    let distinct = db.run(
        "CALL { MATCH (f:ImageFile) RETURN 'x' AS kind UNION MATCH (f:VideoFile) RETURN 'x' AS kind } \
         RETURN kind",
    );
    assert_eq!(distinct, vec![json!({"kind": "x"})]);

    let all = db.run(
        "CALL { MATCH (f:ImageFile) RETURN 'x' AS kind UNION ALL MATCH (f:VideoFile) RETURN 'x' AS kind } \
         RETURN kind",
    );
    assert_eq!(all.len(), 3);
}

#[test]
fn union_deduplicates_per_outer_row() {
    // The same inner row under two outer rows is kept once for each.
    let db = TestDb::new();
    let rows = db.run(
        "UNWIND [1, 2] AS i \
         CALL { RETURN 'same' AS v UNION RETURN 'same' AS v } \
         RETURN i, v ORDER BY i",
    );
    assert_eq!(
        rows,
        vec![json!({"i": 1, "v": "same"}), json!({"i": 2, "v": "same"})]
    );
}

#[test]
fn every_branch_sees_the_outer_row() {
    let db = TestDb::new();
    db.run(
        "CREATE (a:P {name: 'a'})-[:LIKES]->(:T {name: 'x'}), \
                (a)-[:OWNS]->(:T {name: 'y'}), \
                (b:P {name: 'b'})-[:OWNS]->(:T {name: 'z'})",
    );
    let rows = db.run(
        "MATCH (p:P) \
         CALL { WITH p MATCH (p)-[:LIKES]->(t) RETURN t.name AS thing \
                UNION \
                WITH p MATCH (p)-[:OWNS]->(t) RETURN t.name AS thing } \
         RETURN p.name AS p, thing ORDER BY p, thing",
    );
    assert_eq!(
        rows,
        vec![
            json!({"p": "a", "thing": "x"}),
            json!({"p": "a", "thing": "y"}),
            json!({"p": "b", "thing": "z"}),
        ]
    );
}

#[test]
fn union_with_limit_on_each_branch() {
    let db = TestDb::new();
    db.seed_org_graph();
    let rows = db.run(
        "CALL { MATCH (p:Person) RETURN p.name AS name ORDER BY name LIMIT 2 \
         UNION ALL \
         MATCH (c:City) RETURN c.name AS name ORDER BY name LIMIT 2 } \
         RETURN name",
    );
    assert_eq!(rows.len(), 4);
}

#[test]
fn branch_columns_map_by_name_onto_the_head() {
    // Branches bind different variables; the outer query reads one column.
    let db = media();
    let rows = db.run(
        "CALL { MATCH (i:ImageFile) RETURN i.key AS key, i.size AS size \
                UNION ALL MATCH (v:VideoFile) RETURN v.key AS key, v.size AS size } \
         RETURN key, size ORDER BY size",
    );
    assert_eq!(
        rows,
        vec![
            json!({"key": "i1", "size": 10}),
            json!({"key": "v1", "size": 20}),
            json!({"key": "i2", "size": 30}),
        ]
    );
}

#[test]
fn a_branch_does_not_see_the_previous_branch_bindings() {
    let db = media();
    let err = db.run_err(
        "CALL { MATCH (i:ImageFile) RETURN i.key AS key \
                UNION MATCH (v:VideoFile) RETURN i.key AS key } \
         RETURN key",
    );
    assert!(err.contains("unknown variable `i`"), "{err}");
}

#[test]
fn branches_must_return_the_same_columns() {
    let db = media();
    let err = db.run_err(
        "CALL { MATCH (f:ImageFile) RETURN f.key AS key UNION MATCH (f:VideoFile) RETURN f.key AS k } \
         RETURN key",
    );
    assert!(err.to_lowercase().contains("union"), "{err}");

    let err = db.run_err(
        "CALL { MATCH (f:ImageFile) RETURN f.key AS key \
                UNION MATCH (f:VideoFile) RETURN f.key AS key, f.size AS size } \
         RETURN key",
    );
    assert!(err.to_lowercase().contains("union"), "{err}");
}

#[test]
fn every_branch_must_end_in_return() {
    let db = media();
    let err = db.run_err("CALL { CREATE (:X) UNION CREATE (:Y) } RETURN 1 AS one");
    assert!(err.contains("RETURN"), "{err}");
}

#[test]
fn branches_may_write() {
    let db = TestDb::new();
    let rows = db.run(
        "UNWIND [1, 2] AS i \
         CALL { WITH i CREATE (x:X {i: i}) RETURN x.i AS v \
                UNION ALL \
                WITH i CREATE (y:Y {i: i}) RETURN y.i * 10 AS v } \
         RETURN i, v ORDER BY i, v",
    );
    assert_eq!(
        rows,
        vec![
            json!({"i": 1, "v": 1}),
            json!({"i": 1, "v": 10}),
            json!({"i": 2, "v": 2}),
            json!({"i": 2, "v": 20}),
        ]
    );
    db.assert_count("MATCH (x:X) RETURN x", 2);
    db.assert_count("MATCH (y:Y) RETURN y", 2);
}

#[test]
fn a_reading_union_is_read_only_and_a_writing_one_mutates() {
    let db: Database<InMemoryGraph> = Database::in_memory();
    let read = db
        .explain(
            "CALL { MATCH (f:A) RETURN f UNION MATCH (f:B) RETURN f } RETURN f",
            None,
        )
        .unwrap();
    assert_eq!(read.shape, PlanShape::ReadOnly);
    let write = db
        .explain(
            "CALL { MATCH (f:A) RETURN f UNION CREATE (f:B) RETURN f } RETURN f",
            None,
        )
        .unwrap();
    assert_eq!(write.shape, PlanShape::Mutating);
}

#[test]
fn union_in_call_streams_through_db_stream() {
    let db: Database<InMemoryGraph> = Database::in_memory();
    db.execute("CREATE (:A {k: 1}), (:B {k: 2}), (:B {k: 3})", None)
        .unwrap();
    let rows: Vec<_> = db
        .stream_with_params(
            "CALL { MATCH (f:A) RETURN f UNION MATCH (f:B) RETURN f } RETURN f.k AS k",
            BTreeMap::<String, LoraValue>::new(),
        )
        .unwrap()
        .collect();
    assert_eq!(rows.len(), 3);
}
