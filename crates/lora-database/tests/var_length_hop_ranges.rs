//! Variable-length hop ranges mean what Cypher says: `*` is one or more
//! hops, `*2` exactly two, `*2..` two or more, `*..3` one to three and
//! `*2..3` two or three.
//!
//! The parser read `*2` as "two or more" (a start with no end).

mod test_helpers;
use test_helpers::TestDb;

/// A chain a -> b -> c -> d -> e.
fn chain() -> TestDb {
    let db = TestDb::new();
    db.run(
        "CREATE (:N {k: 'a'})-[:NEXT]->(:N {k: 'b'})-[:NEXT]->(:N {k: 'c'})\
         -[:NEXT]->(:N {k: 'd'})-[:NEXT]->(:N {k: 'e'})",
    );
    db
}

#[test]
fn every_range_form() {
    let db = chain();
    for (range, want) in [
        ("*", vec!["b", "c", "d", "e"]),
        ("*2", vec!["c"]),
        ("*1", vec!["b"]),
        ("*0", vec!["a"]),
        ("*2..", vec!["c", "d", "e"]),
        ("*..3", vec!["b", "c", "d"]),
        ("*2..3", vec!["c", "d"]),
        ("*0..1", vec!["a", "b"]),
        ("* 2 .. 3", vec!["c", "d"]),
    ] {
        let q = format!("MATCH (:N {{k: 'a'}})-[:NEXT{range}]->(x) RETURN x.k AS k");
        assert_eq!(db.sorted_strings(&q, "k"), want, "{range}");
    }
    // With the relationship list bound, and on the streamed (write) path.
    assert_eq!(
        db.sorted_strings(
            "MATCH (:N {k: 'a'})-[r:NEXT*2]->(x) RETURN x.k + ':' + toString(size(r)) AS k",
            "k"
        ),
        vec!["c:2"]
    );
    db.run("MATCH (:N {k: 'a'})-[:NEXT*3]->(x) CREATE (:Hit {k: x.k})");
    assert_eq!(
        db.sorted_strings("MATCH (h:Hit) RETURN h.k AS k", "k"),
        vec!["d"]
    );
}
