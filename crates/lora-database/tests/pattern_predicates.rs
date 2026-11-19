//! Pattern forms of existence checks (taste N-02, N-03).
//!
//! `EXISTS { (a)-[:R]->(b) WHERE ... }` takes a WHERE without `MATCH`, as in
//! Neo4j 5, and a bare relationship pattern in an expression,
//! `WHERE NOT (p)-[:BLOCKS]-(q)`, is a boolean meaning the same as
//! `EXISTS { (p)-[:BLOCKS]-(q) }`.

mod test_helpers;
use test_helpers::TestDb;

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE (a:P {name: 'a'}), (b:P {name: 'b'}), (c:P {name: 'c'}), \
                (a)-[:DID]->(:Act {at: 10}), (a)-[:DID]->(:Act {at: 30}), \
                (b)-[:DID]->(:Act {at: 5}), \
                (a)-[:BLOCKS]->(b)",
    );
    db
}

#[test]
fn exists_pattern_form_takes_a_where_without_match() {
    let db = seeded();
    let names = db.sorted_strings(
        "MATCH (p:P) WHERE EXISTS { (p)-[:DID]->(a) WHERE a.at >= 20 } RETURN p.name AS n",
        "n",
    );
    assert_eq!(names, ["a"]);

    // Same answer as the MATCH form.
    let names = db.sorted_strings(
        "MATCH (p:P) WHERE EXISTS { MATCH (p)-[:DID]->(a) WHERE a.at >= 20 } RETURN p.name AS n",
        "n",
    );
    assert_eq!(names, ["a"]);
}

#[test]
fn a_pattern_in_where_is_a_predicate() {
    let db = seeded();
    let pairs = db.sorted_strings(
        "MATCH (p:P), (q:P) WHERE p <> q AND NOT (p)-[:BLOCKS]-(q) \
         RETURN p.name + q.name AS pq",
        "pq",
    );
    assert_eq!(pairs, ["ac", "bc", "ca", "cb"]);

    let blocked = db.sorted_strings(
        "MATCH (p:P), (q:P) WHERE (p)-[:BLOCKS]->(q) RETURN p.name + q.name AS pq",
        "pq",
    );
    assert_eq!(blocked, ["ab"]);

    let incoming = db.sorted_strings(
        "MATCH (p:P) WHERE (p)<-[:BLOCKS]-(:P) RETURN p.name AS n",
        "n",
    );
    assert_eq!(incoming, ["b"]);
}

#[test]
fn a_pattern_predicate_is_a_value_in_projections() {
    let db = seeded();
    let rows = db.run(
        "MATCH (p:P {name: 'a'}), (q:P {name: 'c'}) \
         RETURN (p)-[:BLOCKS]->(:P) AS blocks, (q)-[:BLOCKS]-() AS q_blocks",
    );
    assert_eq!(rows[0]["blocks"], true);
    assert_eq!(rows[0]["q_blocks"], false);
}

#[test]
fn parenthesized_arithmetic_is_not_read_as_a_pattern() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH 5 AS a, 2 AS b RETURN (a) - 1 AS m, (a)-(b) AS d, (a)<-1 AS lt, (a) < -(b) AS lt2",
    );
    assert_eq!(rows[0]["m"], 4);
    assert_eq!(rows[0]["d"], 3);
    assert_eq!(rows[0]["lt"], false);
    assert_eq!(rows[0]["lt2"], false);
}
