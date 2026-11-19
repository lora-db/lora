//! Ordered seeks on a composite RANGE index (taste N-06). With an index on
//! `(e.person, e.at)`, `WHERE e.person = $p AND e.at >= $from ORDER BY
//! e.at DESC LIMIT 20` reads the 20 rows from the index instead of every
//! event of the person (or of the label). Results always match the plan
//! without the index.

mod test_helpers;
use lora_database::{Database, InMemoryGraph, LoraValue, PlanTreeNode};
use std::collections::BTreeMap;
use test_helpers::TestDb;

fn walk<'a>(node: &'a PlanTreeNode, out: &mut Vec<&'a PlanTreeNode>) {
    out.push(node);
    for child in &node.children {
        walk(child, out);
    }
}

fn plan(db: &Database<InMemoryGraph>, query: &str) -> String {
    let plan = db.explain(query, None).unwrap();
    let mut nodes = Vec::new();
    walk(&plan.tree.root, &mut nodes);
    nodes
        .iter()
        .map(|n| format!("{} {:?}", n.operator, n.details))
        .collect::<Vec<_>>()
        .join("\n")
}

const EVENTS: &str = "UNWIND range(1, 3000) AS i \
    CREATE (:Ev {k: i, person: 'p' + toString(i % 7), \
                 at: CASE WHEN i % 50 = 0 THEN null WHEN i % 31 = 0 THEN toFloat(i % 400) ELSE i % 400 END})";

const QUERIES: &[&str] = &[
    "MATCH (e:Ev) WHERE e.person = 'p3' AND e.at >= 100 RETURN e.k AS k ORDER BY e.at DESC, k LIMIT 15",
    "MATCH (e:Ev) WHERE e.person = 'p3' AND e.at >= 100 AND e.at < 200 RETURN e.k AS k ORDER BY e.at, k LIMIT 15",
    "MATCH (e:Ev) WHERE e.at > 380 AND e.person = 'p5' RETURN e.k AS k ORDER BY e.at ASC, k",
    "MATCH (e:Ev) WHERE e.person = 'p1' AND e.at <= 3.5 RETURN e.k AS k ORDER BY e.at DESC, k LIMIT 4",
    "MATCH (e:Ev) WHERE e.person = 'p2' AND e.at IS NOT NULL RETURN e.k AS k ORDER BY e.at DESC, k LIMIT 9",
    "MATCH (e:Ev) WHERE e.person = 'nobody' AND e.at > 1 RETURN e.k AS k ORDER BY e.at LIMIT 5",
    "MATCH (e:Ev) WHERE e.person = $p AND e.at >= $from RETURN e.k AS k ORDER BY e.at DESC, k LIMIT 10",
];

fn rows(db: &Database<InMemoryGraph>, q: &str) -> Vec<String> {
    let params = BTreeMap::from([
        ("p".to_string(), LoraValue::String("p4".into())),
        ("from".to_string(), LoraValue::Int(390)),
    ]);
    let o = Some(lora_database::ExecuteOptions {
        format: lora_database::ResultFormat::Rows,
    });
    let result = db.execute_with_params(q, o, params).unwrap();
    format!("{result:?}").lines().map(str::to_owned).collect()
}

#[test]
fn results_match_the_plan_without_the_index() {
    let plain = TestDb::new();
    plain.run(EVENTS);
    let indexed = TestDb::new();
    indexed.run("CREATE INDEX ev_person_at FOR (e:Ev) ON (e.person, e.at)");
    indexed.run(EVENTS);
    for q in QUERIES {
        assert_eq!(rows(&plain.service, q), rows(&indexed.service, q), "{q}");
    }
    // The index is used, built before or after the data.
    let late = TestDb::new();
    late.run(EVENTS);
    late.run("CREATE INDEX ev_person_at FOR (e:Ev) ON (e.person, e.at)");
    for q in QUERIES {
        assert_eq!(rows(&plain.service, q), rows(&late.service, q), "{q}");
        let p = plan(&late.service, q);
        assert!(p.contains("prefix"), "{q}\n{p}");
    }
}

#[test]
fn writes_keep_the_index_current() {
    let plain = TestDb::new();
    let indexed = TestDb::new();
    indexed.run("CREATE INDEX ev_person_at FOR (e:Ev) ON (e.person, e.at)");
    for db in [&plain, &indexed] {
        db.run(EVENTS);
        db.run("MATCH (e:Ev) WHERE e.k % 11 = 0 SET e.at = e.at + 1000");
        db.run("MATCH (e:Ev) WHERE e.k % 13 = 0 SET e.person = 'p3'");
        db.run("MATCH (e:Ev) WHERE e.k % 17 = 0 REMOVE e.at");
        db.run("MATCH (e:Ev) WHERE e.k % 19 = 0 DELETE e");
        db.run("MATCH (e:Ev) WHERE e.k % 23 = 0 REMOVE e:Ev SET e:Gone");
        db.run("MATCH (e:Gone) WHERE e.k % 46 = 0 SET e:Ev");
        db.run("MATCH (e:Ev) WHERE e.k % 29 = 0 SET e += {person: 'p3', at: 1500}");
    }
    let qs = [
        "MATCH (e:Ev) WHERE e.person = 'p3' AND e.at >= 0 RETURN e.k AS k ORDER BY e.at DESC, k LIMIT 40",
        "MATCH (e:Ev) WHERE e.person = 'p3' AND e.at > 999 RETURN e.k AS k ORDER BY e.at, k",
    ];
    for q in qs {
        assert_eq!(rows(&plain.service, q), rows(&indexed.service, q), "{q}");
    }
}

#[test]
fn a_mixed_kind_run_is_sorted_not_streamed() {
    // Strings and numbers under one person: index order is not Cypher's,
    // so the unbounded seek falls back to sorting.
    let plain = TestDb::new();
    let indexed = TestDb::new();
    indexed.run("CREATE INDEX ev_person_at FOR (e:Ev) ON (e.person, e.at)");
    for db in [&plain, &indexed] {
        db.run("UNWIND range(1, 40) AS i CREATE (:Ev {k: i, person: 'p', at: CASE WHEN i % 3 = 0 THEN 'x' + toString(i) ELSE i END})");
    }
    for q in [
        "MATCH (e:Ev) WHERE e.person = 'p' AND e.at IS NOT NULL RETURN e.k AS k ORDER BY e.at DESC, k LIMIT 10",
        "MATCH (e:Ev) WHERE e.person = 'p' AND e.at IS NOT NULL RETURN e.k AS k ORDER BY e.at, k LIMIT 10",
    ] {
        assert_eq!(rows(&plain.service, q), rows(&indexed.service, q), "{q}");
    }
}
