//! Phase 16 planner fixes (docs/design/graphql-next-phases.md):
//!
//! * condition pushdown: a WHERE conjunct that reads one pattern variable
//!   runs right on that variable's scan (so key tests become index seeks),
//!   and a chain starts from whichever end is cheaper;
//! * E18: `x.key IN list` plans as index seeks;
//! * `count(n)` over a bare label scan is answered from the label count;
//! * E21: labels on every node of a chain are tested, not just the first.
//!
//! E14 (bound end nodes in MERGE) lives in `merge_bound_end.rs`.
//!
//! Each result check compares against `WITH * WHERE ...`, which keeps the
//! condition above the whole pattern and so acts as the reference.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{
    Database, ExecuteOptions, LoraValue, PlanTreeNode, ResultFormat, TransactionMode,
};
use serde_json::Value as JsonValue;
use test_helpers::TestDb;

// ---------- helpers ----------

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn s(v: &str) -> LoraValue {
    LoraValue::String(v.to_string())
}

fn plan(db: &TestDb, query: &str) -> PlanTreeNode {
    db.service.explain(query, None).unwrap().tree.root
}

fn find<'a>(node: &'a PlanTreeNode, op: &str, out: &mut Vec<&'a PlanTreeNode>) {
    if node.operator == op {
        out.push(node);
    }
    for child in &node.children {
        find(child, op, out);
    }
}

fn ops<'a>(node: &'a PlanTreeNode, op: &str) -> Vec<&'a PlanTreeNode> {
    let mut out = Vec::new();
    find(node, op, &mut out);
    out
}

fn leaf(node: &PlanTreeNode) -> &PlanTreeNode {
    match node.children.first() {
        Some(child) => leaf(child),
        None => node,
    }
}

/// Every `Expand` must be fed by an index seek (possibly through filters),
/// never by a label scan: the check the lora-graphql package runs.
fn assert_no_scan_expand(root: &PlanTreeNode) {
    for expand in ops(root, "Expand") {
        let mut below = expand.children.first();
        while let Some(node) = below {
            assert_ne!(
                node.operator, "NodeByLabelScan",
                "Expand fed by a label scan: {root:#?}"
            );
            if node.operator != "Filter" {
                break;
            }
            below = node.children.first();
        }
    }
}

fn sorted_rows(db: &TestDb, query: &str, p: &BTreeMap<String, LoraValue>) -> Vec<String> {
    let mut rows: Vec<String> = db
        .run_with_params(query, p.clone())
        .iter()
        .map(|r| r.to_string())
        .collect();
    rows.sort();
    rows
}

/// `query` must contain `WHERE`; the reference moves that WHERE into a
/// `WITH * WHERE` after the MATCH, which is never pushed down.
fn assert_same_as_unpushed(db: &TestDb, query: &str, p: &BTreeMap<String, LoraValue>) {
    let (head, tail) = query.split_once(" WHERE ").expect("query has a WHERE");
    let (cond, rest) = tail.split_once(" RETURN ").expect("query has a RETURN");
    let reference = format!("{head} WITH * WHERE {cond} RETURN {rest}");
    assert_eq!(
        sorted_rows(db, query, p),
        sorted_rows(db, &reference, p),
        "\nquery:     {query}\nreference: {reference}"
    );
}

/// Festivals f0..f49, users u0..u29; user i follows festivals
/// (i * 3 + j) % 50 for j in 0..5. Unique keys on both labels.
fn festival_graph() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT fk FOR (n:Festival) REQUIRE n.key IS UNIQUE");
    db.run("CREATE CONSTRAINT uk FOR (n:User) REQUIRE n.key IS UNIQUE");
    db.run("UNWIND range(0, 49) AS i CREATE (:Festival {key: 'f' + toString(i), size: i % 7})");
    db.run("UNWIND range(0, 29) AS i CREATE (:User {key: 'u' + toString(i), age: 20 + i % 5})");
    db.run(
        "UNWIND range(0, 29) AS i MATCH (u:User {key: 'u' + toString(i)}) \
         UNWIND range(0, 4) AS j MATCH (f:Festival {key: 'f' + toString((i * 3 + j) % 50)}) \
         CREATE (u)-[:FOLLOWS {since: j}]->(f)",
    );
    // Warm the key indexes so the stats know them.
    db.run("MATCH (f:Festival {key: 'f0'}) RETURN f");
    db.run("MATCH (u:User {key: 'u0'}) RETURN u");
    db
}

// ---------- condition pushdown ----------

#[test]
fn where_on_both_ends_seeks_then_expands() {
    let db = festival_graph();
    let q = "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.key = $x AND b.key = $y RETURN r.since AS since";
    let root = plan(&db, q);
    assert!(
        !ops(&root, "NodeByPropertyScan").is_empty(),
        "expected a key seek: {root:#?}"
    );
    assert!(ops(&root, "NodeByLabelScan").is_empty(), "{root:#?}");
    assert_no_scan_expand(&root);

    let p = params(&[("x", s("f3")), ("y", s("u1"))]);
    assert_eq!(
        db.run_with_params(q, p.clone()),
        vec![serde_json::json!({"since": 0})]
    );
    assert_same_as_unpushed(&db, q, &p);
    let p = params(&[("x", s("f3")), ("y", s("u2"))]);
    assert!(db.run_with_params(q, p.clone()).is_empty());
}

#[test]
fn key_from_an_unwind_row_seeks() {
    let db = festival_graph();
    let q = "UNWIND $rows AS row MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) \
             WHERE a.key = row.from AND b.key = row.to RETURN a.key AS a, b.key AS b";
    let root = plan(&db, q);
    let seek = leaf(&root);
    assert_eq!(seek.operator, "Argument");
    assert!(!ops(&root, "NodeByPropertyScan").is_empty(), "{root:#?}");
    assert!(ops(&root, "NodeByLabelScan").is_empty(), "{root:#?}");
    assert_no_scan_expand(&root);

    let row = |from: &str, to: &str| {
        LoraValue::Map(BTreeMap::from([
            ("from".to_string(), s(from)),
            ("to".to_string(), s(to)),
        ]))
    };
    let p = params(&[(
        "rows",
        LoraValue::List(vec![
            row("f3", "u1"),
            row("f4", "u1"),
            row("f9", "u9"),
            row("f0", "u0"),
        ]),
    )]);
    assert_eq!(
        sorted_rows(&db, q, &p),
        vec![
            r#"{"a":"f0","b":"u0"}"#.to_string(),
            r#"{"a":"f3","b":"u1"}"#.to_string(),
            r#"{"a":"f4","b":"u1"}"#.to_string(),
        ]
    );
    assert_same_as_unpushed(&db, q, &p);
}

#[test]
fn chain_starts_from_the_end_that_can_seek() {
    let db = festival_graph();
    // Only the tail has a key: start there instead of scanning festivals.
    let q = "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE b.key = $y RETURN a.key AS a";
    let root = plan(&db, q);
    let start = leaf(&root);
    assert_eq!(start.operator, "NodeByPropertyScan", "{root:#?}");
    assert_eq!(
        start.details.get("labels").map(String::as_str),
        Some("User")
    );
    assert_no_scan_expand(&root);
    let p = params(&[("y", s("u2"))]);
    assert_eq!(
        sorted_rows(&db, q, &p),
        ["f10", "f6", "f7", "f8", "f9"]
            .iter()
            .map(|k| format!(r#"{{"a":"{k}"}}"#))
            .collect::<Vec<_>>()
    );
    assert_same_as_unpushed(&db, q, &p);
}

#[test]
fn chain_starts_from_an_end_bound_by_an_earlier_clause() {
    let db = festival_graph();
    let q = "MATCH (b:User {key: $y}) MATCH (a:Festival)<-[r:FOLLOWS]-(b) RETURN a.key AS a";
    let root = plan(&db, q);
    let label_scans: Vec<_> = ops(&root, "NodeByLabelScan");
    assert!(label_scans.is_empty(), "festivals are scanned: {root:#?}");
    assert_no_scan_expand(&root);
    let p = params(&[("y", s("u2"))]);
    assert_eq!(db.run_with_params(q, p).len(), 5);
}

#[test]
fn reversed_chains_match_the_written_direction() {
    let db = festival_graph();
    db.run("MATCH (u:User {key: 'u0'}), (v:User {key: 'u1'}) CREATE (u)-[:KNOWS]->(v)");
    let p = params(&[("y", s("u1")), ("x", s("f3"))]);
    for q in [
        "MATCH (a:Festival)<-[:FOLLOWS]-(b:User) WHERE b.key = $y RETURN a.key AS a, b.key AS b",
        "MATCH (a:Festival)-[:FOLLOWS]-(b:User) WHERE b.key = $y RETURN a.key AS a, b.key AS b",
        "MATCH (a:Festival)-[:FOLLOWS]->(b:User) WHERE b.key = $y RETURN a.key AS a",
        "MATCH (w:User)-[:KNOWS]->(u:User)-[:FOLLOWS]->(f:Festival) WHERE f.key = $x RETURN w.key AS w, u.key AS u",
        "MATCH (w:User)<-[:KNOWS]-(u:User)-[:FOLLOWS]->(f:Festival) WHERE f.key = $x RETURN w.key AS w, u.key AS u",
        "MATCH (u:User)-[r:FOLLOWS]->(f:Festival) WHERE f.key = $x AND r.since > 0 RETURN u.key AS u, r.since AS s",
        "MATCH p = (u:User)-[:FOLLOWS]->(f:Festival) WHERE f.key = $x RETURN length(p) AS l, u.key AS u",
    ] {
        assert_same_as_unpushed(&db, q, &p);
    }
    let q = "MATCH (w:User)-[:KNOWS]->(u:User)-[:FOLLOWS]->(f:Festival) WHERE f.key = $x RETURN w.key AS w";
    assert_eq!(
        db.run_with_params(q, p),
        vec![serde_json::json!({"w": "u0"})]
    );
}

#[test]
fn multi_variable_conditions_wait_for_both_variables() {
    let db = festival_graph();
    let p = BTreeMap::new();
    for q in [
        "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.size = b.age - 20 RETURN a.key AS a, b.key AS b",
        "MATCH (a:Festival), (b:User) WHERE a.key = 'f1' AND b.age = a.size + 20 RETURN b.key AS b",
        "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE r.since = a.size AND b.age > 21 RETURN a.key AS a, b.key AS b",
        "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.size = 3 OR b.key = 'u1' RETURN a.key AS a, b.key AS b",
        "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.key = 'f5' AND EXISTS { (b)-[:FOLLOWS]->(:Festival {key: 'f6'}) } RETURN b.key AS b",
        "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.key = 'f5' AND size([(b)-[:FOLLOWS]->(g) | g]) = 5 RETURN b.key AS b",
        "MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE b.key IN ['u1', 'u2'] AND all(x IN [a.size] WHERE x < 4) RETURN a.key AS a",
    ] {
        assert_same_as_unpushed(&db, q, &p);
    }
}

#[test]
fn earlier_bound_variables_can_feed_a_seek() {
    let db = festival_graph();
    let q = "MATCH (b:User {key: 'u1'}) WITH b, 'f' + toString(b.age - 18) AS fk \
             MATCH (a:Festival)<-[r:FOLLOWS]-(c:User) WHERE a.key = fk AND c.age = b.age RETURN c.key AS c";
    let root = plan(&db, q);
    assert!(!ops(&root, "NodeByPropertyScan").is_empty(), "{root:#?}");
    assert_same_as_unpushed(&db, q, &BTreeMap::new());
}

#[test]
fn optional_match_conditions_stay_inside_the_optional_match() {
    let db = festival_graph();
    // Every user comes back; only the followers of f4 get it filled in.
    let q = "MATCH (u:User) OPTIONAL MATCH (u)-[r:FOLLOWS]->(f:Festival) \
             WHERE f.key = 'f4' AND r.since > 0 RETURN u.key AS u, f.key AS f";
    let rows = db.run(q);
    assert_eq!(rows.len(), 30);
    let mut hits: Vec<_> = rows
        .iter()
        .filter(|r| !r["f"].is_null())
        .map(|r| r["u"].as_str().unwrap().to_string())
        .collect();
    hits.sort();
    // f4 is followed by u0 (since 4), u1 (since 1) and u17 (since 3).
    assert_eq!(hits, vec!["u0", "u1", "u17"]);
    let q2 = "MATCH (u:User) OPTIONAL MATCH (u)-[:FOLLOWS]->(f:Festival) WHERE f.key = 'nope' \
              RETURN u.key AS u, f AS f";
    let rows = db.run(q2);
    assert_eq!(rows.len(), 30);
    assert!(rows.iter().all(|r| r["f"].is_null()));
}

#[test]
fn shortest_path_end_conditions_pick_the_shortest_path_to_that_end() {
    let db = TestDb::new();
    db.run("CREATE (a:P {k: 1})-[:L]->(:P {k: 2})-[:L]->(:P {k: 5})");
    // The end-node condition applies while choosing the shortest path,
    // so the path to k = 5 is found even though a shorter one exists to
    // k = 2.
    let rows = db.run(
        "MATCH p = shortestPath((a:P {k: 1})-[:L*]-(b:P)) WHERE b.k = 5 RETURN length(p) AS l",
    );
    assert_eq!(rows, vec![serde_json::json!({"l": 2})]);
}

// ---------- E21: labels on later chain nodes ----------

#[test]
fn labels_of_every_chain_node_are_tested() {
    let db = TestDb::new();
    db.run(
        "CREATE (a:A {k: 1})-[:T]->(:B {k: 2}), (a)-[:T]->(:C {k: 3}), \
         (a)-[:T]->(:B:C {k: 4}), (x:X {k: 5})-[:T]->(a)",
    );
    let ks = |q: &str| -> Vec<i64> {
        let mut v: Vec<i64> = db.run(q).iter().map(|r| r["k"].as_i64().unwrap()).collect();
        v.sort();
        v
    };
    assert_eq!(ks("MATCH (a:A)-[:T]->(b:B) RETURN b.k AS k"), vec![2, 4]);
    assert_eq!(ks("MATCH (a:A)-[:T]->(b:B:C) RETURN b.k AS k"), vec![4]);
    assert_eq!(
        ks("MATCH (a:A)-[:T]->(b:B|C) RETURN b.k AS k"),
        vec![2, 3, 4]
    );
    assert_eq!(
        ks("MATCH (a:A)-[:T]->(b:Nope) RETURN b.k AS k"),
        Vec::<i64>::new()
    );
    assert_eq!(
        ks("MATCH (x:X)-[:T]->(a:B)-[:T]->(b) RETURN b.k AS k"),
        Vec::<i64>::new()
    );
    assert_eq!(
        ks("MATCH (x:X)-[:T]->(:A)-[:T]->(b:C) RETURN b.k AS k"),
        vec![3, 4]
    );
    assert_eq!(
        ks("MATCH (x:X)-[:T*1..2]->(b:B) RETURN b.k AS k"),
        vec![2, 4]
    );
    assert_eq!(
        ks("MATCH (b:B)-[:T]-(a:X) RETURN b.k AS k"),
        Vec::<i64>::new()
    );
    assert_eq!(
        ks("MATCH (b)<-[:T]-(a:A) WHERE b:B RETURN b.k AS k"),
        vec![2, 4]
    );
    assert_eq!(
        ks("MATCH (a:A) RETURN size([(a)-[:T]->(b:B) | b]) AS k"),
        vec![2]
    );
    assert_eq!(
        ks("MATCH (a:A) WHERE EXISTS { (a)-[:T]->(:C) } RETURN a.k AS k"),
        vec![1]
    );
}

// ---------- E18: IN seeks ----------

fn in_graph() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE INDEX nk FOR (n:N) ON (n.k)");
    db.run("UNWIND range(0, 199) AS i CREATE (:N {k: i, s: 'n' + toString(i)})");
    db.run("CREATE (:N {k: 2.5}), (:N {k: 'two'}), (:N {k: null}), (:M {k: 1})");
    db.run("MATCH (n:N {k: 0}) RETURN n");
    db
}

#[test]
fn in_list_plans_an_index_seek() {
    let db = in_graph();
    for q in [
        "MATCH (n:N) WHERE n.k IN [1, 2, 3] RETURN n.k AS k",
        "MATCH (n:N) WHERE n.k IN $ks RETURN n.k AS k",
    ] {
        let root = plan(&db, q);
        let seeks = ops(&root, "NodeByPropertyScan");
        assert_eq!(seeks.len(), 1, "{root:#?}");
        assert_eq!(seeks[0].details.get("mode").map(String::as_str), Some("in"));
        assert!(ops(&root, "NodeByLabelScan").is_empty(), "{root:#?}");
    }
    let q = "UNWIND $rows AS row MATCH (n:N) WHERE n.k IN row.ks RETURN n.k AS k";
    let root = plan(&db, q);
    assert_eq!(ops(&root, "NodeByPropertyScan").len(), 1, "{root:#?}");
}

#[test]
fn in_list_seek_matches_the_filter_semantics() {
    let db = in_graph();
    let list = |items: Vec<LoraValue>| params(&[("ks", LoraValue::List(items))]);
    let q = "MATCH (n:N) WHERE n.k IN $ks RETURN n.k AS k";
    let cases = [
        list(vec![
            LoraValue::Int(1),
            LoraValue::Int(2),
            LoraValue::Int(3),
        ]),
        // duplicates are returned once
        list(vec![
            LoraValue::Int(7),
            LoraValue::Int(7),
            LoraValue::Float(7.0),
        ]),
        // null elements never match; other elements still do
        list(vec![LoraValue::Null, LoraValue::Int(4)]),
        // 5.0 finds the integer 5, 2.5 the float, 'two' the string
        list(vec![LoraValue::Float(5.0), LoraValue::Float(2.5), s("two")]),
        list(vec![]),
        list(vec![s("n3"), LoraValue::Int(999)]),
        params(&[("ks", LoraValue::Null)]),
    ];
    for p in &cases {
        assert_same_as_unpushed(&db, q, p);
    }
    assert_eq!(db.run_with_params(q, cases[1].clone()).len(), 1);
    assert_eq!(db.run_with_params(q, cases[3].clone()).len(), 3);
    assert!(db.run_with_params(q, cases[6].clone()).is_empty());

    // Literal lists, lists from an UNWIND row, and a seek on a bound node.
    assert_eq!(
        db.run("MATCH (n:N) WHERE n.k IN [10, 11, 10] RETURN n")
            .len(),
        2
    );
    let rows = params(&[(
        "rows",
        LoraValue::List(vec![
            LoraValue::Map(BTreeMap::from([(
                "ks".to_string(),
                LoraValue::List(vec![LoraValue::Int(1), LoraValue::Int(2)]),
            )])),
            LoraValue::Map(BTreeMap::from([(
                "ks".to_string(),
                LoraValue::List(vec![LoraValue::Int(3)]),
            )])),
        ]),
    )]);
    let q = "UNWIND $rows AS row MATCH (n:N) WHERE n.k IN row.ks RETURN n.k AS k";
    assert_eq!(
        sorted_rows(&db, q, &rows),
        vec![r#"{"k":1}"#, r#"{"k":2}"#, r#"{"k":3}"#]
    );
    assert_same_as_unpushed(&db, q, &rows);
    assert_eq!(
        db.run("MATCH (n:N {k: 3}) WITH n MATCH (n:N) WHERE n.k IN [3, 4] RETURN n.k AS k")
            .len(),
        1
    );
    // Labels still apply: the :M node with k = 1 is not an :N.
    assert_eq!(db.run("MATCH (n:N) WHERE n.k IN [1] RETURN n").len(), 1);
}

#[test]
fn in_over_a_non_list_behaves_like_the_filter() {
    let db = in_graph();
    let q = "MATCH (n:N) WHERE n.k IN $ks RETURN n.k AS k";
    let seek = db.exec_with_params(q, params(&[("ks", LoraValue::Int(3))]));
    let reference = db.exec_with_params(
        "MATCH (n:N) WITH * WHERE n.k IN $ks RETURN n.k AS k",
        params(&[("ks", LoraValue::Int(3))]),
    );
    assert_eq!(seek.is_ok(), reference.is_ok(), "{seek:?} vs {reference:?}");
}

// ---------- count(n) from label counts ----------

#[test]
fn count_of_a_label_is_exact() {
    let db = TestDb::new();
    db.run("UNWIND range(1, 500) AS i CREATE (:L {i: i})");
    db.run("UNWIND range(1, 20) AS i CREATE (:L:K {i: i}), (:K {i: i})");
    db.run("MATCH (n:L) WHERE n.i <= 10 DETACH DELETE n");
    let count = |q: &str| db.scalar(q).as_i64().unwrap();
    assert_eq!(count("MATCH (n:L) RETURN count(n) AS c"), 500);
    assert_eq!(count("MATCH (n:L) RETURN count(*) AS c"), 500);
    assert_eq!(count("MATCH (n:K) RETURN count(n) AS c"), 30);
    assert_eq!(count("MATCH (n:L:K) RETURN count(n) AS c"), 10);
    assert_eq!(count("MATCH (n:L|K) RETURN count(n) AS c"), 520);
    assert_eq!(count("MATCH (n) RETURN count(n) AS c"), 520);
    assert_eq!(count("MATCH (n:Missing) RETURN count(n) AS c"), 0);
    assert_eq!(count("MATCH (n:L) RETURN count(n.i) AS c"), 500);
    assert_eq!(
        count("MATCH (n:L) WHERE n.i > 400 RETURN count(n) AS c"),
        100
    );
    // Used as a value, still one row.
    assert_eq!(
        db.run("MATCH (n:L) RETURN count(n) AS a, count(*) AS b"),
        vec![serde_json::json!({"a": 500, "b": 500})]
    );
    // Correlated bodies must not use the global count.
    assert_eq!(
        db.run(
            "MATCH (m:K) WHERE m.i = 15 CALL { WITH m MATCH (m:L) RETURN count(m) AS c } RETURN c"
        )
        .iter()
        .map(|r| r["c"].as_i64().unwrap())
        .sum::<i64>(),
        1
    );
}

#[test]
fn count_of_a_label_sees_transaction_writes() {
    let db = Database::in_memory();
    let opts = || {
        Some(ExecuteOptions {
            format: ResultFormat::Rows,
        })
    };
    db.execute("UNWIND range(1, 5) AS i CREATE (:L)", opts())
        .unwrap();
    let mut tx = db.begin_transaction(TransactionMode::ReadWrite).unwrap();
    tx.execute("UNWIND range(1, 3) AS i CREATE (:L)", opts())
        .unwrap();
    let json = serde_json::to_value(
        tx.execute("MATCH (n:L) RETURN count(n) AS c", opts())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(json["rows"][0]["c"], JsonValue::from(8));
    tx.rollback().unwrap();
    let json = serde_json::to_value(
        db.execute("MATCH (n:L) RETURN count(n) AS c", opts())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(json["rows"][0]["c"], JsonValue::from(5));
}
