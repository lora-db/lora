//! A value the row carries for other reasons is shared, not copied, when an
//! operator makes one row per candidate.
//!
//! An expand, an OPTIONAL MATCH, a variable-length expand, UNWIND, FOREACH and
//! the list constructs each clone their working row per candidate. A list
//! collected earlier in the query (Festimap's E-4: a `CALL` that collects a
//! page before its counts) used to be copied with it, so a 200-row hop over
//! a row carrying a 10k-element list ran ~250x slower than the same hop
//! without it. Large values now sit behind a shared pointer in the row slot,
//! and a lone variable argument (`size(big)`) is read in place.

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

/// The query body after `MATCH (c:C {key: 'c'}) WITH c, <big> AS big`, for
/// each site that makes a row per candidate.
const SITES: &[(&str, &str)] = &[
    (
        "expand",
        "MATCH (c)<-[:IN]-(m:M) RETURN count(m) AS n, size(big) AS b",
    ),
    (
        "optional match",
        "OPTIONAL MATCH (c)<-[:IN]-(m:M) RETURN count(m) AS n, size(big) AS b",
    ),
    (
        "expand + filter",
        "MATCH (c)<-[:IN]-(m:M) WHERE m.i > 10 RETURN count(m) AS n, size(big) AS b",
    ),
    (
        "variable-length expand",
        "MATCH (c)<-[:IN*1..2]-(m:M) RETURN count(m) AS n, size(big) AS b",
    ),
    (
        "shortest path",
        "MATCH (m:M) WITH c, big, m MATCH p = shortestPath((c)-[:IN*..2]-(m)) \
         RETURN count(p) AS n, size(big) AS b",
    ),
    (
        "foreach",
        "FOREACH (i IN range(1, 200) | SET c.x = i) RETURN size(big) AS b",
    ),
    (
        "unwind + where",
        "UNWIND range(1, 200) AS i WITH c, big, i WHERE i > 10 RETURN count(i) AS n, size(big) AS b",
    ),
    (
        "call without importing with",
        "CALL { MATCH (m:M) RETURN m } RETURN count(m) AS n, size(big) AS b",
    ),
    (
        "list comprehension",
        "RETURN size([i IN range(1, 200) WHERE i > 10 | i]) AS n, size(big) AS b",
    ),
    (
        "reduce",
        "RETURN reduce(s = 0, i IN range(1, 200) | s + i) AS n, size(big) AS b",
    ),
    (
        "quantifier",
        "RETURN any(i IN range(1, 200) WHERE i < 0) AS n, size(big) AS b",
    ),
];

fn per_query(db: &TestDb, q: &str) -> f64 {
    db.run(q);
    // The fastest of several runs: robust against a loaded host.
    (0..7)
        .map(|_| {
            let start = Instant::now();
            db.run(q);
            start.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min)
}

fn query(big: &str, body: &str) -> String {
    format!("MATCH (c:C {{key: 'c'}}) WITH c, {big} AS big {body}")
}

#[test]
fn a_carried_list_does_not_multiply_the_cost_of_a_row_per_candidate() {
    let db = hub(200);
    let mut slow = Vec::new();
    for (site, body) in SITES {
        let small = per_query(&db, &query("[]", body));
        let large = per_query(&db, &query("range(1, 10000)", body));
        // Copying the list per candidate made the row-per-candidate sites
        // 200-380x slower (17 ms against 0.07 ms for the expand in a release
        // build); now they run within 2x. Building the list once is the only
        // cost left; the bound leaves room for a loaded host and a debug
        // build.
        if large > small * 5.0 + 0.01 {
            slow.push(format!("{site}: [] {small:.5}s, 10k {large:.5}s"));
        }
    }
    assert!(slow.is_empty(), "{slow:#?}");
}

#[test]
fn a_carried_list_keeps_its_value_through_every_site() {
    let db = hub(20);
    for (site, body) in SITES {
        let rows = db.run(&query("range(1, 50)", body));
        assert_eq!(rows.len(), 1, "{site}");
        assert_eq!(rows[0]["b"], 50, "{site}");
    }
}

#[test]
fn shared_values_are_independent_after_a_copy() {
    let db = hub(3);
    // The carried list and map reach every row intact, and a value built
    // from one row's copy does not leak into another's.
    let rows = db.run(
        "MATCH (c:C {key: 'c'}) WITH c, range(1, 100) AS big, \
           {a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9} AS cfg \
         MATCH (c)<-[:IN]-(m:M) \
         WITH m, big + [m.i] AS mine, cfg \
         RETURN m.i AS i, size(mine) AS n, mine[-1] AS last, mine[0] AS first, cfg.i AS ci \
         ORDER BY i",
    );
    assert_eq!(rows.len(), 3);
    for (row, i) in rows.iter().zip(1..) {
        assert_eq!(row["i"], i);
        assert_eq!(row["n"], 101);
        assert_eq!(row["last"], i);
        assert_eq!(row["first"], 1);
        assert_eq!(row["ci"], 9);
    }

    // A large list returned from many rows is returned whole on each.
    let rows = db.run("WITH range(1, 64) AS big MATCH (m:M) RETURN m.i AS i, big ORDER BY i");
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row["big"].as_array().unwrap().len(), 64);
    }

    // OPTIONAL MATCH with no match null-extends and keeps the carried list.
    let rows = db.run("WITH range(1, 64) AS big OPTIONAL MATCH (x:Nope) RETURN x, size(big) AS b");
    assert_eq!(rows[0]["x"], serde_json::Value::Null);
    assert_eq!(rows[0]["b"], 64);

    // FOREACH over rows carrying a list writes once per element.
    db.run(
        "MATCH (c:C {key: 'c'}) WITH c, range(1, 100) AS big \
         FOREACH (i IN range(1, 5) | CREATE (:F {i: i, b: size(big)}))",
    );
    let rows = db.run("MATCH (f:F) RETURN count(f) AS n, min(f.b) AS b");
    assert_eq!(rows[0]["n"], 5);
    assert_eq!(rows[0]["b"], 100);
}
