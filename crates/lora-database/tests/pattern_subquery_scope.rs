//! A pattern comprehension or EXISTS subquery works on the bindings it reads,
//! not on a copy of the whole outer row.
//!
//! Every candidate and every expansion step of a pattern subquery clones its
//! working row. That row used to be the full outer row, so a comprehension's
//! cost grew with every other value the row carried — a list collected by an
//! earlier CALL made a 200-row comprehension ~100× slower, and nesting
//! multiplied it (E-4 in Festimap's brief: a GraphQL page with counts after
//! its collected lists took 40 ms instead of 3).

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
fn outer_bindings_stay_visible_in_pattern_where_and_projection() {
    let db = hub(10);
    // The pattern names an outer node, WHERE and the projection read an outer
    // value, and a nested comprehension reads both levels.
    let rows = db.run(
        "MATCH (c:C {key: 'c'}) WITH c, 5 AS floor, 'x' AS tag, range(1, 1000) AS unused \
         RETURN [(c)<-[:IN]-(m:M) WHERE m.i > floor | tag + toString(m.i)] AS picked, \
                size([(c)<-[:IN]-(m:M) WHERE size([(m)-[:IN]->(o:C) WHERE o = c AND m.i > floor | 1]) > 0 | 1]) AS nested, \
                size(unused) AS kept",
    );
    let mut picked: Vec<String> = rows[0]["picked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    picked.sort();
    assert_eq!(picked, ["x10", "x6", "x7", "x8", "x9"]);
    assert_eq!(rows[0]["nested"], 5);
    assert_eq!(rows[0]["kept"], 1000);
}

#[test]
fn a_bound_relationship_or_path_variable_still_restricts_the_pattern() {
    let db = hub(3);
    let rows = db.run(
        "MATCH (m:M {i: 2})-[r:IN]->(c:C) \
         RETURN size([(m)-[r]->(x:C) | 1]) AS same, \
                size([(n:M)-[:IN]->(c) WHERE n <> m | n.i]) AS others",
    );
    assert_eq!(rows[0]["same"], 1);
    assert_eq!(rows[0]["others"], 2);
}

fn per_query(db: &TestDb, carried: usize) -> f64 {
    let q = format!(
        "MATCH (c:C {{key: 'c'}}) WITH c, range(1, {carried}) AS big \
         RETURN size([(c)<-[:IN]-(m:M) WHERE size([(m)-[:IN]->(x:C) WHERE x.key = 'c' | 1]) > 0 | 1]) AS n, \
                size(big) AS b"
    );
    db.run(&q);
    let start = Instant::now();
    for _ in 0..5 {
        db.run(&q);
    }
    start.elapsed().as_secs_f64() / 5.0
}

#[test]
fn comprehension_cost_is_independent_of_other_row_values() {
    let db = hub(200);
    let small = per_query(&db, 0);
    let large = per_query(&db, 20_000);
    // Copying the carried list per expansion made the large case ~100x slower.
    assert!(
        large < small * 4.0 + 0.002,
        "no carried list {small:.5}s, 20k-element list {large:.5}s"
    );
}
