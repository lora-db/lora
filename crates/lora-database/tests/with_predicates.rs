//! A `WITH ... WHERE` predicate that reads a projected variable only inside
//! a pattern (`size([(a)<-[:T]-(x) | 1]) = 0`, `EXISTS { (a)--() }`) must
//! stay above the projection that binds the variable. The optimizer's
//! variable walk skipped patterns, so such a predicate was pushed below the
//! projection and ran with the variable unbound: the pattern matched every
//! relationship of the type, the query returned no rows, and it cost a
//! scan per row (Festimap brief G-11: 0 rows in ~1 s instead of 125 in
//! 0.5 ms).

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraValue, PlanTreeNode};
use serde_json::{json, Value};
use test_helpers::TestDb;

fn graph() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT m_key IF NOT EXISTS FOR (n:M) REQUIRE n.key IS UNIQUE");
    db.run(
        "UNWIND range(1, 40) AS i CREATE (m:M {key: toString(i)}) \
         WITH m, i WHERE i % 2 = 0 CREATE (:P)-[:SENT]->(m)",
    );
    db
}

fn keys(db: &TestDb, query: &str) -> Vec<Value> {
    let mut params = BTreeMap::new();
    params.insert(
        "keys".to_string(),
        LoraValue::List((1..=6).map(|i| LoraValue::String(i.to_string())).collect()),
    );
    let mut out: Vec<Value> = db
        .run_with_params(query, params)
        .into_iter()
        .map(|r| r["k"].clone())
        .collect();
    out.sort_by_key(|v| v.as_str().unwrap_or_default().parse::<i64>().unwrap_or(0));
    out
}

fn odd() -> Vec<Value> {
    vec![json!("1"), json!("3"), json!("5")]
}

#[test]
fn pattern_comprehension_after_with_sees_the_projected_variable() {
    let db = graph();
    let written = "UNWIND $keys AS k MATCH (a:M) WHERE a.key = k \
                   WITH a WHERE size([(a)<-[:SENT]-(x) WHERE x:P | 1]) = 0 RETURN a.key AS k";
    let bound = "UNWIND $keys AS k MATCH (a:M) WHERE a.key = k \
                 WITH a, size([(a)<-[:SENT]-(x) WHERE x:P | 1]) AS c WHERE c = 0 RETURN a.key AS k";
    assert_eq!(keys(&db, written), odd());
    assert_eq!(keys(&db, bound), odd());
}

#[test]
fn without_a_seek_and_with_renames_too() {
    let db = graph();
    assert_eq!(
        keys(
            &db,
            "MATCH (a:M) WHERE a.key IN $keys WITH a WHERE size([(a)<-[:SENT]-() | 1]) = 0 RETURN a.key AS k"
        ),
        odd()
    );
    assert_eq!(
        keys(
            &db,
            "MATCH (a:M) WHERE a.key IN $keys WITH a AS b WHERE size([(b)<-[:SENT]-() | 1]) = 0 RETURN b.key AS k"
        ),
        odd()
    );
    assert_eq!(
        keys(
            &db,
            "MATCH (a:M) WHERE a.key IN $keys WITH a WHERE NOT EXISTS { (a)<-[:SENT]-() } RETURN a.key AS k"
        ),
        odd()
    );
}

fn operators(node: &PlanTreeNode, out: &mut Vec<String>) {
    out.push(node.operator.clone());
    for child in &node.children {
        operators(child, out);
    }
}

#[test]
fn the_seek_feeding_the_with_is_kept() {
    let db = graph();
    let plan = db
        .service
        .explain(
            "UNWIND $keys AS k MATCH (a:M) WHERE a.key = k \
             WITH a WHERE size([(a)<-[:SENT]-(x) | 1]) = 0 RETURN a.key AS k",
            None,
        )
        .unwrap();
    let mut ops = Vec::new();
    operators(&plan.tree.root, &mut ops);
    assert!(ops.iter().any(|o| o == "NodeByPropertyScan"), "{ops:?}");
    assert!(!ops.iter().any(|o| o == "NodeScan"), "{ops:?}");
    // RETURN's projection, then the pattern predicate, then the WITH's
    // projection that binds `a`: the predicate stays above it.
    let with_projection = ops
        .iter()
        .enumerate()
        .filter(|(_, o)| *o == "Projection")
        .nth(1)
        .map(|(i, _)| i)
        .unwrap();
    let first_filter = ops.iter().position(|o| o == "Filter").unwrap();
    assert!(first_filter < with_projection, "{ops:?}");
}
