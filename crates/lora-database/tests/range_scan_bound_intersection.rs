//! Several bounds on one indexed property intersect into the range scan's
//! bounds, even when they are parameters the planner cannot order: the scan
//! picks the tightest per row. A keyset page within a prefix
//! (`STARTS WITH $p AND key > $after`) then seeks from `$after`, not from
//! the prefix, and reads only its page.

mod test_helpers;
use std::collections::BTreeMap;

use lora_compiler::PlanTreeNode;
use lora_database::LoraValue;
use test_helpers::TestDb;

const PAGE: &str = "MATCH (r:R) WHERE r.key STARTS WITH $p AND r.key > $after \
                    RETURN r.key AS key ORDER BY r.key LIMIT 5";

fn seeded(indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE CONSTRAINT k IF NOT EXISTS FOR (n:R) REQUIRE n.key IS UNIQUE");
    }
    db.run("UNWIND range(1000, 2999) AS i CREATE (:R {key: 'a:' + toString(i)})");
    db.run("UNWIND range(1000, 1099) AS i CREATE (:R {key: 'b:' + toString(i)})");
    db
}

fn params(pairs: &[(&str, LoraValue)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn s(v: &str) -> LoraValue {
    LoraValue::String(v.to_string())
}

fn keys(db: &TestDb, q: &str, p: BTreeMap<String, LoraValue>) -> Vec<String> {
    db.run_with_params(q, p)
        .iter()
        .map(|r| r["key"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(node: &'a PlanTreeNode, operator: &str) -> Option<&'a PlanTreeNode> {
    if node.operator == operator {
        return Some(node);
    }
    node.children.iter().find_map(|c| find(c, operator))
}

/// Rows the range scan produced when profiling `q`.
fn scan_rows(db: &TestDb, q: &str, p: BTreeMap<String, LoraValue>) -> u64 {
    let prof = db.service.profile(q, Some(p)).unwrap();
    let scan = find(&prof.plan.tree.root, "NodeByPropertyRangeScan")
        .unwrap_or_else(|| panic!("no range scan in {:#?}", prof.plan.tree));
    prof.metrics.per_operator[&scan.id].rows
}

#[test]
fn a_page_within_a_prefix_reads_only_its_page() {
    let db = seeded(true);
    let p = params(&[("p", s("a:")), ("after", s("a:2500"))]);
    assert_eq!(
        keys(&db, PAGE, p.clone()),
        ["a:2501", "a:2502", "a:2503", "a:2504", "a:2505"]
    );
    assert_eq!(scan_rows(&db, PAGE, p.clone()), 5);
    // The predicates in the other order.
    let swapped = "MATCH (r:R) WHERE r.key > $after AND r.key STARTS WITH $p \
                   RETURN r.key AS key ORDER BY r.key LIMIT 5";
    assert_eq!(scan_rows(&db, swapped, p), 5);
}

#[test]
fn parameter_bounds_intersect() {
    let db = seeded(true);
    let q = "MATCH (r:R) WHERE r.key >= $a AND r.key > $b AND r.key < $c AND r.key <= $d \
             RETURN r.key AS key ORDER BY r.key LIMIT 3";
    let p = params(&[
        ("a", s("a:1000")),
        ("b", s("a:2000")),
        ("c", s("b:")),
        ("d", s("a:2002")),
    ]);
    assert_eq!(keys(&db, q, p.clone()), ["a:2001", "a:2002"]);
    assert_eq!(scan_rows(&db, q, p), 2);
}

#[test]
fn results_match_the_unindexed_plan() {
    let indexed = seeded(true);
    let plain = seeded(false);
    let cases: Vec<(&str, LoraValue)> = vec![
        ("a:", s("")),
        ("a:", s("a:")),
        ("a:", s("a:2500")),
        ("a:", s("a:2999")),
        ("a:", s("b:1050")),
        ("b:", s("a:2500")),
        ("b:", s("b:1050")),
        ("a:25", s("a:2550")),
        ("", s("a:2998")),
        ("a:", LoraValue::Null),
        ("a:", LoraValue::Int(5)),
    ];
    for q in [
        PAGE,
        "MATCH (r:R) WHERE r.key > $after AND r.key STARTS WITH $p RETURN r.key AS key ORDER BY r.key LIMIT 5",
        "MATCH (r:R) WHERE r.key STARTS WITH $p AND r.key >= $after RETURN r.key AS key ORDER BY r.key DESC LIMIT 5",
        "MATCH (r:R) WHERE r.key STARTS WITH $p AND r.key < $after RETURN r.key AS key ORDER BY r.key LIMIT 5",
        "MATCH (r:R) WHERE r.key STARTS WITH $p AND r.key <= $after RETURN r.key AS key ORDER BY r.key DESC LIMIT 5",
        "MATCH (r:R) WHERE r.key STARTS WITH $p AND r.key > $after RETURN r.key AS key",
    ] {
        for (prefix, after) in &cases {
            let p = params(&[("p", s(prefix)), ("after", after.clone())]);
            let mut got = keys(&indexed, q, p.clone());
            let mut want = keys(&plain, q, p);
            if !q.contains("ORDER BY") {
                got.sort();
                want.sort();
            }
            assert_eq!(got, want, "{q} with p={prefix:?} after={after:?}");
        }
    }
}

#[test]
fn a_keyset_walk_within_a_prefix_visits_each_key_once() {
    let db = seeded(true);
    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let page = keys(&db, PAGE, params(&[("p", s("a:")), ("after", s(&cursor))]));
        if page.is_empty() {
            break;
        }
        cursor = page.last().unwrap().clone();
        seen.extend(page);
    }
    let want: Vec<String> = (1000..3000).map(|i| format!("a:{i}")).collect();
    assert_eq!(seen, want);
}

#[test]
fn explain_shows_every_bound() {
    let db = seeded(true);
    let plan = db.service.explain(PAGE, None).unwrap();
    let scan = find(&plan.tree.root, "NodeByPropertyRangeScan").unwrap();
    let lo = &scan.details["lo"];
    assert!(
        lo.contains(r#"> Parameter("after")"#) && lo.contains(r#">= Parameter("p")"#),
        "{lo}"
    );
    assert!(scan.details["hi"].starts_with("< "), "{:?}", scan.details);
}

#[test]
fn relationship_scans_intersect_parameter_bounds() {
    let seed = |indexed: bool| {
        let db = TestDb::new();
        if indexed {
            db.run("CREATE INDEX t_k FOR ()-[t:T]-() ON (t.k)");
        }
        db.run("UNWIND range(0, 199) AS i CREATE (:A)-[:T {k: i}]->(:B)");
        db
    };
    let (indexed, plain) = (seed(true), seed(false));
    let q =
        "MATCH ()-[t:T]->() WHERE t.k > $a AND t.k >= $b AND t.k < $c RETURN t.k AS k ORDER BY k";
    for (a, b, c) in [(10, 50, 60), (50, 10, 60), (50, 50, 51), (0, 0, 0)] {
        let p = params(&[
            ("a", LoraValue::Int(a)),
            ("b", LoraValue::Int(b)),
            ("c", LoraValue::Int(c)),
        ]);
        assert_eq!(
            indexed.run_with_params(q, p.clone()),
            plain.run_with_params(q, p),
            "a={a} b={b} c={c}"
        );
    }
}
