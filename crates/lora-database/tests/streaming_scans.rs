//! A read-only scan fetches node ids a page at a time instead of copying
//! every id before its first row. A scan that feeds a write still copies
//! them first, so it never sees what the write adds or removes.

mod test_helpers;

use test_helpers::TestDb;

/// More nodes than one page of ids (1024), with gaps from deletes.
fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run("UNWIND range(1, 5000) AS i CREATE (:Item {i: i})");
    db.run("UNWIND range(1, 300) AS i CREATE (:Other {i: i})");
    db.run("MATCH (n:Item) WHERE n.i % 7 = 0 DELETE n");
    db
}

#[test]
fn paged_scans_return_every_node_once() {
    let db = seeded();
    let expected: Vec<i64> = (1..=5000).filter(|i| i % 7 != 0).collect();

    assert_eq!(
        db.sorted_ints("MATCH (n:Item) RETURN n.i AS i", "i"),
        expected
    );
    db.assert_count("MATCH (n) RETURN n", expected.len() + 300);
    db.assert_count("MATCH (n:Item) RETURN n LIMIT 1500", 1500);
    db.assert_count("MATCH (n) RETURN n SKIP 1024 LIMIT 2000", 2000);
    // A scan that restarts for every input row.
    db.assert_count("UNWIND [1, 2, 3] AS x MATCH (n:Other) RETURN x, n", 3 * 300);
    db.assert_count("MATCH (n:Missing) RETURN n", 0);
}

#[test]
fn a_scan_feeding_a_write_does_not_see_that_write() {
    let db = seeded();
    let items = 5000 - 5000 / 7;

    // Each scanned node creates another node the same scan would match.
    db.run("MATCH (n:Item) CREATE (:Item {i: -n.i})");
    db.assert_count("MATCH (n:Item) RETURN n", 2 * items);
    db.run("MATCH (n) CREATE (:Copy)");
    db.assert_count("MATCH (n:Copy) RETURN n", 2 * items + 300);

    // Deleting while scanning removes ids from the list being scanned.
    db.run("MATCH (n:Item) WHERE n.i < 0 DELETE n");
    db.assert_count("MATCH (n:Item) RETURN n", items);
    db.run("MATCH (n:Copy) DELETE n");
    db.assert_count("MATCH (n) RETURN n", items + 300);
}

/// Nodes without labels are indexed under an internal scope. A pattern
/// that names an empty label must not be answered from it.
#[test]
fn an_empty_label_does_not_match_unlabelled_nodes() {
    let db = TestDb::new();
    db.run("CREATE ({k: 1}), (:A {k: 1}), ({k: 2})");
    // Build the equality index on `k`.
    db.assert_count("MATCH (n:A {k: 1}) RETURN n", 1);
    db.assert_count("MATCH (n:`` {k: 1}) RETURN n", 0);
    db.assert_count("MATCH (n:``) WHERE n.k = 1 RETURN n", 0);
    db.assert_count("MATCH (n {k: 1}) RETURN n", 2);
}
