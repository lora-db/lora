//! A WHERE on a property inside a pattern subquery (comprehension, EXISTS,
//! COUNT) finds its node through the property's index, as the inline
//! property map does and as `MATCH` does for the same WHERE: equality,
//! `IN` a list and `STARTS WITH`, on the first node of the pattern or the
//! last.
//!
//! Regression: `[(p:P)-[:LIKES]->(g:G) WHERE p.key = $key | g.key]` scanned
//! `:P` once per row it was evaluated for and applied the WHERE to every
//! match, where `[(:P {key: $key})-[:LIKES]->(g:G) | g.key]` did one seek:
//! 4.5 s against 0.7 ms with 30,000 `:P` and 300 rows.

mod test_helpers;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use lora_database::LoraValue;
use test_helpers::TestDb;

fn seeded(people: usize) -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT p_key FOR (p:P) REQUIRE p.key IS UNIQUE");
    db.run("CREATE INDEX p_name FOR (p:P) ON (p.name)");
    db.run(&format!(
        "UNWIND range(1, {people}) AS i \
         CREATE (:P {{key: 'p' + toString(i), name: 'n' + toString(i) + '/', odd: i % 2 = 1}})\
                -[:LIKES]->(:G {{key: 'g' + toString(i)}})"
    ));
    db.run("UNWIND range(1, 100) AS i CREATE (:F {key: toString(i)})");
    db
}

fn params() -> BTreeMap<String, LoraValue> {
    let strings = |items: &[&str]| {
        LoraValue::List(
            items
                .iter()
                .map(|s| LoraValue::String(s.to_string()))
                .collect(),
        )
    };
    BTreeMap::from([
        ("key".to_string(), LoraValue::String("p77".into())),
        ("keys".to_string(), strings(&["p78", "p77", "p0", "p77"])),
        ("prefix".to_string(), LoraValue::String("n77/".into())),
        ("none".to_string(), LoraValue::Null),
    ])
}

/// Each comprehension with what it yields for one row, at 100 `:P` or more.
const FORMS: [(&str, &[&str]); 9] = [
    ("[(:P {key: $key})-[:LIKES]->(g:G) | g.key]", &["g77"]),
    (
        "[(p:P)-[:LIKES]->(g:G) WHERE p.key = $key | g.key]",
        &["g77"],
    ),
    (
        "[(p:P)-[:LIKES]->(g:G) WHERE $key = p.key | g.key]",
        &["g77"],
    ),
    (
        "[(p:P)-[:LIKES]->(g:G) WHERE p.key IN $keys | g.key]",
        &["g77", "g78"],
    ),
    (
        "[(p:P)-[:LIKES]->(g:G) WHERE p.name STARTS WITH $prefix | g.key]",
        &["g77"],
    ),
    // The WHERE pins the last node: the walk starts there.
    (
        "[(g:G)<-[:LIKES]-(p:P) WHERE p.key = $key | g.key]",
        &["g77"],
    ),
    (
        "[(g:G)<-[:LIKES]-(p:P) WHERE p.key IN $keys | g.key]",
        &["g77", "g78"],
    ),
    // Other conjuncts still apply.
    (
        "[(p:P)-[:LIKES]->(g:G) WHERE p.key = $key AND p.odd | g.key]",
        &["g77"],
    ),
    (
        "[(p:P)-[:LIKES]->(g:G) WHERE p.odd = false AND p.key = $key | g.key]",
        &[],
    ),
];

fn statement(pattern: &str) -> String {
    format!(
        "MATCH (f:F) WITH f, {pattern} AS liked \
         RETURN count(f) AS rows, collect(DISTINCT liked) AS lists"
    )
}

#[test]
fn where_seeks_answer_what_the_scan_did() {
    let db = seeded(100);
    for (pattern, expected) in FORMS {
        let rows = db.run_with_params(&statement(pattern), params());
        assert_eq!(rows[0]["rows"], 100, "{pattern}");
        assert_eq!(rows[0]["lists"], serde_json::json!([expected]), "{pattern}");
    }
}

#[test]
fn a_where_seek_keeps_the_semantics_of_its_predicate() {
    let db = seeded(100);
    let list = |pattern: &str| {
        db.run_with_params(&format!("RETURN {pattern} AS l"), params())[0]["l"].clone()
    };
    let empty = serde_json::json!([]);
    // Null never equals, is in nothing, and prefixes nothing.
    assert_eq!(list("[(p:P) WHERE p.key = $none | p.key]"), empty);
    assert_eq!(list("[(p:P) WHERE p.key IN $none | p.key]"), empty);
    assert_eq!(list("[(p:P) WHERE p.key STARTS WITH $none | p.key]"), empty);
    // Values no index holds a key of: the WHERE still judges every node.
    assert_eq!(list("[(p:P) WHERE p.key = 77 | p.key]"), empty);
    assert_eq!(list("[(p:P) WHERE p.key STARTS WITH 77 | p.key]"), empty);
    assert_eq!(list("[(p:P) WHERE p.key IN [] | p.key]"), empty);
    assert_eq!(
        list("[(p:P) WHERE p.key STARTS WITH 'p10' | p.key]"),
        serde_json::json!(["p10", "p100"])
    );
    // No label, and a property with no index.
    assert_eq!(
        list("[(p) WHERE p.key = $key | p.name]"),
        serde_json::json!(["n77/"])
    );
    assert_eq!(
        list("[(p:P) WHERE p.odd = true AND p.key IN ['p1', 'p2', 'p3'] | p.key]"),
        serde_json::json!(["p1", "p3"])
    );
    // A value the pattern itself binds is no seek: it is not known up front.
    assert_eq!(
        list("[(p:P)-[:LIKES]->(g:G) WHERE p.key = 'p' + substring(g.key, 1) AND g.key = 'g5' | p.key]"),
        serde_json::json!(["p5"])
    );
    // A node the row already binds is matched on, not sought.
    let rows = db.run_with_params(
        "MATCH (p:P {key: 'p5'}) RETURN [(p)-[:LIKES]->(g:G) WHERE p.key = $key | g.key] AS a, \
         [(p)-[:LIKES]->(g:G) WHERE p.key = 'p5' | g.key] AS b, \
         EXISTS { MATCH (g:G)<-[:LIKES]-(p) WHERE g.key = 'g5' } AS c, \
         EXISTS { MATCH (g:G)<-[:LIKES]-(p) WHERE g.key = 'g6' } AS d",
        params(),
    );
    assert_eq!(rows[0]["a"], empty);
    assert_eq!(rows[0]["b"], serde_json::json!(["g5"]));
    assert_eq!(rows[0]["c"], true);
    assert_eq!(rows[0]["d"], false);
}

#[test]
fn a_chain_is_not_walked_backwards_when_a_map_reads_its_own_variables() {
    let db = seeded(10);
    db.run("MATCH (p:P)-[:LIKES]->(g:G) SET g.fan = p.key");
    // `g`'s map reads `p`, which only the written order binds before `g`,
    // so the seek on `g` must not turn the walk around.
    let rows = db.run_with_params(
        "RETURN [(p:P)-[:LIKES]->(g:G {fan: p.key}) WHERE g.key = 'g3' | p.key] AS l",
        params(),
    );
    assert_eq!(rows[0]["l"], serde_json::json!(["p3"]));
}

#[test]
fn where_seeks_do_not_scan_the_label_per_row() {
    let time = |db: &TestDb, pattern: &str| {
        let q = statement(pattern);
        db.run_with_params(&q, params());
        let start = Instant::now();
        for _ in 0..3 {
            db.run_with_params(&q, params());
        }
        start.elapsed()
    };
    let small = seeded(1_000);
    let large = seeded(20_000);
    for (pattern, _) in FORMS {
        // Looking up `p.odd = false` to find it leaves more nodes than the
        // key does reads half the label's index entries, which grows.
        if pattern.contains("p.odd = false") {
            continue;
        }
        let (a, b) = (time(&small, pattern), time(&large, pattern));
        // A label scan per row grows 20x with the label; a seek stays flat.
        assert!(
            b < a * 5 + Duration::from_millis(20),
            "{pattern}: {a:?} at 1k nodes, {b:?} at 20k"
        );
    }
}

#[test]
fn explain_names_the_seek() {
    let db = seeded(10);
    let plan = |q: &str| format!("{:?}", db.service.explain(q, None).unwrap().tree);
    let seeks = |q: &str| {
        let plan = plan(q);
        let at = plan.find("\"pattern_seeks\": \"").map(|i| i + 18);
        at.map(|i| plan[i..i + plan[i..].find('"').unwrap()].to_string())
    };
    assert_eq!(
        seeks("MATCH (f:F) RETURN [(p:P)-[:LIKES]->(g:G) WHERE p.key = $key | g.key] AS l")
            .as_deref(),
        Some("P.key =")
    );
    assert_eq!(
        seeks("MATCH (f:F) WHERE EXISTS { MATCH (g:G)<-[:LIKES]-(p:P) WHERE p.key IN $keys } RETURN f")
            .as_deref(),
        Some("P.key IN")
    );
    assert_eq!(
        seeks(
            "MATCH (f:F) WITH f, [(p:P {key: $key}) | p] AS a, \
             [(p:P) WHERE p.name STARTS WITH $prefix | p] AS b RETURN f"
        )
        .as_deref(),
        Some("P.key =, P.name STARTS WITH")
    );
    assert_eq!(
        seeks("MATCH (f:F) RETURN [(p:P)-[:LIKES]->(g:G) WHERE p.odd | g.key] AS l"),
        None
    );
}
