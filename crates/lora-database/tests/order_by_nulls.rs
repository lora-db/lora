//! `ORDER BY x [ASC|DESC] NULLS FIRST|LAST` (G-77), and the ordered index
//! walk that serves `x DESC NULLS LAST, key` pages without sorting the
//! label: the range index walked in order, then the nodes without `x`,
//! with only each run of equal `x` sorted by `key` (a partial sort).

mod test_helpers;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

fn ints(db: &TestDb, q: &str) -> Vec<JsonValue> {
    db.run(q).into_iter().map(|r| r["v"].clone()).collect()
}

#[test]
fn nulls_go_where_the_sort_item_says() {
    let db = TestDb::new();
    let q = |order: &str| {
        ints(
            &db,
            &format!("UNWIND [10, null, 90] AS v RETURN v ORDER BY v {order}"),
        )
    };
    // Cypher's defaults are unchanged.
    assert_eq!(q("DESC"), vec![json!(null), json!(90), json!(10)]);
    assert_eq!(q("ASC"), vec![json!(10), json!(90), json!(null)]);
    assert_eq!(q(""), vec![json!(10), json!(90), json!(null)]);
    assert_eq!(
        q("DESC NULLS FIRST"),
        vec![json!(null), json!(90), json!(10)]
    );
    assert_eq!(q("ASC NULLS LAST"), vec![json!(10), json!(90), json!(null)]);
    // The two the defaults cannot say.
    assert_eq!(
        q("DESC NULLS LAST"),
        vec![json!(90), json!(10), json!(null)]
    );
    assert_eq!(q("NULLS FIRST"), vec![json!(null), json!(10), json!(90)]);
    // Top-k keeps the same order.
    assert_eq!(q("DESC NULLS LAST LIMIT 2"), vec![json!(90), json!(10)]);
    assert_eq!(q("ASC NULLS FIRST LIMIT 1"), vec![json!(null)]);
    // WITH ... ORDER BY, and a second key breaking the null tie.
    let rows = db.run(
        "UNWIND [[1, null], [2, 5], [3, null], [4, 7]] AS p \
         WITH p[0] AS id, p[1] AS v ORDER BY v DESC NULLS LAST, id DESC LIMIT 3 \
         RETURN id",
    );
    assert_eq!(
        rows,
        vec![json!({"id": 4}), json!({"id": 2}), json!({"id": 3})]
    );
}

// ---------------------------------------------------------------------------
// Ordered index walk with the nulls
// ---------------------------------------------------------------------------

/// `n` artists with a scrambled id order: about one in five unscored, the
/// rest scored with many ties, integers and floats mixed (`7` and `7.0`
/// tie).
fn artists(n: usize, indexed: bool) -> TestDb {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT k IF NOT EXISTS FOR (a:Artist) REQUIRE a.key IS UNIQUE");
    if indexed {
        db.run("CREATE INDEX score IF NOT EXISTS FOR (a:Artist) ON (a.score)");
    }
    db.run(&format!(
        "UNWIND range(0, {}) AS i WITH (i * 7919) % {n} AS j \
         CREATE (:Artist {{key: 'a' + substring('000000' + toString(j), size(toString(j))), \
           score: CASE WHEN j % 5 = 0 THEN null \
                       WHEN j % 3 = 0 THEN toFloat(j % 41) \
                       ELSE j % 41 END}})",
        n - 1
    ));
    db
}

const MIN: f64 = f64::NEG_INFINITY;

/// Forward pages of `score DESC NULLS LAST, key`, as lora-graphql writes
/// them: the first page, after a scored cursor, after an unscored one.
const FWD_FIRST: &str = "MATCH (a:Artist) WHERE a.score >= $min OR a.score IS NULL \
    RETURN a.key AS key, a.score AS score ORDER BY a.score DESC NULLS LAST, a.key LIMIT $n";
const FWD_AFTER_VALUE: &str = "MATCH (a:Artist) \
    WHERE (a.score <= $v AND (a.score < $v OR a.key > $k)) OR a.score IS NULL \
    RETURN a.key AS key, a.score AS score ORDER BY a.score DESC NULLS LAST, a.key LIMIT $n";
const FWD_AFTER_NULL: &str = "MATCH (a:Artist) WHERE a.score IS NULL AND a.key > $k \
    RETURN a.key AS key, a.score AS score ORDER BY a.score DESC NULLS LAST, a.key LIMIT $n";
/// Backward pages: the same sort reversed (`score ASC NULLS FIRST, key
/// DESC`), flipped back by the caller.
const BACK_FIRST: &str = "MATCH (a:Artist) WHERE a.score >= $min OR a.score IS NULL \
    RETURN a.key AS key, a.score AS score ORDER BY a.score ASC NULLS FIRST, a.key DESC LIMIT $n";
const BACK_AFTER_VALUE: &str = "MATCH (a:Artist) \
    WHERE a.score >= $v AND (a.score > $v OR a.key < $k) \
    RETURN a.key AS key, a.score AS score ORDER BY a.score ASC NULLS FIRST, a.key DESC LIMIT $n";
const BACK_AFTER_NULL: &str = "MATCH (a:Artist) \
    WHERE (a.score IS NULL AND a.key < $k) OR a.score >= $min \
    RETURN a.key AS key, a.score AS score ORDER BY a.score ASC NULLS FIRST, a.key DESC LIMIT $n";

type Item = (String, Option<f64>);

fn params(n: usize, cursor: Option<&Item>) -> BTreeMap<String, LoraValue> {
    let mut p = BTreeMap::from([
        ("n".to_string(), LoraValue::Int(n as i64)),
        ("min".to_string(), LoraValue::Float(MIN)),
    ]);
    if let Some((k, v)) = cursor {
        p.insert("k".into(), LoraValue::String(k.clone()));
        p.insert("v".into(), v.map_or(LoraValue::Null, LoraValue::Float));
    }
    p
}

fn page(db: &TestDb, q: &str, p: BTreeMap<String, LoraValue>) -> Vec<Item> {
    db.run_with_params(q, p)
        .iter()
        .map(|r| (r["key"].as_str().unwrap().to_string(), r["score"].as_f64()))
        .collect()
}

/// Every artist in `score DESC NULLS LAST, key ASC` order.
fn reference(db: &TestDb) -> Vec<Item> {
    let mut all = page(
        db,
        "MATCH (a:Artist) RETURN a.key AS key, a.score AS score",
        BTreeMap::new(),
    );
    all.sort_by(|(ka, va), (kb, vb)| {
        let by_score = match (va, vb) {
            (None, None) => Ordering::Equal,
            (None, _) => Ordering::Greater,
            (_, None) => Ordering::Less,
            (Some(a), Some(b)) => b.partial_cmp(a).unwrap(),
        };
        by_score.then_with(|| ka.cmp(kb))
    });
    all
}

fn forward(db: &TestDb, size: usize) -> Vec<Item> {
    let mut seen: Vec<Item> = Vec::new();
    loop {
        let q = match seen.last() {
            None => FWD_FIRST,
            Some((_, Some(_))) => FWD_AFTER_VALUE,
            Some((_, None)) => FWD_AFTER_NULL,
        };
        let got = page(db, q, params(size, seen.last()));
        assert!(got.len() <= size);
        let done = got.len() < size;
        seen.extend(got);
        if done {
            return seen;
        }
    }
}

fn backward(db: &TestDb, size: usize) -> Vec<Item> {
    let mut seen: Vec<Item> = Vec::new();
    loop {
        let q = match seen.last() {
            None => BACK_FIRST,
            Some((_, Some(_))) => BACK_AFTER_VALUE,
            Some((_, None)) => BACK_AFTER_NULL,
        };
        let got = page(db, q, params(size, seen.last()));
        let done = got.len() < size;
        seen.extend(got);
        if done {
            seen.reverse();
            return seen;
        }
    }
}

#[test]
fn keyset_pages_cross_values_ties_and_nulls_both_ways() {
    for indexed in [true, false] {
        let db = artists(600, indexed);
        let expected = reference(&db);
        assert_eq!(expected.len(), 600);
        assert!(expected[0].1.is_some() && expected[599].1.is_none());
        for size in [1, 7, 24, 600] {
            assert_eq!(
                forward(&db, size),
                expected,
                "forward size {size} indexed {indexed}"
            );
            assert_eq!(
                backward(&db, size),
                expected,
                "backward size {size} indexed {indexed}"
            );
        }
    }
}

#[test]
fn indexed_pages_match_the_unindexed_plan() {
    let indexed = artists(300, true);
    let plain = artists(300, false);
    let all = reference(&plain);
    let single = "MATCH (a:Artist) WHERE a.score > $v OR a.score IS NULL \
        RETURN a.key AS key, a.score AS score ORDER BY a.score NULLS FIRST LIMIT $n";
    for q in [
        FWD_FIRST,
        FWD_AFTER_VALUE,
        FWD_AFTER_NULL,
        BACK_FIRST,
        BACK_AFTER_VALUE,
        BACK_AFTER_NULL,
    ] {
        for cursor in [
            &all[0], &all[37], &all[150], &all[239], &all[240], &all[299],
        ] {
            for n in [0, 1, 5, 40, 400] {
                let p = || params(n, Some(cursor));
                assert_eq!(
                    page(&indexed, q, p()),
                    page(&plain, q, p()),
                    "{q} {cursor:?} {n}"
                );
            }
        }
    }
    // A single key: NULLS FIRST walks the nulls, then the range. Ties
    // may come in any order, so compare scores.
    let scores = |db: &TestDb| -> Vec<Option<f64>> {
        page(db, single, params(100, Some(&all[100])))
            .into_iter()
            .map(|(_, v)| v)
            .collect()
    };
    assert_eq!(scores(&indexed), scores(&plain));
}

fn operators(db: &TestDb, q: &str) -> Vec<(String, BTreeMap<String, String>)> {
    fn walk(n: &lora_database::PlanTreeNode, out: &mut Vec<(String, BTreeMap<String, String>)>) {
        out.push((n.operator.clone(), n.details.clone()));
        for c in &n.children {
            walk(c, out);
        }
    }
    let plan = db.service.explain(q, Some(params(24, None))).unwrap();
    let mut out = Vec::new();
    walk(&plan.tree.root, &mut out);
    out
}

#[test]
fn pages_walk_the_index_with_a_partial_sort() {
    let db = artists(50, true);
    for q in [FWD_FIRST, FWD_AFTER_VALUE, BACK_FIRST, BACK_AFTER_NULL] {
        let ops = operators(&db, q);
        let names: Vec<&str> = ops.iter().map(|(o, _)| o.as_str()).collect();
        assert!(names.contains(&"PartialSort"), "{q}: {names:?}");
        assert!(!names.contains(&"Sort"), "{q}: {names:?}");
        let scan = ops
            .iter()
            .find(|(o, _)| o == "NodeByPropertyRangeScan")
            .expect(q);
        assert_eq!(scan.1["key"], "score");
        let nulls = if q.contains("NULLS FIRST") {
            "FIRST"
        } else {
            "LAST"
        };
        assert_eq!(scan.1.get("nulls").map(String::as_str), Some(nulls), "{q}");
    }
    // After a scored cursor backward the page holds no nulls: a plain
    // ordered range walk.
    let ops = operators(&db, BACK_AFTER_VALUE);
    assert!(ops
        .iter()
        .any(|(o, d)| o == "NodeByPropertyRangeScan" && !d.contains_key("nulls")));
    assert!(ops.iter().any(|(o, _)| o == "PartialSort"));
    // Inside the null group there is no range to walk: it sorts.
    let ops = operators(&db, FWD_AFTER_NULL);
    assert!(ops.iter().any(|(o, _)| o == "Sort"));
    // One key: no sort at all.
    let ops = operators(
        &db,
        "MATCH (a:Artist) WHERE a.score >= $min OR a.score IS NULL \
         RETURN a.key AS key ORDER BY a.score DESC NULLS LAST LIMIT $n",
    );
    assert!(!ops.iter().any(|(o, _)| o.ends_with("Sort")), "{ops:?}");
    // Without the index, or without the null test, nothing changes.
    let plain = artists(50, false);
    assert!(operators(&plain, FWD_FIRST)
        .iter()
        .any(|(o, _)| o == "Sort"));
}

/// The scan feeds the partial sort about one page plus one run, not the
/// label: 35,000 artists, a page of 24.
#[test]
fn a_nulls_last_page_reads_about_a_page() {
    let db = artists(35_000, true);
    let profile = db
        .service
        .profile(FWD_FIRST, Some(params(24, None)))
        .unwrap();
    let scan_id = {
        fn find(n: &lora_database::PlanTreeNode) -> Option<usize> {
            if n.operator == "NodeByPropertyRangeScan" {
                return Some(n.id);
            }
            n.children.iter().find_map(find)
        }
        find(&profile.plan.tree.root).expect("range scan")
    };
    let rows = profile.metrics.per_operator[&scan_id].rows;
    // 41 distinct scores over 28,000 scored artists: a run is ~680 rows.
    assert!(rows < 2_000, "the scan produced {rows} rows");
    assert_eq!(profile.metrics.total_rows, 24);
}

#[test]
fn nulls_last_page_latency_matches_a_key_page() {
    let db = TestDb::new();
    db.run("CREATE CONSTRAINT k IF NOT EXISTS FOR (a:Artist) REQUIRE a.key IS UNIQUE");
    db.run("CREATE INDEX score IF NOT EXISTS FOR (a:Artist) ON (a.score)");
    // Distinct scores (popularity), a fifth unscored.
    db.run(
        "UNWIND range(0, 34999) AS i WITH (i * 7919) % 35000 AS j \
         CREATE (:Artist {key: 'a' + substring('000000' + toString(j), size(toString(j))), \
           score: CASE WHEN j % 5 = 0 THEN null ELSE toFloat(j) / 7.0 END})",
    );
    let key_page = "MATCH (a:Artist) WHERE a.key >= '' RETURN a.key AS key ORDER BY a.key LIMIT $n";
    let time = |q: &str| {
        db.run_with_params(q, params(24, None));
        let t = Instant::now();
        for _ in 0..50 {
            assert_eq!(db.run_with_params(q, params(24, None)).len(), 24);
        }
        t.elapsed().as_secs_f64() / 50.0
    };
    let key = time(key_page);
    let nulls_last = time(FWD_FIRST);
    // A sort of the label would be two orders of magnitude slower.
    assert!(
        nulls_last < key * 5.0 + 0.002,
        "key page {key:.6}s, nulls-last page {nulls_last:.6}s"
    );
    eprintln!("35k artists, page of 24: key ASC {key:.6}s, score DESC NULLS LAST {nulls_last:.6}s");
}
