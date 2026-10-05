//! Seek by internal id: `WHERE id(n) = value` / `id(n) IN list` (and the
//! same on a relationship variable) plan as `NodeByIdSeek` /
//! `RelByIdSeek` instead of a scan with a filter.
//!
//! Every result check compares against the same query with each `id(x)`
//! written as `(id(x) + 0)`: the same value under the same comparison,
//! but not a shape the planner seeks, so it runs the scan and filter that
//! defined the semantics before the seek existed.

mod test_helpers;

use std::collections::BTreeMap;

use lora_database::{LoraValue, PlanTreeNode, TransactionMode};
use test_helpers::TestDb;

// ---------- helpers ----------

fn params(v: &LoraValue) -> BTreeMap<String, LoraValue> {
    BTreeMap::from([("v".to_string(), v.clone())])
}

fn plan(db: &TestDb, query: &str) -> PlanTreeNode {
    db.service.explain(query, None).unwrap().tree.root
}

fn ops<'a>(node: &'a PlanTreeNode, op: &str, out: &mut Vec<&'a PlanTreeNode>) {
    if node.operator == op {
        out.push(node);
    }
    for child in &node.children {
        ops(child, op, out);
    }
}

fn find<'a>(root: &'a PlanTreeNode, op: &str) -> Vec<&'a PlanTreeNode> {
    let mut out = Vec::new();
    ops(root, op, &mut out);
    out
}

fn has_scan(root: &PlanTreeNode) -> bool {
    !find(root, "NodeScan").is_empty() || !find(root, "NodeByLabelScan").is_empty()
}

/// `id(x)` → `(id(x) + 0)` for every variable `x`, likewise `node.id(x)`,
/// `edge.id(x)` and `value.id(x)`.
fn reference(query: &str) -> String {
    let mut out = String::new();
    let mut rest = query;
    while let Some(pos) = rest.find("id(") {
        let head = &rest[..pos];
        let start = ["node.", "edge.", "value."]
            .iter()
            .find(|prefix| head.ends_with(*prefix))
            .map_or(pos, |prefix| pos - prefix.len());
        let prev = rest[..start].chars().last();
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
            out.push_str(&rest[..pos + 3]);
            rest = &rest[pos + 3..];
            continue;
        }
        let close = rest[pos..].find(')').expect("closing paren") + pos;
        out.push_str(&rest[..start]);
        out.push('(');
        out.push_str(&rest[start..=close]);
        out.push_str(" + 0)");
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

fn sorted_rows(db: &TestDb, query: &str, v: &LoraValue) -> Vec<String> {
    let mut rows: Vec<String> = db
        .run_with_params(query, params(v))
        .iter()
        .map(|r| r.to_string())
        .collect();
    rows.sort();
    rows
}

/// `query` returns the rows its reference returns, for every value.
fn assert_same_as_scan(db: &TestDb, query: &str, values: &[LoraValue]) {
    let reference = reference(query);
    assert!(
        has_scan(&plan(db, &reference)),
        "reference must scan: {reference}"
    );
    for v in values {
        assert_eq!(
            sorted_rows(db, query, v),
            sorted_rows(db, &reference, v),
            "\nquery:     {query}\nreference: {reference}\n$v = {v:?}"
        );
    }
}

fn int(i: i64) -> LoraValue {
    LoraValue::Int(i)
}

fn list(items: Vec<LoraValue>) -> LoraValue {
    LoraValue::List(items)
}

struct Graph {
    db: TestDb,
    /// Node ids by `k`; `deleted_node` / `deleted_rel` no longer exist.
    node: BTreeMap<i64, i64>,
    deleted_node: i64,
    rel: BTreeMap<String, i64>,
    deleted_rel: i64,
}

/// Nodes k0 (:A), k1 (:B), k2 (:A:B), k3 (:C, deleted), k4 (:A), k5 (no
/// label). Relationships r01 k0-[:R]->k1, r12 k1-[:S]->k2, r22 k2-[:R]->k2
/// (a self-loop), r41 k4-[:R]->k1, r40 k4-[:S]->k0, rdel (deleted).
fn graph() -> Graph {
    let db = TestDb::new();
    db.run("CREATE (:A {k: 0}), (:B {k: 1}), (:A:B {k: 2}), (:C {k: 3}), (:A {k: 4}), ({k: 5})");
    db.run(
        "MATCH (a {k: 0}), (b {k: 1}), (c {k: 2}), (d {k: 4}), (e {k: 5}) \
         CREATE (a)-[:R {name: 'r01'}]->(b), (b)-[:S {name: 'r12'}]->(c), \
                (c)-[:R {name: 'r22'}]->(c), (d)-[:R {name: 'r41'}]->(b), \
                (d)-[:S {name: 'r40'}]->(a), (e)-[:R {name: 'rdel'}]->(a)",
    );
    let mut node = BTreeMap::new();
    for row in db.run("MATCH (n) RETURN n.k AS k, id(n) AS id") {
        node.insert(row["k"].as_i64().unwrap(), row["id"].as_i64().unwrap());
    }
    let mut rel = BTreeMap::new();
    for row in db.run("MATCH ()-[r]->() RETURN r.name AS name, id(r) AS id") {
        rel.insert(
            row["name"].as_str().unwrap().to_string(),
            row["id"].as_i64().unwrap(),
        );
    }
    let deleted_node = node.remove(&3).unwrap();
    let deleted_rel = rel.remove("rdel").unwrap();
    db.run("MATCH ()-[r {name: 'rdel'}]->() DELETE r");
    db.run("MATCH (n {k: 3}) DELETE n");
    Graph {
        db,
        node,
        deleted_node,
        rel,
        deleted_rel,
    }
}

/// Every scalar the id comparison has to get right.
fn scalar_values(g: &Graph) -> Vec<LoraValue> {
    let mut values: Vec<LoraValue> = g.node.values().map(|id| int(*id)).collect();
    values.extend(g.rel.values().map(|id| int(*id)));
    values.extend([
        int(g.deleted_node),
        int(g.deleted_rel),
        int(-1),
        int(1_000),
        int(i64::MAX),
        int(i64::MIN),
        LoraValue::Float(g.node[&1] as f64),
        LoraValue::Float(g.node[&2] as f64 + 0.5),
        LoraValue::Float(-0.0),
        LoraValue::Float(f64::NAN),
        LoraValue::Float(f64::INFINITY),
        LoraValue::Float(1e300),
        LoraValue::Float(9_007_199_254_740_993.0),
        LoraValue::Null,
        LoraValue::String(g.node[&1].to_string()),
        LoraValue::Bool(true),
        list(vec![int(g.node[&1])]),
    ]);
    values
}

/// Every list (and non-list) an `IN` has to get right.
fn list_values(g: &Graph) -> Vec<LoraValue> {
    let n1 = int(g.node[&1]);
    let n2 = int(g.node[&2]);
    let r = int(g.rel["r12"]);
    vec![
        list(vec![n1.clone(), n2.clone()]),
        list(vec![n1.clone(), n1.clone(), n1.clone()]),
        list(vec![n2.clone(), n1.clone(), n2.clone()]),
        list(vec![n1.clone(), LoraValue::Null]),
        list(vec![LoraValue::Null]),
        list(vec![]),
        list(vec![
            int(-1),
            int(1_000),
            int(g.deleted_node),
            int(g.deleted_rel),
        ]),
        list(vec![LoraValue::Float(g.node[&2] as f64), n1.clone()]),
        list(vec![LoraValue::String("1".into()), list(vec![n1.clone()])]),
        list(vec![r.clone(), int(g.rel["r22"]), r]),
        list((0..20).map(int).collect()),
        LoraValue::Null,
        n1,
        LoraValue::String("x".into()),
    ]
}

// ---------- plans ----------

#[test]
fn id_equality_plans_a_node_seek() {
    let g = graph();
    for q in [
        "MATCH (n) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n) WHERE $v = id(n) RETURN n.k AS k",
        "MATCH (n) WHERE node.id(n) = $v RETURN n.k AS k",
        "MATCH (n) WHERE value.id(n) = $v RETURN n.k AS k",
        "MATCH (n:A) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n) WHERE n.k > 0 AND id(n) = $v AND n.k < 9 RETURN n.k AS k",
        "MATCH (n) WHERE id(n) = 1 RETURN n.k AS k",
        "MATCH (n) WHERE id(n) IN $v RETURN n.k AS k",
        "MATCH (n) WHERE id(n) IN [1, 2, 3] RETURN n.k AS k",
        "MATCH (n) WHERE id(n) = $v SET n.hit = true",
        "MATCH (n) WHERE id(n) = $v DETACH DELETE n",
        "OPTIONAL MATCH (n) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (m {k: 0}) MATCH (n) WHERE id(n) = m.k + $v RETURN n.k AS k",
    ] {
        let root = plan(&g.db, q);
        assert_eq!(find(&root, "NodeByIdSeek").len(), 1, "{q}: {root:#?}");
    }
}

#[test]
fn the_seek_shows_in_explain() {
    let g = graph();
    let root = plan(&g.db, "MATCH (n:A) WHERE id(n) = $v RETURN n.k AS k");
    let seek = find(&root, "NodeByIdSeek")[0];
    assert_eq!(
        seek.details.get("ids").map(String::as_str),
        Some(r#"Parameter("v")"#)
    );
    assert_eq!(seek.details.get("labels").map(String::as_str), Some("A"));
    assert!(!seek.details.contains_key("mode"));
    assert_eq!(seek.estimated_rows, Some(1));
    // The id test stays in the Filter above the seek.
    assert_eq!(find(&root, "Filter").len(), 1, "{root:#?}");

    let root = plan(&g.db, "MATCH (n) WHERE id(n) IN $v RETURN n");
    let seek = find(&root, "NodeByIdSeek")[0];
    assert_eq!(seek.details.get("mode").map(String::as_str), Some("in"));

    let root = plan(&g.db, "MATCH (a)-[r:R]-(b) WHERE id(r) = $v RETURN a");
    let seek = find(&root, "RelByIdSeek")[0];
    assert_eq!(seek.details.get("types").map(String::as_str), Some("R"));
    assert_eq!(seek.details.get("direction").map(String::as_str), Some("-"));
    assert_eq!(seek.estimated_rows, Some(2));
    assert!(find(&root, "Expand").is_empty(), "{root:#?}");
}

#[test]
fn shapes_that_cannot_seek_keep_the_scan() {
    let g = graph();
    for q in [
        // The value reads the variable itself.
        "MATCH (n) WHERE id(n) = n.k RETURN n.k AS k",
        // Not a conjunct.
        "MATCH (n) WHERE id(n) = $v OR n.k = 0 RETURN n.k AS k",
        // `edge.id` of a node is null.
        "MATCH (n) WHERE edge.id(n) = $v RETURN n.k AS k",
        // Not the id itself.
        "MATCH (n) WHERE id(n) + 0 = $v RETURN n.k AS k",
        "MATCH (n) WHERE id(n) > $v RETURN n.k AS k",
    ] {
        let root = plan(&g.db, q);
        assert!(find(&root, "NodeByIdSeek").is_empty(), "{q}: {root:#?}");
    }
    // `node.id` of a relationship is null: no relationship seek.
    let root = plan(
        &g.db,
        "MATCH (a)-[r]->(b) WHERE node.id(r) = $v RETURN a.k AS k",
    );
    assert!(find(&root, "RelByIdSeek").is_empty(), "{root:#?}");
}

#[test]
fn a_seek_on_the_last_node_starts_the_chain_there() {
    let g = graph();
    let q = "MATCH (a)-[:R]->(b)-[:S]->(c) WHERE id(c) = $v RETURN a.k AS a, b.k AS b";
    let root = plan(&g.db, q);
    assert_eq!(find(&root, "NodeByIdSeek").len(), 1, "{root:#?}");
    assert!(!has_scan(&root), "{root:#?}");
    let values: Vec<LoraValue> = g.node.values().map(|id| int(*id)).collect();
    assert_same_as_scan(&g.db, q, &values);
}

#[test]
fn a_seek_on_the_last_relationship_starts_the_chain_there() {
    let g = graph();
    let q = "MATCH (a)-[r]->(b)-[s]->(c) WHERE id(s) = $v RETURN a.k AS a, b.k AS b, c.k AS c";
    let root = plan(&g.db, q);
    assert_eq!(find(&root, "RelByIdSeek").len(), 1, "{root:#?}");
    assert!(!has_scan(&root), "{root:#?}");
    assert_same_as_scan(&g.db, q, &scalar_values(&g));
}

// ---------- node seek semantics ----------

#[test]
fn node_id_equality_matches_the_scan() {
    let g = graph();
    let values = scalar_values(&g);
    for q in [
        "MATCH (n) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n) WHERE $v = id(n) RETURN n.k AS k",
        "MATCH (n) WHERE node.id(n) = $v RETURN n.k AS k",
        "MATCH (n:A) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n:A:B) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n:A|C) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n:Missing) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n) WHERE id(n) = $v AND n.k >= 1 RETURN n.k AS k",
        "MATCH (n {k: 1}) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (n) WHERE id(n) = $v RETURN count(*) AS c",
        "MATCH (n) WHERE id(n) = $v RETURN n.k AS k LIMIT 1",
        "MATCH (n) WHERE id(n) = $v AND id(n) = $v RETURN n.k AS k",
        "MATCH (n), (m) WHERE id(n) = $v AND id(m) = $v RETURN n.k AS a, m.k AS b",
        "MATCH (m) MATCH (n) WHERE id(n) = $v RETURN m.k AS a, n.k AS b",
        "MATCH (m) WITH m MATCH (m) WHERE id(m) = $v RETURN m.k AS k",
        "MATCH (m) WITH m MATCH (m:A) WHERE id(m) = $v RETURN m.k AS k",
        "MATCH (n)-[r]->(m) WHERE id(n) = $v RETURN r.name AS r",
        "MATCH (n)-[r]->(m) WHERE id(m) = $v RETURN r.name AS r",
        "OPTIONAL MATCH (n) WHERE id(n) = $v RETURN n.k AS k",
        "MATCH (m:A) OPTIONAL MATCH (m)-->(n) WHERE id(n) = $v RETURN m.k AS a, n.k AS b",
        "UNWIND [1, 2] AS x MATCH (n) WHERE id(n) = $v RETURN x, n.k AS k",
    ] {
        assert_same_as_scan(&g.db, q, &values);
    }
}

#[test]
fn value_from_an_earlier_row_matches_the_scan() {
    let g = graph();
    let values = vec![int(0), int(1), int(-3), LoraValue::Null];
    for q in [
        "MATCH (m) MATCH (n) WHERE id(n) = m.k + $v RETURN m.k AS a, n.k AS b",
        "UNWIND [0, 1, 2, 2, 7, null, -1, 1.0] AS i MATCH (n) WHERE id(n) = i RETURN i, n.k AS k",
        "UNWIND [[0, 1], [1, 1], null, [], [null]] AS l MATCH (n) WHERE id(n) IN l RETURN l, n.k AS k",
    ] {
        assert_same_as_scan(&g.db, q, &values);
    }
}

#[test]
fn node_id_in_list_matches_the_scan() {
    let g = graph();
    let values = list_values(&g);
    for q in [
        "MATCH (n) WHERE id(n) IN $v RETURN n.k AS k",
        "MATCH (n:A) WHERE id(n) IN $v RETURN n.k AS k",
        "MATCH (n) WHERE id(n) IN $v AND n.k <> 2 RETURN n.k AS k",
        "MATCH (n) WHERE id(n) IN $v RETURN count(*) AS c",
        "MATCH (n) WHERE id(n) IN $v AND id(n) = 1 RETURN n.k AS k",
    ] {
        assert_same_as_scan(&g.db, q, &values);
    }
    // Literal lists, duplicates included: each node once.
    let n1 = g.node[&1];
    let n2 = g.node[&2];
    let q = format!("MATCH (n) WHERE id(n) IN [{n1}, {n1}, {n2}, null, {n1}.0] RETURN n.k AS k");
    assert_same_as_scan(&g.db, &q, &[LoraValue::Null]);
    assert_eq!(g.db.run(&q).len(), 2);
}

#[test]
fn deleted_and_missing_ids_match_nothing() {
    let g = graph();
    for v in [g.deleted_node, -1, 1_000, i64::MAX] {
        assert!(
            sorted_rows(&g.db, "MATCH (n) WHERE id(n) = $v RETURN n", &int(v)).is_empty(),
            "{v}"
        );
    }
    // A float equal to an id matches it, as `1 = 1.0` is true.
    let n1 = g.node[&1];
    assert_eq!(
        sorted_rows(
            &g.db,
            "MATCH (n) WHERE id(n) = $v RETURN n.k AS k",
            &LoraValue::Float(n1 as f64)
        ),
        vec![r#"{"k":1}"#.to_string()]
    );
}

#[test]
fn every_execution_path_agrees() {
    let g = graph();
    let n1 = int(g.node[&1]);
    let q = "MATCH (n) WHERE id(n) = $v RETURN n.k AS k";
    let rq = "MATCH (a)-[r]-(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b";
    let rid = int(g.rel["r01"]);
    for (query, v) in [(q, &n1), (rq, &rid)] {
        let normalise = |rows: Vec<lora_database::Row>| {
            let mut v: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect();
            v.sort();
            v
        };
        let expected = normalise(
            g.db.service
                .execute_rows_with_params(query, params(v))
                .unwrap(),
        );
        assert!(!expected.is_empty(), "{query}");
        let streamed = normalise(
            g.db.service
                .stream_with_params(query, params(v))
                .unwrap()
                .collect(),
        );
        assert_eq!(streamed, expected, "stream: {query}");
        for mode in [TransactionMode::ReadWrite, TransactionMode::ReadOnly] {
            let mut tx = g.db.service.begin_transaction(mode).unwrap();
            let rows = normalise(tx.execute_rows_with_params(query, params(v)).unwrap());
            tx.rollback().unwrap();
            assert_eq!(rows, expected, "tx {mode:?}: {query}");
        }
    }
}

// ---------- writes ----------

/// Run `write` on a fresh graph with the seek and with its reference, and
/// compare the graphs afterwards.
fn assert_write_same_as_scan(write: &str, v: impl Fn(&Graph) -> LoraValue) {
    let state = |g: &Graph| {
        let mut rows: Vec<String> = g
            .db
            .run(
                "MATCH (n) OPTIONAL MATCH (n)-[r]->(m) \
                 RETURN properties(n) AS n, labels(n) AS l, type(r) AS t, properties(r) AS r, m.k AS m",
            )
            .iter()
            .map(|r| r.to_string())
            .collect();
        rows.sort();
        rows
    };
    let seek = graph();
    let value = v(&seek);
    let root = plan(&seek.db, write);
    assert!(
        !find(&root, "NodeByIdSeek").is_empty() || !find(&root, "RelByIdSeek").is_empty(),
        "{write}: {root:#?}"
    );
    seek.db.run_with_params(write, params(&value));

    let scan = graph();
    let reference = reference(write);
    scan.db.run_with_params(&reference, params(&value));

    assert_eq!(
        state(&seek),
        state(&scan),
        "\nwrite:     {write}\nreference: {reference}"
    );
    assert_ne!(state(&seek), state(&graph()), "{write} changed nothing");
}

#[test]
fn writes_through_the_seek_match_the_scan() {
    let n1 = |g: &Graph| int(g.node[&1]);
    assert_write_same_as_scan("MATCH (n) WHERE id(n) = $v SET n.hit = true", n1);
    assert_write_same_as_scan("MATCH (n:B) WHERE id(n) = $v SET n.hit = $v", n1);
    assert_write_same_as_scan("MATCH (n) WHERE id(n) = $v SET n += {a: 1, b: 2}", n1);
    assert_write_same_as_scan("MATCH (n) WHERE id(n) = $v REMOVE n:B", n1);
    assert_write_same_as_scan("MATCH (n) WHERE id(n) = $v DETACH DELETE n", n1);
    assert_write_same_as_scan(
        "MATCH (n) WHERE id(n) = $v CREATE (n)-[:NEW]->(:X {k: 9})",
        n1,
    );
    assert_write_same_as_scan("MATCH (n) WHERE id(n) = $v MERGE (n)-[:M]->(:Y {k: 8})", n1);
    assert_write_same_as_scan(
        "MATCH (n) WHERE id(n) IN $v SET n.hit = true",
        |g: &Graph| list(vec![int(g.node[&1]), int(g.node[&1]), int(g.node[&4])]),
    );
    assert_write_same_as_scan(
        "MATCH (a)-[r]->(b) WHERE id(r) = $v SET r.hit = true, a.src = true, b.dst = true",
        |g: &Graph| int(g.rel["r41"]),
    );
    assert_write_same_as_scan("MATCH ()-[r]->() WHERE id(r) = $v DELETE r", |g: &Graph| {
        int(g.rel["r22"])
    });
}

#[test]
fn a_set_by_id_writes_one_node_in_place() {
    let g = graph();
    let n4 = g.node[&4];
    let rows = g.db.run_with_params(
        "MATCH (n) WHERE id(n) = $v SET n.score = 1.5 RETURN n.k AS k",
        params(&int(n4)),
    );
    assert_eq!(rows, vec![serde_json::json!({"k": 4})]);
    assert_eq!(
        g.db.run("MATCH (n) WHERE n.score = 1.5 RETURN n.k AS k"),
        vec![serde_json::json!({"k": 4})]
    );
    // No such node: nothing written, no error.
    g.db.run_with_params(
        "MATCH (n) WHERE id(n) = $v SET n.score = 2.5",
        params(&int(g.deleted_node)),
    );
    assert!(g
        .db
        .run("MATCH (n) WHERE n.score = 2.5 RETURN n")
        .is_empty());
}

// ---------- relationship seek semantics ----------

#[test]
fn relationship_id_seek_matches_the_scan() {
    let g = graph();
    let mut values = scalar_values(&g);
    values.extend(
        list_values(&g)
            .into_iter()
            .filter(|v| matches!(v, LoraValue::List(_))),
    );
    for q in [
        "MATCH (a)-[r]->(b) WHERE id(r) = $v RETURN a.k AS a, r.name AS r, b.k AS b",
        "MATCH (a)<-[r]-(b) WHERE id(r) = $v RETURN a.k AS a, r.name AS r, b.k AS b",
        "MATCH (a)-[r]-(b) WHERE id(r) = $v RETURN a.k AS a, r.name AS r, b.k AS b",
        "MATCH (a)-[r:R]->(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (a)-[r:S|R]-(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (a)-[r:Missing]->(b) WHERE id(r) = $v RETURN a.k AS a",
        "MATCH (a:A)-[r]->(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (a:A)-[r]-(b:B) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (a)-[r]->(a) WHERE id(r) = $v RETURN a.k AS a",
        "MATCH (a)-[r]-(a) WHERE id(r) = $v RETURN a.k AS a",
        "MATCH (a)-[r]->(b) WHERE edge.id(r) = $v RETURN a.k AS a",
        "MATCH (a)-[r]->(b) WHERE $v = id(r) AND b.k > 0 RETURN a.k AS a, b.k AS b",
        "MATCH ()-[r]->() WHERE id(r) = $v RETURN r.name AS r",
        "MATCH ()-[r]->() WHERE id(r) = $v RETURN count(*) AS c",
        "MATCH (a) WITH a MATCH (a)-[r]->(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (b) WITH b MATCH (a)-[r]-(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH ()-[r]->() WITH r MATCH (a)-[r]-(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (x {k: 0}) MATCH (a)-[r]->(b) WHERE id(r) = $v RETURN x.k AS x, a.k AS a",
        "MATCH (a)-[r]->(b)-[s]->(c) WHERE id(r) = $v RETURN a.k AS a, c.k AS c",
        "MATCH p = (a)-[r]->(b) WHERE id(r) = $v RETURN length(p) AS l, a.k AS a",
        "OPTIONAL MATCH (a)-[r]->(b) WHERE id(r) = $v RETURN a.k AS a, b.k AS b",
        "MATCH (x:A) OPTIONAL MATCH (x)-[r]->(b) WHERE id(r) = $v RETURN x.k AS x, b.k AS b",
    ] {
        assert_same_as_scan(&g.db, q, &values);
    }
}

#[test]
fn relationship_id_in_list_matches_the_scan() {
    let g = graph();
    let values = list_values(&g);
    for q in [
        "MATCH (a)-[r]->(b) WHERE id(r) IN $v RETURN a.k AS a, r.name AS r, b.k AS b",
        "MATCH (a)-[r]-(b) WHERE id(r) IN $v RETURN a.k AS a, r.name AS r, b.k AS b",
        "MATCH (a:A)-[r:R]->(b) WHERE id(r) IN $v RETURN a.k AS a, b.k AS b",
    ] {
        let root = plan(&g.db, q);
        assert_eq!(find(&root, "RelByIdSeek").len(), 1, "{q}: {root:#?}");
        assert_same_as_scan(&g.db, q, &values);
    }
}
