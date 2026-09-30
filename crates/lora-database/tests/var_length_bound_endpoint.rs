//! A variable-length pattern whose far endpoint is already bound matches
//! only paths that end at that node.
//!
//! The expansion wrote each reached node into the endpoint's slot,
//! overwriting the bound value, so `MATCH (c)-[:IN*1..1]-(m)` with `m`
//! bound returned a row for every neighbour of `c`; `shortestPath` and
//! `allShortestPaths` likewise.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

fn star() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE (:C {k: 'hub'})");
    db.run("UNWIND range(1, 200) AS i MATCH (c:C) CREATE (c)-[:IN]->(:M {i: i})");
    db
}

#[test]
fn a_bound_endpoint_is_checked_not_rebound() {
    let db = star();
    for q in [
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (c)-[:IN*1..1]-(m) RETURN count(*) AS n",
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (c)-[:IN*1..3]-(m) RETURN count(*) AS n",
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (c)-[:IN*]->(m) RETURN count(*) AS n",
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (m)-[:IN*1..1]-(c) RETURN count(*) AS n",
        "MATCH (c:C), (m:M {i: 1}) MATCH (c)-[r:IN*1..1]-(m) RETURN count(r) AS n",
        // The fixed-length pattern already did this right.
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (c)-[:IN]-(m) RETURN count(*) AS n",
    ] {
        assert_eq!(db.scalar(q), json!(1), "{q}");
    }
    // The endpoint keeps its value.
    assert_eq!(
        db.scalar(
            "MATCH (c:C) MATCH (m:M {i: 7}) WITH c, m MATCH (c)-[:IN*1..1]-(m) RETURN m.i AS i"
        ),
        json!(7)
    );
    // A bound endpoint the pattern cannot reach gives no row.
    assert_eq!(
        db.scalar(
            "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (c)<-[:IN*1..2]-(m) \
             RETURN count(*) AS n"
        ),
        json!(0)
    );
}

#[test]
fn shortest_paths_to_a_bound_endpoint() {
    let db = star();
    for q in [
        "MATCH (c:C), (m:M {i: 1}) MATCH p = shortestPath((c)-[:IN*]-(m)) RETURN count(p) AS n",
        "MATCH (c:C), (m:M {i: 1}) MATCH p = allShortestPaths((c)-[:IN*]-(m)) RETURN count(p) AS n",
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m \
         MATCH p = allShortestPaths((c)-[:IN*]-(m)) RETURN count(p) AS n",
    ] {
        assert_eq!(db.scalar(q), json!(1), "{q}");
    }
}

#[test]
fn a_write_over_a_streamed_expansion_sees_one_match() {
    // A write streams its input through the pull pipeline, the other
    // variable-length implementation.
    let db = star();
    db.run(
        "MATCH (c:C) MATCH (m:M {i: 1}) WITH c, m MATCH (c)-[:IN*1..2]-(m) \
         CREATE (:Hit {i: m.i})",
    );
    assert_eq!(db.scalar("MATCH (h:Hit) RETURN count(h) AS n"), json!(1));
    assert_eq!(db.scalar("MATCH (h:Hit) RETURN h.i AS i"), json!(1));
}
