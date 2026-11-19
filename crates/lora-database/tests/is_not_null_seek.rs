//! `WHERE n.p IS NOT NULL` is served by a RANGE index on `p` (taste N-08):
//! the index lists every node holding a value, so a property set on a few
//! nodes of a large label is found without reading the label.
//!
//! The answers are the label scan's, values the index cannot order
//! (points, durations, vectors, NaN, lists of those) included, through
//! every write that adds, changes or removes them.

mod test_helpers;
use std::time::Instant;

use lora_database::PlanTreeNode;
use test_helpers::TestDb;

fn find<'a>(node: &'a PlanTreeNode, operator: &str) -> Option<&'a PlanTreeNode> {
    if node.operator == operator {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, operator))
}

fn operators(db: &TestDb, query: &str) -> PlanTreeNode {
    db.service.explain(query, None).unwrap().tree.root
}

const MIXED: &str = "CREATE (:T {k: 1, p: 1}), (:T {k: 2, p: 'a'}), (:T {k: 3, p: [1, 2]}), \
     (:T {k: 4, p: point({x: 1, y: 2})}), (:T {k: 5, p: duration('P1D')}), \
     (:T {k: 6, p: $nan}), (:T {k: 7, p: [duration('P1D')]}), \
     (:T {k: 8, p: vector([1, 2], 2, FLOAT32)}), (:T {k: 9, p: false}), (:T {k: 10})";

/// [`MIXED`], with a NaN that only a parameter can carry.
fn create_mixed(db: &TestDb) {
    let mut params = std::collections::BTreeMap::new();
    params.insert("nan".to_string(), lora_database::LoraValue::Float(f64::NAN));
    db.run_with_params(MIXED, params);
}

fn both() -> (TestDb, TestDb) {
    let indexed = TestDb::new();
    indexed.run("CREATE INDEX t_p FOR (t:T) ON (t.p)");
    let plain = TestDb::new();
    for db in [&indexed, &plain] {
        create_mixed(db);
    }
    (indexed, plain)
}

fn same(indexed: &TestDb, plain: &TestDb, query: &str) {
    assert_eq!(
        indexed.sorted_ints(query, "k"),
        plain.sorted_ints(query, "k"),
        "{query}"
    );
}

#[test]
fn not_null_is_a_range_seek_with_the_scans_answer() {
    let (indexed, plain) = both();
    let q = "MATCH (t:T) WHERE t.p IS NOT NULL RETURN t.k AS k";
    let plan = operators(&indexed, q);
    assert!(
        find(&plan, "NodeByPropertyRangeScan").is_some(),
        "{plan:#?}"
    );
    assert!(find(&plan, "NodeByLabelScan").is_none(), "{plan:#?}");
    assert_eq!(indexed.sorted_ints(q, "k"), (1..=9).collect::<Vec<_>>());
    same(&indexed, &plain, q);
    same(
        &indexed,
        &plain,
        "MATCH (t:T) WHERE t.p IS NOT NULL AND t.k > 4 RETURN t.k AS k",
    );
    same(
        &indexed,
        &plain,
        "MATCH (t:T) WHERE t.p IS NOT NULL RETURN t.k AS k ORDER BY t.k LIMIT 3",
    );
    same(
        &indexed,
        &plain,
        "MATCH (t:T) WHERE t.p IS NOT NULL RETURN t.k AS k ORDER BY t.p LIMIT 20",
    );
}

#[test]
fn writes_keep_the_index_answer() {
    let (indexed, plain) = both();
    for write in [
        "MATCH (t:T {k: 4}) SET t.p = 5",
        "MATCH (t:T {k: 1}) SET t.p = duration('PT1H')",
        "MATCH (t:T {k: 5}) REMOVE t.p",
        "MATCH (t:T {k: 8}) DETACH DELETE t",
        "MATCH (t:T {k: 10}) SET t.p = point({x: 0, y: 0})",
        "CREATE (:T {k: 11, p: [point({x: 1, y: 1})]})",
        "MATCH (t:T {k: 9}) REMOVE t:T",
        "MATCH (t {k: 9}) SET t:T",
    ] {
        indexed.run(write);
        plain.run(write);
        same(
            &indexed,
            &plain,
            "MATCH (t:T) WHERE t.p IS NOT NULL RETURN t.k AS k",
        );
    }
}

#[test]
fn an_index_built_over_existing_data_holds_unkeyed_values() {
    let db = TestDb::new();
    create_mixed(&db);
    db.run("CREATE INDEX t_p FOR (t:T) ON (t.p)");
    assert_eq!(
        db.sorted_ints("MATCH (t:T) WHERE t.p IS NOT NULL RETURN t.k AS k", "k"),
        (1..=9).collect::<Vec<_>>()
    );
}

#[test]
fn a_null_bound_still_matches_nothing() {
    let (indexed, plain) = both();
    let mut params = std::collections::BTreeMap::new();
    params.insert("x".to_string(), lora_database::LoraValue::Null);
    for q in [
        "MATCH (t:T) WHERE t.p < $x RETURN t.k AS k",
        "MATCH (t:T) WHERE t.p >= $x RETURN t.k AS k",
    ] {
        let a = indexed.run_with_params(q, params.clone());
        let b = plain.run_with_params(q, params.clone());
        assert_eq!(a, b, "{q}");
        assert!(a.is_empty(), "{q}: {a:?}");
    }
}

#[test]
fn a_sparse_property_does_not_read_the_label() {
    let time = |db: &TestDb, q: &str| {
        db.run(q);
        (0..5)
            .map(|_| {
                let t = Instant::now();
                db.run(q);
                t.elapsed().as_secs_f64()
            })
            .fold(f64::MAX, f64::min)
    };
    let db = TestDb::new();
    db.run("CREATE INDEX r FOR (e:E) ON (e.rank)");
    db.run("UNWIND range(1, 50) AS i CREATE (:E {rank: toFloat(i) / 50})");
    let q = "MATCH (e:E) WHERE e.rank IS NOT NULL RETURN count(*) AS n";
    let alone = time(&db, q);
    db.run("UNWIND range(1, 50000) AS i CREATE (:E {k: i})");
    let crowded = time(&db, q);
    assert_eq!(db.run(q)[0]["n"], 50);
    assert!(
        crowded < alone * 5.0 + 0.005,
        "50 ranked alone {alone:.5}s, among 50k unranked {crowded:.5}s"
    );
}

#[test]
fn an_index_built_in_parallel_holds_unkeyed_values() {
    // Above the store's parallel build threshold (2^17 members).
    let db = TestDb::new();
    db.run("UNWIND range(1, 140000) AS i CREATE (:B {k: i})");
    db.run(
        "MATCH (b:B) WHERE b.k % 10000 = 0 \
         SET b.p = CASE b.k % 20000 WHEN 0 THEN duration('P1D') ELSE b.k END",
    );
    db.run("CREATE INDEX b_p FOR (b:B) ON (b.p)");
    let ks = db.sorted_ints("MATCH (b:B) WHERE b.p IS NOT NULL RETURN b.k AS k", "k");
    assert_eq!(ks, (1..=14).map(|i| i * 10000).collect::<Vec<_>>());
}
