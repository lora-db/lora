//! Unknown labels, relationship types and property keys are not errors.
//!
//! Standard Cypher: a pattern naming a label or type that no entity
//! carries matches nothing, and reading a key an entity does not have
//! yields `null`. The same query must give the same *kind* of answer on
//! an empty database, a populated one, and one whose last entity of a
//! kind was just deleted, removed or rolled back. Before this, whether a
//! query errored depended on whether matching data happened to exist.

mod test_helpers;
use lora_database::{Database, ExecuteOptions, ResultFormat, TransactionMode};
use test_helpers::TestDb;

const LABEL_Q: &str = "MATCH (c:Comment) RETURN count(c) AS n";
const TYPE_Q: &str = "MATCH ()-[r:REPLIED]->() RETURN count(r) AS n";
const PROP_Q: &str = "MATCH (x:Control) RETURN x.brandNew AS v";

fn count(db: &TestDb, q: &str) -> i64 {
    db.scalar(q).as_i64().unwrap()
}

fn prop_is_null(db: &TestDb) {
    let rows = db.run(PROP_Q);
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0]["v"].is_null(),
        "expected null, got {:?}",
        rows[0]["v"]
    );
}

/// Every probe that must return an answer (never an error), whatever
/// the store currently holds.
fn assert_answers(db: &TestDb) {
    assert_eq!(count(db, LABEL_Q), 0);
    assert_eq!(count(db, TYPE_Q), 0);
    assert_eq!(
        count(db, "MATCH (c:Comment {key: 'a'}) RETURN count(c) AS n"),
        0
    );
    // OPTIONAL MATCH and UNION over unknown names must behave too:
    // OPTIONAL MATCH keeps the outer row with nulls, UNION concatenates.
    let rows = db.run("MATCH (x:Control) OPTIONAL MATCH (x)-[:REPLIED]->(c:Comment) RETURN c");
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["c"].is_null());
    let rows =
        db.run("MATCH (c:Comment) RETURN c.key AS k UNION MATCH (x:Control) RETURN x.key AS k");
    assert_eq!(rows.len(), 1);
}

fn with_control() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (:Control {key: 'c'})");
    db
}

#[test]
fn empty_database_answers() {
    let db = TestDb::new();
    assert_eq!(count(&db, LABEL_Q), 0);
    assert_eq!(count(&db, TYPE_Q), 0);
    db.assert_count(PROP_Q, 0);
}

#[test]
fn never_written_names_answer() {
    let db = with_control();
    assert_answers(&db);
    prop_is_null(&db);
}

#[test]
fn written_then_deleted_names_answer() {
    let db = with_control();
    db.run("CREATE (:Comment {key: 'a'})-[:REPLIED]->(:Comment {key: 'b'})");
    db.run("MATCH (x:Control) SET x.brandNew = 1");
    db.run("MATCH (c:Comment) DETACH DELETE c");
    db.run("MATCH (x:Control) REMOVE x.brandNew");
    assert_answers(&db);
    prop_is_null(&db);
}

#[test]
fn removed_label_and_property_answer() {
    let db = with_control();
    db.run("MATCH (x:Control) SET x:Comment, x.brandNew = 1");
    db.run("MATCH (x:Control) REMOVE x:Comment, x.brandNew");
    assert_answers(&db);
    prop_is_null(&db);
}

#[test]
fn rolled_back_names_answer() {
    let db = with_control();
    let opts = Some(ExecuteOptions {
        format: ResultFormat::Rows,
    });
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute(
        "CREATE (:Comment {key: 'a'})-[:REPLIED]->(:Comment {key: 'b'})",
        opts,
    )
    .unwrap();
    tx.execute("MATCH (x:Control) SET x.brandNew = 1", opts)
        .unwrap();
    tx.rollback().unwrap();
    assert_answers(&db);
    prop_is_null(&db);
}

#[test]
fn property_written_in_the_same_statement_is_readable() {
    let db = with_control();
    let rows = db.run("CREATE (x:Control {key: 'z', brandNew: 1}) RETURN x.brandNew AS v");
    assert_eq!(rows[0]["v"], 1);
}

#[test]
fn schema_on_an_unused_label_does_not_change_answers() {
    let db = with_control();
    db.run("CREATE CONSTRAINT comment_key IF NOT EXISTS FOR (c:Comment) REQUIRE c.key IS UNIQUE");
    assert_answers(&db);
}

#[test]
fn fresh_database_type_is_unaffected_by_other_handles() {
    // Two independent in-memory databases: the names one of them knows
    // must not leak into (or be required by) the other.
    let a = Database::in_memory();
    a.execute("CREATE (:Comment {key: 'a'})", None).unwrap();
    let b = TestDb::new();
    b.run("CREATE (:Control {key: 'c'})");
    assert_answers(&b);
}
