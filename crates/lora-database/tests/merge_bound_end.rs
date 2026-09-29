//! E14: MERGE and pattern comprehensions honour an already-bound end node
//! instead of reusing any edge of the start node, and a MERGE that does
//! not match leaves no half-matched bindings behind for its create path.

mod test_helpers;

use serde_json::Value as JsonValue;
use test_helpers::TestDb;

#[test]
fn merge_honours_a_bound_end_node() {
    let db = TestDb::new();
    db.run("CREATE (:X {k: 1}), (:Y {k: 2}), (:Y {k: 3})");
    db.run("MATCH (x:X), (y:Y {k: 2}) CREATE (x)-[:R]->(y)");

    let rows = db.run("MATCH (x:X), (y:Y {k: 3}) MERGE (x)-[r:R]->(y) RETURN y.k AS k");
    assert_eq!(rows, vec![serde_json::json!({"k": 3})]);
    let targets = |db: &TestDb| -> Vec<i64> {
        let mut v: Vec<i64> = db
            .run("MATCH (:X)-[:R]->(y) RETURN y.k AS k")
            .iter()
            .map(|r| r["k"].as_i64().unwrap())
            .collect();
        v.sort();
        v
    };
    assert_eq!(targets(&db), vec![2, 3]);
    // Running it again matches the new edge instead of adding another.
    db.run("MATCH (x:X), (y:Y {k: 3}) MERGE (x)-[r:R]->(y) RETURN r");
    assert_eq!(targets(&db), vec![2, 3]);
    // ON MATCH / ON CREATE see the right outcome per pair.
    db.run(
        "MATCH (x:X), (y:Y) MERGE (x)<-[r:BACK]-(y) \
         ON CREATE SET r.created = true ON MATCH SET r.matched = true",
    );
    db.run(
        "MATCH (x:X), (y:Y) MERGE (x)<-[r:BACK]-(y) \
         ON CREATE SET r.created2 = true ON MATCH SET r.matched = true",
    );
    let rows = db.run("MATCH (:X)<-[r:BACK]-(y) RETURN r.created2 AS c2, r.matched AS m");
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| r["c2"].is_null() && r["m"] == JsonValue::Bool(true)));
}

#[test]
fn merge_backtracks_over_candidate_paths() {
    let db = TestDb::new();
    // a has two T edges to B nodes; only the second B reaches c.
    db.run(
        "CREATE (a:A {k: 1}), (c:C {k: 9}), (a)-[:T]->(:B {k: 1}), \
         (a)-[:T]->(b2:B {k: 2}), (b2)-[:U]->(c)",
    );
    db.run("MATCH (a:A), (c:C) MERGE (a)-[:T]->(b:B)-[:U]->(c)");
    assert_eq!(db.run("MATCH (b:B) RETURN b").len(), 2, "no new B created");
    assert_eq!(db.run("MATCH ()-[r:U]->() RETURN r").len(), 1);
    // An unbound head is searched too, and a miss creates the whole path
    // instead of reusing a half-matched start node.
    db.run("MERGE (a:A {k: 1})-[:T]->(b:B {k: 2})");
    assert_eq!(db.run("MATCH (a:A) RETURN a").len(), 1);
    db.run("MERGE (a:A {k: 1})-[:T]->(b:B {k: 77})");
    assert_eq!(db.run("MATCH (a:A) RETURN a").len(), 2);
}

#[test]
fn pattern_comprehension_honours_a_bound_end_node() {
    let db = TestDb::new();
    db.run("CREATE (x:X), (y2:Y {k: 2}), (y3:Y {k: 3}), (x)-[:R]->(y2)");
    let rows = db.run("MATCH (x:X), (y:Y) RETURN y.k AS k, [(x)-[:R]->(y) | y.k] AS l ORDER BY k");
    assert_eq!(
        rows,
        vec![
            serde_json::json!({"k": 2, "l": [2]}),
            serde_json::json!({"k": 3, "l": []}),
        ]
    );
    let rows = db.run("MATCH (x:X)-[r:R]->(y:Y) RETURN size([(x)-[r]->(z) | z]) AS n");
    assert_eq!(rows, vec![serde_json::json!({"n": 1})]);
}
