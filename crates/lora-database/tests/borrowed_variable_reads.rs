//! Reading part of a carried list or map does not copy the whole value.
//!
//! `x IN big`, `big[i]`, `big[a..b]`, `doc.key` and `doc {.key}` evaluated
//! the bare variable first, which copied the whole list or map, and then read
//! one part of it. A comprehension evaluates its WHERE per element, so
//! `[i IN range(1, 200) WHERE i IN big]` copied `big` 200 times even when
//! every element was found among the first few.

mod test_helpers;
use std::time::Instant;

use test_helpers::TestDb;

fn hub(members: usize) -> TestDb {
    let db = TestDb::new();
    db.run(&format!(
        "CREATE (c:C {{key: 'c'}}) WITH c UNWIND range(1, {members}) AS i \
         CREATE (:M {{i: i}})-[:IN]->(c)"
    ));
    db
}

#[test]
fn reads_of_carried_values_keep_their_results() {
    let db = hub(5);
    let rows = db.run(
        "MATCH (c:C) WITH c, [3, 4, null] AS l, {k: 2, n: {deep: [7, 8]}} AS doc, \
                          range(1, 100) AS big, null AS nothing \
         RETURN size([(c)<-[:IN]-(m:M) WHERE m.i IN l | 1]) AS inl, \
                9 IN l AS missing, 1 IN nothing AS innull, nothing.k AS nullprop, \
                big[0] AS first, big[-1] AS last, big[200] AS out, big[98..] AS tail, \
                doc.k AS k, doc['k'] AS kk, doc.n.deep[1] AS deep, doc {.k} AS proj, \
                [(c)<-[:IN]-(m:M) WHERE m.i = doc.k | m.i] AS bydoc",
    );
    let r = &rows[0];
    assert_eq!(r["inl"], 2);
    assert_eq!(r["missing"], false);
    assert_eq!(r["innull"], serde_json::Value::Null);
    assert_eq!(r["nullprop"], serde_json::Value::Null);
    assert_eq!(r["first"], 1);
    assert_eq!(r["last"], 100);
    assert_eq!(r["out"], serde_json::Value::Null);
    assert_eq!(r["tail"], serde_json::json!([99, 100]));
    assert_eq!(r["k"], 2);
    assert_eq!(r["kk"], 2);
    assert_eq!(r["deep"], 8);
    assert_eq!(r["proj"], serde_json::json!({"k": 2}));
    assert_eq!(r["bydoc"], serde_json::json!([2]));
}

fn per_query(db: &TestDb, carried: usize) -> f64 {
    // Every element is found among the first 200, so only copying the list
    // (or the map) per iteration would make the cost depend on its length.
    let q = format!(
        "WITH range(1, {carried}) AS big, {{k: 1, rest: range(1, {carried})}} AS doc \
         RETURN size([i IN range(1, 200) \
                      WHERE i IN big AND big[0] = 1 AND size(big[0..2]) = 2 AND doc.k = 1 \
                      | i]) AS n"
    );
    db.run(&q);
    // The fastest of several runs: robust against a loaded host.
    (0..7)
        .map(|_| {
            let start = Instant::now();
            db.run(&q);
            start.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min)
}

#[test]
fn partial_reads_cost_is_independent_of_the_value_size() {
    let db = TestDb::new();
    let small = per_query(&db, 200);
    let large = per_query(&db, 20_000);
    // Copying the list and the map per element made this ~50x slower.
    assert!(
        large < small * 10.0 + 0.02,
        "200-element values {small:.5}s, 20k-element values {large:.5}s"
    );
}
