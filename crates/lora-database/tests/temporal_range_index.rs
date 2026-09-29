//! RANGE indexes hold temporal values (E17).
//!
//! Before the fix `PropertyIndexKey::from_value` skipped every temporal
//! value, so a RANGE index held none of them, yet the planner still routed
//! `WHERE n.createdAt > $t` through the index: the query returned no rows.
//! Comparisons on `localdatetime`, `time` and `localtime` values also
//! always returned false, and `ORDER BY` on them compared debug strings.

mod test_helpers;
use std::collections::BTreeMap;
use std::time::Instant;

use lora_database::LoraValue;
use lora_store::LoraDateTime;
use serde_json::{json, Value as JsonValue};
use test_helpers::TestDb;

const FEED: &str = "MATCH (p:Post) WHERE p.createdAt > $t \
                    RETURN p.i AS i ORDER BY p.createdAt DESC LIMIT 20";

fn posts(n: usize, indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE INDEX post_created FOR (p:Post) ON (p.createdAt)");
    }
    // Scrambled so id order differs from time order; one post per minute.
    db.run(&format!(
        "UNWIND range(0, {}) AS i WITH i, (i * 7919) % {n} AS j \
         CREATE (:Post {{i: j, createdAt: ('2024-01-01T00:00:00Z'::DATETIME) + ({{minutes: j}}::DURATION)}})",
        n - 1
    ));
    db
}

fn at(s: &str) -> BTreeMap<String, LoraValue> {
    BTreeMap::from([(
        "t".to_string(),
        LoraValue::DateTime(LoraDateTime::parse(s).unwrap()),
    )])
}

fn ints(rows: &[JsonValue], col: &str) -> Vec<i64> {
    rows.iter().map(|r| r[col].as_i64().unwrap()).collect()
}

#[test]
fn newest_first_feed_is_index_backed_and_correct() {
    let indexed = posts(2_000, true);
    let plain = posts(2_000, false);

    let plan = format!("{:?}", indexed.service.explain(FEED, None).unwrap());
    assert!(!plan.contains("\"Sort\""), "{plan}");

    for t in [
        "2023-12-31T00:00:00Z",
        "2024-01-01T10:00:00Z",
        "2024-01-01T12:00:00+02:00",
        "2024-01-02T09:19:00Z",
        "2030-01-01T00:00:00Z",
    ] {
        let got = ints(&indexed.run_with_params(FEED, at(t)), "i");
        let want = ints(&plain.run_with_params(FEED, at(t)), "i");
        assert_eq!(got, want, "after {t}");
    }
    let got = ints(
        &indexed.run_with_params(FEED, at("2024-01-01T10:00:00Z")),
        "i",
    );
    assert_eq!(got, (1980..=1999).rev().collect::<Vec<_>>());
    assert_eq!(got.len(), 20);
}

#[test]
fn feed_latency_does_not_grow_with_label_size() {
    let time = |db: &TestDb| {
        let params = at("2024-01-01T00:00:00Z");
        assert_eq!(db.run_with_params(FEED, params.clone()).len(), 20);
        let t = Instant::now();
        for _ in 0..20 {
            db.run_with_params(FEED, params.clone());
        }
        t.elapsed().as_secs_f64() / 20.0
    };
    let small = time(&posts(2_000, true));
    let large_db = posts(20_000, true);
    let large = time(&large_db);
    let unindexed = time(&posts(20_000, false));
    eprintln!(
        "feed at 20k: indexed {:.3} ms, unindexed {:.3} ms (2k indexed {:.3} ms)",
        large * 1e3,
        unindexed * 1e3,
        small * 1e3
    );
    assert!(
        large < small * 5.0 + 0.002,
        "small {small:.5}s, large {large:.5}s"
    );
}

/// Every temporal kind, with range and equality predicates, gives the
/// same rows with and without the index.
#[test]
fn every_temporal_kind_matches_the_unindexed_plan() {
    let seed = |indexed: bool| {
        let db = TestDb::new();
        if indexed {
            db.run("CREATE INDEX ev_t FOR (e:Ev) ON (e.t)");
        }
        db.run(
            "UNWIND range(0, 29) AS i CREATE \
             (:Ev {i: i, kind: 'date', t: ('2024-01-01'::DATE) + ({days: i}::DURATION)}), \
             (:Ev {i: i, kind: 'datetime', t: ('2024-01-01T00:00:00Z'::DATETIME) + ({hours: i}::DURATION)}), \
             (:Ev {i: i, kind: 'localdatetime', t: ('2024-09-01T00:00:00'::LOCAL_DATETIME) + ({days: i * 3}::DURATION)}), \
             (:Ev {i: i, kind: 'time', t: ('08:00:00+01:00'::TIME) + ({minutes: i * 7}::DURATION)}), \
             (:Ev {i: i, kind: 'localtime', t: ('08:00:00'::LOCAL_TIME) + ({seconds: i * 13}::DURATION)}), \
             (:Ev {i: i, kind: 'string', t: 'x' + toString(i)}), \
             (:Ev {i: i, kind: 'int', t: i})",
        );
        // Same instant as datetime i = 3, written in another offset.
        db.run(
            "CREATE (:Ev {i: 100, kind: 'datetime', t: ('2024-01-01T05:00:00+02:00'::DATETIME)})",
        );
        db
    };
    let indexed = seed(true);
    let plain = seed(false);

    let queries = [
        "MATCH (e:Ev) WHERE e.t > ('2024-01-10'::DATE) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t >= ('2024-01-10'::DATE) AND e.t < ('2024-01-20'::DATE) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t <= ('2024-01-03'::DATE) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t >= ('2024-01-01T03:00:00Z'::DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t > ('2024-01-01T03:00:00Z'::DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t <= ('2024-01-01T04:00:00+01:00'::DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t < ('2024-01-01T03:00:00Z'::DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t = ('2024-01-01T03:00:00Z'::DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t = ('2024-01-01T05:00:00+02:00'::DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t > ('2024-09-20T00:00:00'::LOCAL_DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t < ('2024-09-10T00:00:00'::LOCAL_DATETIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t >= ('08:30:00+01:00'::TIME) AND e.t < ('09:30:00+02:00'::TIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t > ('08:05:00'::LOCAL_TIME) RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t = ('2024-01-05'::DATE) RETURN e.i AS i",
        "MATCH (e:Ev) WHERE e.t > ('2024-01-10'::DATE) RETURN e.i AS i ORDER BY e.t DESC LIMIT 5",
        "MATCH (e:Ev) WHERE e.t < ('2024-09-20T00:00:00'::LOCAL_DATETIME) RETURN e.i AS i ORDER BY e.t DESC LIMIT 5",
        "MATCH (e:Ev) WHERE e.t > ('08:02:00'::LOCAL_TIME) RETURN e.i AS i ORDER BY e.t LIMIT 5",
        "MATCH (e:Ev) WHERE e.t > ('07:00:00Z'::TIME) RETURN e.i AS i ORDER BY e.t LIMIT 5",
        "MATCH (e:Ev) WHERE e.t > 'x2' RETURN e.i AS i ORDER BY i",
        "MATCH (e:Ev) WHERE e.t > 25 RETURN e.i AS i ORDER BY i",
        // A duration bound has no index image: the scan answers it.
        "MATCH (e:Ev) WHERE e.t > ('P1D'::DURATION) RETURN e.i AS i ORDER BY i",
    ];
    for q in queries {
        let got = indexed.run(q);
        let want = plain.run(q);
        assert_eq!(got, want, "{q}");
    }

    let got = ints(
        &indexed.run("MATCH (e:Ev) WHERE e.t >= ('2024-01-01T03:00:00Z'::DATETIME) AND e.t <= ('2024-01-01T03:00:00Z'::DATETIME) RETURN e.i AS i ORDER BY i"),
        "i",
    );
    assert_eq!(got, vec![3, 100], "same instant in two offsets");
    let got = ints(
        &indexed.run("MATCH (e:Ev) WHERE e.t = ('2024-01-01T03:00:00Z'::DATETIME) RETURN e.i AS i"),
        "i",
    );
    assert_eq!(got, vec![3], "equality is exact, offset included");
    let got = ints(
        &indexed.run("MATCH (e:Ev) WHERE e.t > ('2024-01-27'::DATE) RETURN e.i AS i ORDER BY i"),
        "i",
    );
    assert_eq!(got, vec![27, 28, 29]);
}

#[test]
fn index_tracks_temporal_updates_and_deletes() {
    let db = TestDb::new();
    db.run("CREATE INDEX d FOR (n:N) ON (n.d)");
    db.run("UNWIND range(1, 5) AS i CREATE (:N {i: i, d: ('2024-01-01'::DATE) + ({days: i}::DURATION)})");
    let q = "MATCH (n:N) WHERE n.d > ('2024-01-03'::DATE) RETURN n.i AS i ORDER BY i";
    assert_eq!(ints(&db.run(q), "i"), vec![3, 4, 5]);
    db.run("MATCH (n:N {i: 1}) SET n.d = ('2025-01-01'::DATE)");
    db.run("MATCH (n:N {i: 5}) SET n.d = ('2020-01-01'::DATE)");
    db.run("MATCH (n:N {i: 4}) DELETE n");
    db.run("MATCH (n:N {i: 3}) REMOVE n.d");
    assert_eq!(ints(&db.run(q), "i"), vec![1]);
}

#[test]
fn comparisons_and_ordering_on_every_temporal_kind() {
    let db = TestDb::new();
    for (q, want) in [
        (
            "RETURN ('2024-10-01T00:00'::LOCAL_DATETIME) > ('2024-09-01T00:00'::LOCAL_DATETIME) AS r",
            true,
        ),
        ("RETURN ('10:00'::LOCAL_TIME) < ('09:00'::LOCAL_TIME) AS r", false),
        ("RETURN ('08:00'::LOCAL_TIME) < ('09:00'::LOCAL_TIME) AS r", true),
        // 10:00+02:00 is 08:00 UTC.
        ("RETURN ('10:00+02:00'::TIME) < ('09:00Z'::TIME) AS r", true),
        (
            "RETURN ('2024-01-01T00:00:00.000000001Z'::DATETIME) > ('2024-01-01T00:00:00Z'::DATETIME) AS r",
            true,
        ),
        (
            "RETURN ('2024-01-01T02:00:00+02:00'::DATETIME) >= ('2024-01-01T00:00:00Z'::DATETIME) AS r",
            true,
        ),
        (
            "RETURN ('2024-01-01T02:00:00+02:00'::DATETIME) > ('2024-01-01T00:00:00Z'::DATETIME) AS r",
            false,
        ),
        ("RETURN ('2024-01-01'::DATE) < ('2024-01-02T00:00:00Z'::DATETIME) AS r", false),
    ] {
        assert_eq!(db.run(q), vec![json!({ "r": want })], "{q}");
    }

    let rows = db.run(
        "UNWIND [('2024-10-01T00:00'::LOCAL_DATETIME), ('2024-09-01T00:00'::LOCAL_DATETIME), \
         ('0999-01-01T00:00'::LOCAL_DATETIME)] AS t WITH t ORDER BY t RETURN toString(t) AS t",
    );
    let order: Vec<&str> = rows.iter().map(|r| r["t"].as_str().unwrap()).collect();
    assert_eq!(
        order,
        vec![
            "0999-01-01T00:00:00",
            "2024-09-01T00:00:00",
            "2024-10-01T00:00:00"
        ]
    );
}
