//! A pattern subquery (comprehension, EXISTS, COUNT) whose start node is
//! not bound by the outer row and carries an inline property map seeks that
//! property, as `MATCH` does, instead of scanning the label once per row.

mod test_helpers;
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use test_helpers::TestDb;

fn seeded(people: usize) -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT pk FOR (p:Person) REQUIRE p.subject IS UNIQUE");
    db.run(&format!(
        "UNWIND range(1, {people}) AS i CREATE (:Person {{subject: 's' + toString(i), verified: i % 2 = 0}})"
    ));
    db.run("UNWIND range(1, 200) AS i CREATE (:Post {k: i})");
    db
}

fn params(s: &str) -> BTreeMap<String, LoraValue> {
    BTreeMap::from([("s".to_string(), LoraValue::String(s.to_string()))])
}

const FORMS: [&str; 3] = [
    "MATCH (n:Post) WHERE size([(v:Person {subject: $s}) WHERE v.verified = true | 1]) > 0 RETURN count(n) AS c",
    "MATCH (n:Post) WHERE EXISTS { MATCH (v:Person {subject: $s}) WHERE v.verified = true } RETURN count(n) AS c",
    "MATCH (n:Post) WHERE COUNT { MATCH (v:Person {subject: $s}) WHERE v.verified = true } > 0 RETURN count(n) AS c",
];

#[test]
fn inline_property_subqueries_are_correct() {
    let db = seeded(100);
    for q in FORMS {
        assert_eq!(db.run_with_params(q, params("s2"))[0]["c"], 200, "{q}");
        // s3 exists but is not verified; s999 does not exist.
        assert_eq!(db.run_with_params(q, params("s3"))[0]["c"], 0, "{q}");
        assert_eq!(db.run_with_params(q, params("s999"))[0]["c"], 0, "{q}");
    }
    // A null value matches nothing, as in MATCH.
    let null = BTreeMap::from([("s".to_string(), LoraValue::Null)]);
    assert_eq!(db.run_with_params(FORMS[0], null)[0]["c"], 0);
}

#[test]
fn inline_property_subqueries_do_not_scan_the_label_per_row() {
    let time = |db: &TestDb, q: &str| {
        db.run_with_params(q, params("s2"));
        let start = Instant::now();
        for _ in 0..3 {
            db.run_with_params(q, params("s2"));
        }
        start.elapsed()
    };
    let small = seeded(1_000);
    let large = seeded(20_000);
    for q in FORMS {
        let (a, b) = (time(&small, q), time(&large, q));
        // A label scan per row grows 20x with the label; a seek stays flat.
        assert!(
            b < a * 5 + std::time::Duration::from_millis(20),
            "{q}: {a:?} at 1k people, {b:?} at 20k"
        );
    }
}
