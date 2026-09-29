//! OPTIONAL MATCH anchored on an already-bound node expands from that
//! node, like MATCH does, instead of matching the pattern across the whole
//! graph and joining. Correctness first, then a scaling guard.
//!
//! The release-mode benchmark for the Festimap acceptance target (within
//! 3x of the equivalent pattern comprehension at 200 and 20k festivals) is
//! `cargo run --release -p lora-database --example optional_probe`.

mod test_helpers;

use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use test_helpers::TestDb;

fn festivals(db: &TestDb, n: usize, per: usize) {
    db.run(&format!(
        "UNWIND range(0, {}) AS i CREATE (f:Festival {{key: toString(i)}}) \
         WITH f UNWIND range(0, {}) AS j \
         CREATE (:User {{key: f.key + '-' + toString(j)}})-[:FOLLOWS]->(f)",
        n - 1,
        per - 1
    ));
}

fn keys(n: usize) -> BTreeMap<String, LoraValue> {
    let ks = (0..n).map(|i| LoraValue::String(i.to_string())).collect();
    BTreeMap::from([("ks".to_string(), LoraValue::List(ks))])
}

#[test]
fn counts_include_zero_and_match_the_comprehension() {
    let db = TestDb::new();
    festivals(&db, 5, 3);
    db.run("CREATE (:Festival {key: 'lonely'})");
    let rows = db.run(
        "MATCH (f:Festival) OPTIONAL MATCH (f)<-[r:FOLLOWS]-() \
         RETURN f.key AS k, count(r) AS n ORDER BY k",
    );
    let got: Vec<(String, i64)> = rows
        .iter()
        .map(|r| {
            (
                r["k"].as_str().unwrap().to_string(),
                r["n"].as_i64().unwrap(),
            )
        })
        .collect();
    let mut want: Vec<(String, i64)> = (0..5).map(|i| (i.to_string(), 3)).collect();
    want.push(("lonely".into(), 0));
    assert_eq!(got, want);
}

#[test]
fn optional_row_keeps_nulls_for_unmatched_and_all_matches_otherwise() {
    let db = TestDb::new();
    db.run("CREATE (:A {k: 1})-[:R]->(:B {v: 10}), (:A {k: 1})-[:R]->(:B {v: 11}), (:A {k: 2})");
    let rows = db
        .run("MATCH (a:A) OPTIONAL MATCH (a)-[:R]->(b:B) RETURN a.k AS k, b.v AS v ORDER BY k, v");
    let got: Vec<(i64, Option<i64>)> = rows
        .iter()
        .map(|r| (r["k"].as_i64().unwrap(), r["v"].as_i64()))
        .collect();
    assert_eq!(got, vec![(1, Some(10)), (1, Some(11)), (2, None)]);
}

#[test]
fn inner_predicate_can_reference_outer_variables() {
    let db = TestDb::new();
    db.run("CREATE (:P {min: 5})-[:R]->(:Q {v: 3}), (:P {min: 1})-[:R]->(:Q {v: 3})");
    let rows = db.run(
        "MATCH (p:P) OPTIONAL MATCH (p)-[:R]->(q:Q) WHERE q.v > p.min \
         RETURN p.min AS m, q.v AS v ORDER BY m",
    );
    let got: Vec<(i64, Option<i64>)> = rows
        .iter()
        .map(|r| (r["m"].as_i64().unwrap(), r["v"].as_i64()))
        .collect();
    assert_eq!(got, vec![(1, Some(3)), (5, None)]);
}

#[test]
fn uncorrelated_optional_pattern_still_cross_joins() {
    let db = TestDb::new();
    db.run("CREATE (:X {i: 1}), (:X {i: 2}), (:Y {j: 7})");
    let rows = db.run("MATCH (x:X) OPTIONAL MATCH (y:Y) RETURN x.i AS i, y.j AS j ORDER BY i");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r["j"] == 7));
}

#[test]
fn anchored_optional_match_scales_like_match() {
    // Before correlation this ratio was ~1000x at this size.
    let db = TestDb::new();
    festivals(&db, 20_000, 5);
    let optional = "UNWIND $ks AS k MATCH (f:Festival {key: k}) \
                    OPTIONAL MATCH (f)<-[r:FOLLOWS]-() RETURN k, count(r) AS n";
    let comprehension = "UNWIND $ks AS k MATCH (f:Festival {key: k}) \
                         RETURN k, size([(f)<-[:FOLLOWS]-(u) | u]) AS n";
    let time = |q: &str| {
        db.run_with_params(q, keys(100)); // warm plan cache
        let t = Instant::now();
        for _ in 0..5 {
            db.run_with_params(q, keys(100));
        }
        t.elapsed().as_secs_f64()
    };
    let a = time(optional);
    let b = time(comprehension);
    assert!(
        a / b < 10.0,
        "OPTIONAL MATCH {a:.4}s vs comprehension {b:.4}s"
    );
}
