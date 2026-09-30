//! A RANGE-index scan with a temporal bound hands the values of another
//! temporal kind to the filter above it, so they are judged exactly as an
//! unindexed scan judges them.
//!
//! The index keeps each temporal kind apart, so a range scan never meets
//! a DATETIME when the bound is a DATE. It used to fail up front whenever
//! the label held another kind, even when the rest of the WHERE (a
//! `type.of(x) = 'DATE'` guard) would have dropped those rows first, and
//! without a usable index it rescanned the whole label for that check on
//! every input row. Now the index lists the other-kind ids directly (its
//! kinds are contiguous runs) and the filter decides.

mod test_helpers;
use test_helpers::TestDb;

fn events(indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE INDEX e_x FOR (e:E) ON (e.x)");
        db.run("CREATE INDEX r_x FOR ()-[r:R]-() ON (r.x)");
    }
    db.run(
        "CREATE (:E:Tagged {k: 'date', x: date('2024-01-01')}), \
                (:E {k: 'datetime', x: datetime('2024-01-01T00:00Z')}), \
                (:E {k: 'old', x: date('2010-01-01')}), \
                (:E {k: 'number', x: 5})",
    );
    db.run(
        "CREATE (:P)-[:R {k: 'date', x: date('2024-01-01')}]->(:C), \
                (:P)-[:R {k: 'datetime', x: datetime('2024-01-01T00:00Z')}]->(:C)",
    );
    db
}

#[test]
fn a_guard_protects_the_comparison_on_every_plan() {
    for indexed in [false, true] {
        let db = events(indexed);
        for q in [
            "MATCH (e:E) WHERE type.of(e.x) = 'DATE' AND e.x >= date('2020-01-01') \
             RETURN e.k AS k",
            "MATCH (e:E) WHERE type.of(e.x) = 'DATE' AND e.x >= date('2020-01-01') \
             RETURN e.k AS k ORDER BY e.x LIMIT 1",
            "MATCH (e:E:Tagged) WHERE type.of(e.x) = 'DATE' AND e.x >= date('2020-01-01') \
             RETURN e.k AS k",
            "MATCH ()-[r:R]->() WHERE type.of(r.x) = 'DATE' AND r.x >= date('2020-01-01') \
             RETURN r.k AS k",
            "MATCH (e:E {k: 'datetime'}) WITH e MATCH (e) \
             WHERE type.of(e.x) = 'DATE' AND e.x >= date('2020-01-01') RETURN e.k AS k",
        ] {
            let want: Vec<&str> = if q.contains("'datetime'") {
                vec![]
            } else {
                vec!["date"]
            };
            assert_eq!(db.sorted_strings(q, "k"), want, "indexed: {indexed}: {q}");
        }
    }
}

#[test]
fn an_unguarded_comparison_still_fails_on_every_plan() {
    for indexed in [false, true] {
        let db = events(indexed);
        for q in [
            "MATCH (e:E) WHERE e.x >= date('2020-01-01') RETURN e.k AS k",
            "MATCH (e:E) WHERE e.x >= date('2020-01-01') RETURN e.k AS k ORDER BY e.x LIMIT 1",
            "MATCH (e:E) WHERE e.x < datetime('2030-01-01T00:00Z') RETURN e.k AS k",
            "MATCH ()-[r:R]->() WHERE r.x >= date('2020-01-01') RETURN r.k AS k",
            "MATCH (e:E {k: 'datetime'}) WITH e MATCH (e) \
             WHERE e.x >= date('2020-01-01') RETURN e.k AS k",
        ] {
            let err = db.run_err(q);
            assert!(
                err.contains("cannot compare"),
                "indexed: {indexed}: {q}: {err}"
            );
        }
        // Non-temporal bounds and values keep Cypher's null: no error.
        assert_eq!(
            db.sorted_strings("MATCH (e:E) WHERE e.x > 1 RETURN e.k AS k", "k"),
            vec!["number"],
            "indexed: {indexed}"
        );
    }
}
