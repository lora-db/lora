//! `ORDER BY n.key LIMIT k` over a range-indexed property streams from
//! the index instead of sorting every match (keyset pagination).

mod test_helpers;
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use serde_json::json;
use test_helpers::TestDb;

const PAGE: &str =
    "MATCH (f:Festival) WHERE f.key > $after RETURN f.key AS key ORDER BY f.key LIMIT 20";

fn festivals(n: usize, indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE CONSTRAINT k IF NOT EXISTS FOR (n:Festival) REQUIRE n.key IS UNIQUE");
    }
    // Insert in a scrambled order so id order differs from key order.
    db.run(&format!(
        "UNWIND range(0, {}) AS i WITH (i * 7919) % {n} AS j \
         CREATE (:Festival {{key: 'f' + substring('0000000' + toString(j), size(toString(j))), n: j}})",
        n - 1
    ));
    db
}

fn after(v: &str) -> BTreeMap<String, LoraValue> {
    BTreeMap::from([("after".to_string(), LoraValue::String(v.to_string()))])
}

fn keys(db: &TestDb, q: &str, params: BTreeMap<String, LoraValue>) -> Vec<String> {
    db.run_with_params(q, params)
        .iter()
        .map(|r| r["key"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn results_match_the_unindexed_plan() {
    let indexed = festivals(500, true);
    let plain = festivals(500, false);
    for q in [
        PAGE,
        "MATCH (f:Festival) WHERE f.key > $after RETURN f.key AS key ORDER BY key LIMIT 7",
        "MATCH (f:Festival) WHERE f.key < $after RETURN f.key AS key ORDER BY f.key DESC LIMIT 20",
        "MATCH (f:Festival) WHERE f.key >= $after AND f.n % 3 = 0 RETURN f.key AS key ORDER BY f.key LIMIT 20",
        "MATCH (f:Festival) WHERE f.key > $after RETURN f.key AS key ORDER BY f.key SKIP 5 LIMIT 5",
        "MATCH (f:Festival) WHERE f.key > $after RETURN f.key AS key ORDER BY f.key",
    ] {
        for a in ["f0000000", "f0000123", "f0000499", "z"] {
            assert_eq!(keys(&indexed, q, after(a)), keys(&plain, q, after(a)), "{q} after {a}");
        }
    }
}

#[test]
fn keyset_pagination_walks_every_key_once_in_order() {
    let db = festivals(300, true);
    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let page = keys(&db, PAGE, after(&cursor));
        if page.is_empty() {
            break;
        }
        cursor = page.last().unwrap().clone();
        seen.extend(page);
    }
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(seen, sorted);
    assert_eq!(seen.len(), 300);
}

#[test]
fn plan_drops_the_sort() {
    let db = festivals(10, true);
    let plan = format!("{:?}", db.service.explain(PAGE, None).unwrap());
    assert!(!plan.contains("\"Sort\""), "{plan}");
}

#[test]
fn numeric_bounds_still_order_ints_and_floats_together() {
    // The index orders every integer before every float, which is not
    // Cypher's numeric order: the scan must sort these itself.
    let db = TestDb::new();
    db.run("CREATE INDEX v FOR (n:N) ON (n.v)");
    db.run("UNWIND [5, 1.5, 3, 2.5, 4, 0.5] AS v CREATE (:N {v: v})");
    let rows = db.run("MATCH (n:N) WHERE n.v > 0 RETURN n.v AS v ORDER BY n.v LIMIT 4");
    let got: Vec<f64> = rows.iter().map(|r| r["v"].as_f64().unwrap()).collect();
    assert_eq!(got, vec![0.5, 1.5, 2.5, 3.0]);
    let rows = db.run("MATCH (n:N) WHERE n.v > 0 RETURN n.v AS v ORDER BY n.v DESC LIMIT 2");
    assert_eq!(rows, vec![json!({"v": 5}), json!({"v": 4})]);
}

#[test]
fn page_latency_does_not_grow_with_label_size() {
    let time = |db: &TestDb| {
        db.run_with_params(PAGE, after("f0001000"));
        let t = Instant::now();
        for _ in 0..20 {
            db.run_with_params(PAGE, after("f0001000"));
        }
        t.elapsed().as_secs_f64() / 20.0
    };
    let small = time(&festivals(2_000, true));
    let large = time(&festivals(100_000, true));
    // A full sort would make the large case ~50x slower.
    assert!(
        large < small * 5.0 + 0.002,
        "small {small:.5}s, large {large:.5}s"
    );
}
