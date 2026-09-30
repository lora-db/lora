//! `ORDER BY … LIMIT $n` bounds its sort at run time, keeping only
//! `skip + limit` rows, and returns exactly what the literal form returns.

mod test_helpers;
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use test_helpers::TestDb;

fn festivals(n: usize) -> TestDb {
    let db = TestDb::new();
    db.run("CREATE INDEX fc FOR (n:Festival) ON (n.capacity)");
    // Many ties on `name` so arrival order decides among equal keys.
    db.run(&format!(
        "UNWIND range(0, {}) AS i WITH i, (i * 7919) % {n} AS j \
         CREATE (:Festival {{key: 'k' + toString(j), name: CASE WHEN j % 7 = 0 THEN null ELSE 'n' + toString(j % 13) END, capacity: j % 1000}})",
        n - 1
    ));
    db
}

fn params(pairs: &[(&str, i64)]) -> BTreeMap<String, LoraValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), LoraValue::Int(*v)))
        .collect()
}

#[test]
fn parameter_limits_match_literal_limits() {
    let db = festivals(400);
    for (skip, limit) in [
        (0, 0),
        (0, 1),
        (0, 25),
        (3, 10),
        (390, 20),
        (0, 1000),
        (-1, 5),
    ] {
        for order in [
            "this.name ASC",
            "this.name DESC, this.capacity",
            "this.capacity DESC",
        ] {
            let tail = " RETURN this.key AS k";
            let skip_lit = if skip < 0 {
                String::new()
            } else {
                format!(" SKIP {skip}")
            };
            let skip_par = if skip < 0 { "" } else { " SKIP $s" };
            let lit = format!(
                "MATCH (this:Festival) WHERE this.capacity > 20 WITH this ORDER BY {order}{skip_lit} LIMIT {limit}{tail}"
            );
            let par = format!(
                "MATCH (this:Festival) WHERE this.capacity > $p0 WITH this ORDER BY {order}{skip_par} LIMIT $n{tail}"
            );
            let expected = db.run(&lit);
            let got = db.run_with_params(
                &par,
                params(&[("p0", 20), ("s", skip.max(0)), ("n", limit)]),
            );
            assert_eq!(got, expected, "{par} skip={skip} limit={limit}");
        }
    }
}

#[test]
fn plan_bounds_the_sort_by_the_parameter() {
    let db = festivals(10);
    let plan = format!(
        "{:?}",
        db.service
            .explain(
                "MATCH (f:Festival) RETURN f.key AS k ORDER BY f.name SKIP $s LIMIT $n",
                None,
            )
            .unwrap()
    );
    assert!(plan.contains("\"top_k\""), "{plan}");
}

#[test]
#[ignore]
fn probe() {
    for n in [20_000usize, 100_000] {
        let db = festivals(n);
        let params = params(&[("p0", 20), ("p1", 25)]);
        for q in [
            "MATCH (this:Festival) WHERE this.capacity > $p0 WITH this ORDER BY this.name ASC LIMIT $p1 RETURN this { .key } AS this",
            "MATCH (this:Festival) WHERE this.capacity > $p0 RETURN count(this) AS c",
            "MATCH (f:Festival) WITH f ORDER BY f.key LIMIT 3 RETURN f.key AS k",
        ] {
            db.run_with_params(q, params.clone());
            let t = Instant::now();
            for _ in 0..5 {
                db.run_with_params(q, params.clone());
            }
            println!(
                "n={n} {:.2} ms  {q}",
                t.elapsed().as_secs_f64() * 1000.0 / 5.0
            );
        }
    }
}

#[test]
fn parameter_limit_bounds_a_writing_query() {
    let db = festivals(200);
    let q = "MATCH (f:Festival) WITH f ORDER BY f.name DESC, f.key SKIP $s LIMIT $n \
             SET f.picked = true RETURN f.key AS k";
    let got = db.run_with_params(q, params(&[("s", 2), ("n", 7)]));
    let expected = db.run(
        "MATCH (f:Festival) WITH f ORDER BY f.name DESC, f.key SKIP 2 LIMIT 7 RETURN f.key AS k",
    );
    assert_eq!(got, expected);
    db.assert_count("MATCH (f:Festival {picked: true}) RETURN f", 7);
}
