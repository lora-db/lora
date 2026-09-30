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

#[test]
fn outer_bindings_reach_every_part_of_a_subquery() {
    let db = hub(10);
    let rows = db.run(
        "MATCH p = (m0:M {i: 2})-[r:IN]->(c:C {key: 'c'}) \
         WITH c, r, p, 3 AS k, 'x' AS tag, [2, 3] AS l, {lim: 4} AS cfg \
         UNWIND [5, 6] AS u \
         RETURN u, \
           [(c)<-[:IN]-(m:M {i: k}) | m.i] AS inline_var, \
           [(c)<-[:IN]-(m:M {i: cfg.lim}) | m.i] AS inline_prop, \
           [(c)<-[:IN]-(m:M) WHERE m.i = 1 | m {kk: k}] AS map_proj, \
           [(c)<-[:IN]-(m:M) WHERE m.i = 1 | reduce(s = k, x IN [1] | s + x)] AS reduce_init, \
           [(c)<-[:IN]-(m:M) WHERE m.i <= 3 | CASE WHEN m.i = k THEN tag ELSE null END] AS case_expr, \
           size([(c)<-[:IN]-(m:M) WHERE any(x IN l WHERE x = m.i) | 1]) AS any_where, \
           size([(c)<-[:IN]-(m:M) WHERE size([x IN l WHERE x = m.i]) > 0 | 1]) AS comp_where, \
           [(c)<-[:IN]-(m:M) WHERE m.i = k | COUNT { (m)-[:IN]->(c) }] AS nested_count, \
           [(c)<-[:IN]-(m:M) WHERE m.i = l[0] | m.i] AS index, \
           [(c)<-[:IN]-(m:M) WHERE m.i IN l[1..] | m.i] AS slice, \
           [(c)<-[s:IN]-(m:M) WHERE s = r | m.i] AS same_rel, \
           size([(c)<-[s:IN]-(m:M) WHERE type(s) = type(r) | 1]) AS same_type, \
           [(c)<-[:IN]-(m:M) WHERE m.i = nodes(p)[0].i | m.i] AS path_node, \
           [(c)<-[:IN]-(m:M) WHERE m.i = u | m.i] AS unwound, \
           EXISTS { (m:M {i: k})-[:IN]->(:C) } AS exists_inline, \
           EXISTS { (m:M {i: u + 10})-[:IN]->(:C) } AS exists_missing \
         ORDER BY u",
    );
    assert_eq!(rows.len(), 2);
    for (row, u) in rows.iter().zip([5, 6]) {
        assert_eq!(row["u"], u);
        assert_eq!(row["inline_var"], serde_json::json!([3]));
        assert_eq!(row["inline_prop"], serde_json::json!([4]));
        assert_eq!(row["map_proj"], serde_json::json!([{"kk": 3}]));
        assert_eq!(row["reduce_init"], serde_json::json!([4]));
        let mut case_expr: Vec<serde_json::Value> = row["case_expr"].as_array().unwrap().clone();
        case_expr.sort_by_key(|v| v.is_null());
        assert_eq!(
            case_expr,
            [
                serde_json::json!("x"),
                serde_json::Value::Null,
                serde_json::Value::Null
            ]
        );
        assert_eq!(row["any_where"], 2);
        assert_eq!(row["comp_where"], 2);
        assert_eq!(row["nested_count"], serde_json::json!([1]));
        assert_eq!(row["index"], serde_json::json!([2]));
        assert_eq!(row["slice"], serde_json::json!([3]));
        assert_eq!(row["same_rel"], serde_json::json!([2]));
        assert_eq!(row["same_type"], 10);
        assert_eq!(row["path_node"], serde_json::json!([2]));
        assert_eq!(row["unwound"], serde_json::json!([u]));
        assert_eq!(row["exists_inline"], true);
        assert_eq!(row["exists_missing"], false);
    }
}

fn per_query(db: &TestDb, carried: usize) -> f64 {
    let q = format!(
        "MATCH (c:C {{key: 'c'}}) WITH c, range(1, {carried}) AS big \
         RETURN size([(c)<-[:IN]-(m:M) WHERE size([(m)-[:IN]->(x:C) WHERE x.key = 'c' | 1]) > 0 | 1]) AS n, \
                size(big) AS b"
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
fn comprehension_cost_is_independent_of_other_row_values() {
    let db = hub(200);
    let small = per_query(&db, 0);
    let large = per_query(&db, 20_000);
    // Copying the carried list per expansion made the large case ~100x slower
    // (~500 ms in a debug build); the bound leaves room for a loaded CI host.
    assert!(
        large < small * 20.0 + 0.05,
        "no carried list {small:.5}s, 20k-element list {large:.5}s"
    );
}
