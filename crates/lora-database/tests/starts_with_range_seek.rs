//! `x STARTS WITH s` seeks a RANGE index as `s <= x < string.prefix_end(s)`
//! when no TEXT index covers the property, with the exact same rows.

mod test_helpers;
use std::collections::BTreeMap;

use lora_database::LoraValue;
use test_helpers::TestDb;

fn seeded() -> TestDb {
    let db = TestDb::new();
    db.run("CREATE INDEX post_video FOR (p:Post) ON (p.video)");
    db.run(
        "UNWIND range(1, 200) AS i CREATE (:Post {key: 'p' + i, video: CASE WHEN i % 10 = 0 THEN 'https://x/' + i ELSE null END})",
    );
    db.run("CREATE (:Post {key: 'h', video: 'http://y'}), (:Post {key: 'n', video: 42})");
    db
}

fn plan(db: &TestDb, q: &str) -> String {
    format!("{:?}", db.service.explain(q, None).unwrap().tree)
}

#[test]
fn starts_with_seeks_the_range_index() {
    let db = seeded();
    let q = "MATCH (p:Post) WHERE p.video STARTS WITH 'https://' RETURN count(p) AS n";
    assert!(
        plan(&db, q).contains("NodeByPropertyRangeScan"),
        "{}",
        plan(&db, q)
    );
    assert_eq!(db.scalar(q), 20);
    // A parameter prefix too.
    let rows = db.run_with_params(
        "MATCH (p:Post) WHERE p.video STARTS WITH $s RETURN count(p) AS n",
        BTreeMap::from([("s".to_string(), LoraValue::String("http".into()))]),
    );
    assert_eq!(rows[0]["n"], 21);
}

#[test]
fn edge_prefixes_match_exactly() {
    let db = seeded();
    let count = |s: &str| {
        db.run_with_params(
            "MATCH (p:Post) WHERE p.video STARTS WITH $s RETURN count(p) AS n",
            BTreeMap::from([("s".to_string(), LoraValue::String(s.into()))]),
        )[0]["n"]
            .clone()
    };
    // The empty prefix: every string, no successor.
    assert_eq!(count(""), 21);
    assert_eq!(count("https://x/20"), 2); // 20 and 200
    assert_eq!(count("zzz"), 0);
    assert_eq!(db.scalar("RETURN string.prefix_end('ab') AS v"), "ac");
    assert_eq!(
        db.scalar("RETURN string.prefix_end('') AS v"),
        serde_json::Value::Null
    );
}
