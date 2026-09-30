//! The Cypher temporal constructors build the type they are named for
//! (E-1).
//!
//! `date`, `datetime`, `localdatetime`, `time`, `localtime` and `duration`
//! all aliased `temporal.now`, which ignored the name: `date()` returned a
//! DATETIME, `date(datetime())` returned it unchanged, `date({year: …})`
//! returned the current instant and `time('12:00Z')` returned null. And
//! since a DATE and a DATETIME do not order against each other,
//! `WHERE f.startsOn >= date()` silently dropped every row. Ordering two
//! temporals of different kinds is now an error on every plan.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

fn scalar(q: &str) -> serde_json::Value {
    TestDb::new().scalar(q)
}

#[test]
fn zero_argument_constructors_return_their_own_type() {
    for (call, ty) in [
        ("date()", "DATE"),
        ("datetime()", "DATETIME"),
        ("localdatetime()", "LOCAL_DATETIME"),
        ("time()", "TIME"),
        ("localtime()", "LOCAL_TIME"),
    ] {
        assert_eq!(
            scalar(&format!("RETURN type.of({call})")),
            json!(ty),
            "{call}"
        );
    }
    assert_eq!(scalar("RETURN date() = temporal.today()"), json!(true));
    // Case-insensitive, like every builtin name.
    assert_eq!(scalar("RETURN type.of(DATE())"), json!("DATE"));
}

#[test]
fn date_compares_with_dates_as_the_brief_expects() {
    assert_eq!(
        scalar("RETURN date() + duration('P1D') >= date()"),
        json!(true)
    );
    assert_eq!(scalar("RETURN date('1999-01-01') >= date()"), json!(false));
    assert_eq!(
        scalar("RETURN date(datetime()) = date()"),
        json!(true),
        "date(datetime()) is today's date"
    );
    // Festimap's workaround keeps working and means the same thing.
    assert_eq!(
        scalar("RETURN date(substring(toString(datetime()), 0, 10)) = date()"),
        json!(true)
    );
}

#[test]
fn string_arguments_parse_as_the_named_type() {
    for (q, want) in [
        ("RETURN toString(date('2026-10-01'))", "2026-10-01"),
        (
            "RETURN toString(date('2026-10-01T23:30:00+02:00'))",
            "2026-10-01",
        ),
        (
            "RETURN toString(datetime('2026-10-01'))",
            "2026-10-01T00:00:00Z",
        ),
        (
            "RETURN toString(datetime('2026-10-01T12:00:00+02:00'))",
            "2026-10-01T12:00:00+02:00",
        ),
        (
            "RETURN toString(localdatetime('2026-10-01T12:00'))",
            "2026-10-01T12:00:00",
        ),
        (
            "RETURN toString(localdatetime('2026-10-01'))",
            "2026-10-01T00:00:00",
        ),
        ("RETURN toString(time('12:00Z'))", "12:00:00Z"),
        ("RETURN toString(time('12:00+01:00'))", "12:00:00+01:00"),
        ("RETURN toString(localtime('12:00'))", "12:00:00"),
        ("RETURN toString(duration('P1DT2H'))", "P1DT2H"),
    ] {
        assert_eq!(scalar(q), json!(want), "{q}");
    }
}

#[test]
fn temporal_arguments_keep_the_components_the_target_has() {
    for (q, want) in [
        // The date of a datetime in its own offset, not in UTC.
        (
            "RETURN toString(date(datetime('2026-10-01T23:30:00+02:00')))",
            "2026-10-01",
        ),
        (
            "RETURN toString(date(localdatetime('2026-10-01T23:30')))",
            "2026-10-01",
        ),
        (
            "RETURN toString(datetime(date('2026-10-01')))",
            "2026-10-01T00:00:00Z",
        ),
        (
            "RETURN toString(datetime(localdatetime('2026-10-01T08:00')))",
            "2026-10-01T08:00:00Z",
        ),
        (
            "RETURN toString(localdatetime(datetime('2026-10-01T08:00:00+02:00')))",
            "2026-10-01T08:00:00",
        ),
        (
            "RETURN toString(localdatetime(date('2026-10-01')))",
            "2026-10-01T00:00:00",
        ),
        (
            "RETURN toString(time(datetime('2026-10-01T08:00:00+02:00')))",
            "08:00:00+02:00",
        ),
        ("RETURN toString(time(localtime('08:00')))", "08:00:00Z"),
        (
            "RETURN toString(localtime(datetime('2026-10-01T08:00:00+02:00')))",
            "08:00:00",
        ),
        (
            "RETURN toString(localtime(time('08:00+02:00')))",
            "08:00:00",
        ),
    ] {
        assert_eq!(scalar(q), json!(want), "{q}");
    }
}

#[test]
fn map_arguments_give_the_components() {
    for (q, want) in [
        ("RETURN toString(date({year: 2026, month: 2, day: 3}))", "2026-02-03"),
        ("RETURN toString(date({year: 2026}))", "2026-01-01"),
        (
            "RETURN toString(datetime({year: 2026, month: 2, day: 3, hour: 4, minute: 5, timezone: '+01:00'}))",
            "2026-02-03T04:05:00+01:00",
        ),
        (
            "RETURN toString(localdatetime({year: 2026, month: 2, day: 3, hour: 4}))",
            "2026-02-03T04:00:00",
        ),
        (
            "RETURN toString(time({hour: 4, minute: 5, timezone: 'Z'}))",
            "04:05:00Z",
        ),
        (
            "RETURN toString(localtime({hour: 4, minute: 5, second: 6, millisecond: 7}))",
            "04:05:06.007",
        ),
        ("RETURN toString(duration({days: 2, hours: 3}))", "P2DT3H"),
    ] {
        assert_eq!(scalar(q), json!(want), "{q}");
    }
}

#[test]
fn bad_arguments_fail_instead_of_returning_null_or_now() {
    let db = TestDb::new();
    for q in [
        "RETURN date('not a date')",
        "RETURN time('25:00')",
        // A map without a year used to be year 0, and before that "now".
        "RETURN date({month: 2, day: 3})",
        "RETURN date(time('12:00Z'))",
        "RETURN localtime(date('2026-10-01'))",
        // An unknown zone used to read as UTC.
        "RETURN datetime({year: 2026, timezone: 'Mars/Olympus'})",
        "RETURN date(42)",
    ] {
        let err = db.run_err(q);
        assert!(err.contains("cannot cast"), "{q}: {err}");
    }
    for q in ["RETURN date('2026-01-01', 'x')", "RETURN duration()"] {
        let err = db.run_err(q);
        assert!(err.contains("argument"), "{q}: {err}");
    }
    // Null in, null out, as with every Cypher function.
    assert_eq!(db.scalar("RETURN date(null)"), json!(null));
    assert_eq!(db.scalar("RETURN datetime(null)"), json!(null));
}

fn festivals(indexed: bool) -> TestDb {
    let db = TestDb::new();
    if indexed {
        db.run("CREATE INDEX festival_starts FOR (f:Festival) ON (f.startsOn)");
    }
    db.run(
        "CREATE (:Festival {key: 'past', startsOn: date() - duration('P10D')}), \
                (:Festival {key: 'today', startsOn: date()}), \
                (:Festival {key: 'soon', startsOn: date() + duration('P10D')})",
    );
    db
}

#[test]
fn where_on_date_keeps_the_matching_rows_on_both_plans() {
    for indexed in [false, true] {
        let db = festivals(indexed);
        assert_eq!(
            db.sorted_strings(
                "MATCH (f:Festival) WHERE f.startsOn >= date() RETURN f.key AS k",
                "k"
            ),
            vec!["soon", "today"],
            "indexed: {indexed}"
        );
        assert_eq!(
            db.sorted_strings(
                "MATCH (f:Festival) WHERE f.startsOn >= date() RETURN f.key AS k \
                 ORDER BY f.startsOn LIMIT 1",
                "k"
            ),
            vec!["today"],
            "indexed: {indexed}"
        );
    }
}

#[test]
fn comparing_a_date_with_a_datetime_fails_on_every_plan() {
    for indexed in [false, true] {
        let db = festivals(indexed);
        for q in [
            "MATCH (f:Festival) WHERE f.startsOn >= datetime() RETURN f.key AS k",
            "MATCH (f:Festival) WHERE f.startsOn < datetime() RETURN f.key AS k",
            "MATCH (f:Festival) WHERE f.startsOn >= datetime() RETURN f.key AS k \
             ORDER BY f.startsOn LIMIT 1",
            "MATCH (f:Festival) WHERE f.startsOn > date() AND f.startsOn < datetime() \
             RETURN f.key AS k",
            "MATCH (f:Festival {key: 'soon'}) WITH f MATCH (f) \
             WHERE f.startsOn >= datetime() RETURN f.key AS k",
        ] {
            let err = db.run_err(q);
            assert!(
                err.contains("cannot compare") && err.contains("DATETIME"),
                "indexed: {indexed}, {q}: {err}"
            );
        }
    }
}

#[test]
fn comparing_relationship_temporals_of_different_kinds_fails_on_every_plan() {
    for indexed in [false, true] {
        let db = TestDb::new();
        if indexed {
            db.run("CREATE INDEX hired_on FOR ()-[r:HIRED]-() ON (r.on)");
        }
        db.run("CREATE (:P)-[:HIRED {on: date('2020-03-15')}]->(:C)");
        assert_eq!(
            db.run("MATCH ()-[r:HIRED]->() WHERE r.on < date() RETURN count(r) AS n"),
            vec![json!({ "n": 1 })]
        );
        let err = db.run_err("MATCH ()-[r:HIRED]->() WHERE r.on < datetime() RETURN r");
        assert!(err.contains("cannot compare"), "indexed: {indexed}: {err}");
    }
}

#[test]
fn every_pair_of_different_temporal_kinds_fails_to_order() {
    let values = [
        "date('2026-10-01')",
        "datetime('2026-10-01T00:00Z')",
        "localdatetime('2026-10-01T00:00')",
        "time('00:00Z')",
        "localtime('00:00')",
    ];
    let db = TestDb::new();
    for (i, a) in values.iter().enumerate() {
        for (j, b) in values.iter().enumerate() {
            for op in ["<", "<=", ">", ">="] {
                let q = format!("RETURN {a} {op} {b} AS r");
                if i == j {
                    assert!(db.scalar(&q).is_boolean(), "{q}");
                } else {
                    let err = db.run_err(&q);
                    assert!(err.contains("cannot compare"), "{q}: {err}");
                }
            }
            // Equality never fails: different kinds are simply unequal.
            let eq = db.scalar(&format!("RETURN {a} = {b}"));
            assert_eq!(eq, json!(i == j), "{a} = {b}");
        }
    }
    // A temporal against a non-temporal keeps Cypher's null.
    assert_eq!(
        db.scalar("RETURN date('2026-10-01') < '2026-10-02'"),
        json!(null)
    );
    assert_eq!(db.scalar("RETURN date('2026-10-01') < 1"), json!(null));
}

#[test]
fn sorting_and_aggregating_mixed_kinds_still_works() {
    // ORDER BY, min and max use Cypher's total order across types; only
    // the comparison operators refuse to order different kinds.
    let db = TestDb::new();
    db.run("CREATE (:E {t: date('2026-10-01')}), (:E {t: datetime('2026-10-01T00:00Z')})");
    assert_eq!(db.run("MATCH (e:E) RETURN e.t AS t ORDER BY t").len(), 2);
    assert_eq!(
        db.run("MATCH (e:E) RETURN min(e.t) AS a, max(e.t) AS b")
            .len(),
        1
    );
}
