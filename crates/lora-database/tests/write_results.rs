//! A write statement without RETURN produces no result rows (as in other
//! Cypher databases) instead of anonymous `_0` columns exposing internal
//! node ids. The writes themselves still apply.

mod test_helpers;
use test_helpers::TestDb;

#[test]
fn writes_without_return_yield_no_rows() {
    let db = TestDb::new();
    for q in [
        "CREATE (:X {key: 'a'})",
        "MERGE (c:X {key: 'x'}) SET c.name = 'y'",
        "MATCH (c:X {key: 'x'}) SET c.name = 'z'",
        "MATCH (c:X {key: 'a'}) REMOVE c.key",
        "UNWIND [1, 2] AS i CREATE (:Y {i: i})",
        "FOREACH (i IN [1, 2] | CREATE (:Z {i: i}))",
        "MATCH (y:Y {i: 1}) DELETE y",
    ] {
        assert!(db.run(q).is_empty(), "{q} returned rows");
    }
    db.assert_count("MATCH (x:X) RETURN x", 2);
    db.assert_count("MATCH (y:Y) RETURN y", 1);
    db.assert_count("MATCH (z:Z) RETURN z", 2);
    assert_eq!(db.scalar("MATCH (c:X {key: 'x'}) RETURN c.name AS n"), "z");
}

#[test]
fn writes_with_return_still_return() {
    let db = TestDb::new();
    let rows = db.run("CREATE (n:X {key: 'a'}) RETURN n.key AS k");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["k"], "a");
}
