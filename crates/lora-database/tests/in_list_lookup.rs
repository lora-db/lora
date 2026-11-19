//! `x IN list` against a list built once per query (taste N-10, N-10b).
//!
//! A check against a list the row carries, a parameter, or a list inside a
//! carried map or list (`x IN m.l`, `x IN tiers[r]`) is a hash lookup once
//! the list has been tested a few times, and reading the inner list does
//! not copy it. The answers are the scan's: three-valued with nulls, `1`
//! finds `1.0`, and values a hash cannot agree with fall back to the scan.

mod test_helpers;
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use serde_json::json;
use test_helpers::TestDb;

#[test]
fn answers_match_the_scan() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH [x IN range(1, 100) | toString(x)] + [null] AS s, \
              range(1, 100) + [2.5, 9007199254740993] AS n, \
              {l: [x IN range(1, 100) | 'g' + toString(x)]} AS m, \
              [range(1, 50), range(51, 100)] AS tiers \
         UNWIND range(1, 5) AS rep \
         RETURN rep, \
                '7' IN s AS s_hit, 'x' IN s AS s_null, null IN s AS s_lhs_null, 7 IN s AS s_kind, \
                7 IN n AS n_hit, 7.0 IN n AS n_float, 2.5 IN n AS n_frac, 101 IN n AS n_miss, \
                9007199254740993 IN n AS n_big, 'g42' IN m.l AS m_hit, 'h' IN m.l AS m_miss, \
                60 IN tiers[1] AS t_hit, 60 IN tiers[0] AS t_miss, 0.0 IN [x IN range(-40, 0) | x * 1.0] AS zero \
         ORDER BY rep",
    );
    assert_eq!(rows.len(), 5);
    for r in rows {
        assert_eq!(r["s_hit"], true, "{r}");
        assert_eq!(r["s_null"], json!(null), "{r}");
        assert_eq!(r["s_lhs_null"], json!(null), "{r}");
        assert_eq!(r["s_kind"], json!(null), "{r}");
        assert_eq!(r["n_hit"], true, "{r}");
        assert_eq!(r["n_float"], true, "{r}");
        assert_eq!(r["n_frac"], true, "{r}");
        assert_eq!(r["n_miss"], false, "{r}");
        assert_eq!(r["n_big"], true, "{r}");
        assert_eq!(r["m_hit"], true, "{r}");
        assert_eq!(r["m_miss"], false, "{r}");
        assert_eq!(r["t_hit"], true, "{r}");
        assert_eq!(r["t_miss"], false, "{r}");
        assert_eq!(r["zero"], true, "{r}");
    }
}

#[test]
fn a_parameter_list_is_looked_up() {
    let db = TestDb::new();
    let list: Vec<LoraValue> = (0..1000)
        .map(|i| LoraValue::String(format!("g{i}")))
        .collect();
    let mut params = BTreeMap::new();
    params.insert("l".to_string(), LoraValue::List(list));
    let rows = db.run_with_params(
        "UNWIND range(990, 1010) AS i WITH i WHERE 'g' + toString(i) IN $l RETURN count(*) AS n",
        params,
    );
    assert_eq!(rows[0]["n"], 10);
}

fn per_check(db: &TestDb, len: usize, nested: bool) -> f64 {
    let probe = if nested { "m.l" } else { "l" };
    let q = format!(
        "WITH [x IN range(1, {len}) | 'g' + toString(x)] AS l \
         WITH l, {{l: l}} AS m \
         RETURN size([i IN range(1, 5000) WHERE 'g' + toString({len} - i % 7) IN {probe} | i]) AS n"
    );
    db.run(&q);
    (0..5)
        .map(|_| {
            let start = Instant::now();
            db.run(&q);
            start.elapsed().as_secs_f64()
        })
        .fold(f64::MAX, f64::min)
}

#[test]
fn a_lookup_does_not_grow_with_the_list() {
    let db = TestDb::new();
    let small = per_check(&db, 100, false);
    let large = per_check(&db, 20_000, false);
    // A scan that finds its element near the end is ~200x slower at 20k.
    assert!(
        large < small * 5.0 + 0.05,
        "100-element list {small:.5}s, 20k-element list {large:.5}s"
    );
}

#[test]
fn a_list_inside_a_map_is_neither_copied_nor_scanned() {
    let db = TestDb::new();
    let flat = per_check(&db, 20_000, false);
    let nested = per_check(&db, 20_000, true);
    assert!(
        nested < flat * 3.0 + 0.05,
        "x IN l {flat:.5}s, x IN m.l {nested:.5}s"
    );
}
