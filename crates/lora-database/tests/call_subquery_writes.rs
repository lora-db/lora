//! Writes inside `CALL { ... }` subqueries (E15).
//!
//! Before the fix every write in a subquery body failed with
//! `LORA_READ_ONLY`, although `explain()` classified the query as
//! mutating: the mutable executor ran the body on the read-only pull
//! pipeline. A body also had to end in `RETURN`, so unit subqueries (a
//! body that ends in an update) were rejected.

mod test_helpers;

use std::collections::BTreeMap;
use std::time::Duration;

use lora_database::{
    Database, ExecuteOptions, InMemoryGraph, LoraErrorCode, LoraValue, PlanShape, ResultFormat,
    TransactionMode,
};
use serde_json::json;
use test_helpers::TestDb;

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run("UNWIND range(1, 3) AS i CREATE (:N {i: i})");
    db
}

#[test]
fn unit_subquery_creates_per_outer_row_and_keeps_the_rows() {
    let db = seeded();
    let rows = db.run(
        "MATCH (n:N) CALL { WITH n CREATE (n)-[:HAS]->(:X {i: n.i}) } \
         RETURN n.i AS i ORDER BY i",
    );
    assert_eq!(
        rows,
        vec![json!({"i": 1}), json!({"i": 2}), json!({"i": 3})]
    );
    db.assert_count("MATCH (n:N)-[:HAS]->(x:X) WHERE x.i = n.i RETURN x", 3);
}

#[test]
fn unit_subquery_keeps_cardinality_when_its_body_fans_out() {
    let db = seeded();
    db.run("UNWIND range(1, 4) AS j CREATE (:T {j: j})");
    let rows = db
        .run("MATCH (n:N) CALL { WITH n MATCH (t:T) CREATE (n)-[:ALL]->(t) } RETURN count(*) AS c");
    assert_eq!(rows, vec![json!({"c": 3})]);
    db.assert_count("MATCH (:N)-[r:ALL]->(:T) RETURN r", 12);
}

#[test]
fn query_may_end_in_a_unit_subquery() {
    let db = seeded();
    let rows = db.run("MATCH (n:N) CALL { WITH n SET n.done = true }");
    assert!(rows.is_empty());
    db.assert_count("MATCH (n:N) WHERE n.done RETURN n", 3);

    // Still rejected: a query that ends in a read.
    assert!(db.run_err("MATCH (n:N)").contains("RETURN"));
    assert!(db
        .run_err("MATCH (n:N) CALL { WITH n MATCH (m) RETURN m }")
        .contains("CALL"));
}

#[test]
fn returning_subquery_with_writes() {
    let db = seeded();
    let rows = db.run(
        "MATCH (n:N) CALL { WITH n CREATE (y:Y {i: n.i * 10}) RETURN y } \
         RETURN n.i AS i, y.i AS yi ORDER BY i",
    );
    assert_eq!(
        rows,
        vec![
            json!({"i": 1, "yi": 10}),
            json!({"i": 2, "yi": 20}),
            json!({"i": 3, "yi": 30})
        ]
    );

    let rows = db.run("CALL { CREATE (z:Z {k: 1}) RETURN z } RETURN z.k AS k");
    assert_eq!(rows, vec![json!({"k": 1})]);

    // An aggregate over the body's writes, per outer row.
    let rows = db.run(
        "MATCH (n:N) CALL { WITH n UNWIND range(1, n.i) AS k CREATE (c:C {k: k}) \
         RETURN count(c) AS made } RETURN n.i AS i, made ORDER BY i",
    );
    assert_eq!(
        rows,
        vec![
            json!({"i": 1, "made": 1}),
            json!({"i": 2, "made": 2}),
            json!({"i": 3, "made": 3})
        ]
    );
}

#[test]
fn merge_set_delete_and_remove_in_subqueries() {
    let db = seeded();
    // MERGE sees the writes of the runs before it: one :Tag per value.
    db.run(
        "UNWIND [1, 2, 1, 2, 3] AS v \
         CALL { WITH v MERGE (t:Tag {v: v}) ON CREATE SET t.hits = 1 ON MATCH SET t.hits = t.hits + 1 }",
    );
    let rows = db.run("MATCH (t:Tag) RETURN t.v AS v, t.hits AS h ORDER BY v");
    assert_eq!(
        rows,
        vec![
            json!({"v": 1, "h": 2}),
            json!({"v": 2, "h": 2}),
            json!({"v": 3, "h": 1})
        ]
    );

    db.run("MATCH (n:N) CALL { WITH n SET n += {tag: 'x'} REMOVE n.i }");
    db.assert_count("MATCH (n:N) WHERE n.tag = 'x' AND n.i IS NULL RETURN n", 3);

    db.run("MATCH (t:Tag) WHERE t.v > 1 CALL { WITH t DETACH DELETE t }");
    db.assert_count("MATCH (t:Tag) RETURN t", 1);
}

#[test]
fn nested_subqueries_and_writes_after_the_call() {
    let db = seeded();
    db.run(
        "MATCH (n:N) CALL { WITH n CALL { WITH n CREATE (n)-[:A]->(:Leaf) } \
         CREATE (n)-[:B]->(:Leaf) } SET n.visited = true",
    );
    db.assert_count("MATCH (:N)-[:A]->(:Leaf) RETURN 1", 3);
    db.assert_count("MATCH (:N)-[:B]->(:Leaf) RETURN 1", 3);
    db.assert_count("MATCH (n:N) WHERE n.visited RETURN n", 3);
}

#[test]
fn explain_and_execution_agree_on_mutating() {
    let db = seeded();
    let q = "MATCH (n:N) CALL { WITH n CREATE (:X) } RETURN count(*) AS c";
    let plan = db.service.explain(q, None).unwrap();
    assert_eq!(plan.shape, PlanShape::Mutating);
    assert_eq!(db.run(q), vec![json!({"c": 3})]);
}

#[test]
fn failure_inside_a_subquery_rolls_back_the_statement() {
    let db = seeded();
    db.run("CREATE CONSTRAINT u FOR (x:U) REQUIRE x.k IS UNIQUE");
    let err = db
        .exec("UNWIND [1, 2, 1] AS k CALL { WITH k CREATE (:U {k: k}) }")
        .expect_err("duplicate key");
    assert_eq!(err.code(), LoraErrorCode::UniqueConstraint, "{err}");
    db.assert_count("MATCH (u:U) RETURN u", 0);
}

#[test]
fn subquery_writes_in_a_transaction_and_under_a_deadline() {
    let db = seeded();
    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute_rows("MATCH (n:N) CALL { WITH n CREATE (n)-[:TX]->(:W) }")
        .unwrap();
    assert_eq!(
        tx.execute_rows("MATCH (:N)-[:TX]->(w:W) RETURN w")
            .unwrap()
            .len(),
        3
    );
    tx.rollback().unwrap();
    db.assert_count("MATCH (w:W) RETURN w", 0);

    let mut tx = db
        .service
        .begin_transaction(TransactionMode::ReadWrite)
        .unwrap();
    tx.execute_rows("MATCH (n:N) CALL { WITH n CREATE (n)-[:TX]->(:W) }")
        .unwrap();
    tx.commit().unwrap();
    db.assert_count("MATCH (w:W) RETURN w", 3);

    let rows = db
        .service
        .execute_with_params_timeout(
            "MATCH (n:N) CALL { WITH n CREATE (:D {i: n.i}) RETURN count(*) AS one } RETURN sum(one) AS s",
            Some(ExecuteOptions {
                format: ResultFormat::Rows,
            }),
            BTreeMap::new(),
            Duration::from_secs(30),
        )
        .unwrap();
    let json = serde_json::to_value(rows).unwrap();
    assert_eq!(json["rows"], json!([{"s": 3}]));

    // A deadline that fires inside the body rolls the whole write back.
    db.run("UNWIND range(1, 20000) AS i CREATE (:Big {i: i})");
    let err = db
        .service
        .execute_with_params_timeout(
            "MATCH (n:N) CALL { WITH n MATCH (a:Big), (b:Big) CREATE (:Pair) }",
            None,
            BTreeMap::new(),
            Duration::from_millis(50),
        )
        .expect_err("times out");
    assert_eq!(err.code(), LoraErrorCode::Timeout, "{err}");
    db.assert_count("MATCH (p:Pair) RETURN p", 0);
}

#[test]
fn subquery_writes_stream_through_db_stream() {
    let db: Database<InMemoryGraph> = Database::in_memory();
    db.execute("UNWIND range(1, 3) AS i CREATE (:N {i: i})", None)
        .unwrap();
    let rows: Vec<_> = db
        .stream_with_params(
            "MATCH (n:N) CALL { WITH n CREATE (y:Y {i: n.i}) RETURN y.i AS yi } RETURN yi ORDER BY yi",
            BTreeMap::<String, LoraValue>::new(),
        )
        .unwrap()
        .collect();
    assert_eq!(rows.len(), 3);
    let count = db.execute_rows("MATCH (y:Y) RETURN y").unwrap().len();
    assert_eq!(count, 3);
}
