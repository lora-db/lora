//! `AND` and `OR` short-circuit: when the left side decides the result,
//! the right side is not evaluated, so its errors cannot fail the query.
//!
//! Both sides were evaluated before the operator combined them, so
//! `false AND date() < datetime()` failed on the cross-kind comparison
//! instead of being `false`, and a guard such as
//! `type.of(x) = 'DATE' AND x >= date(…)` could not protect the
//! comparison from values of another kind.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

#[test]
fn a_deciding_left_side_skips_the_right() {
    let db = TestDb::new();
    for (q, want) in [
        ("RETURN false AND date() < datetime() AS r", json!(false)),
        ("RETURN true OR date() < datetime() AS r", json!(true)),
        ("RETURN (1 = 2) AND 1 / 0 = 1 AS r", json!(false)),
        (
            "RETURN (1 = 1) OR toInteger('x' + date()) = 1 AS r",
            json!(true),
        ),
    ] {
        assert_eq!(db.scalar(q), want, "{q}");
    }
}

#[test]
fn an_undecided_left_side_evaluates_the_right() {
    let db = TestDb::new();
    // Three-valued logic is unchanged.
    for (q, want) in [
        ("RETURN null AND false AS r", json!(false)),
        ("RETURN null AND true AS r", json!(null)),
        ("RETURN null OR true AS r", json!(true)),
        ("RETURN null OR false AS r", json!(null)),
        ("RETURN true AND null AS r", json!(null)),
        ("RETURN false OR null AS r", json!(null)),
        ("RETURN true XOR false AS r", json!(true)),
        ("RETURN null XOR true AS r", json!(null)),
    ] {
        assert_eq!(db.scalar(q), want, "{q}");
    }
    // When the left side does not decide, the right side's error stands.
    for q in [
        "RETURN true AND date() < datetime() AS r",
        "RETURN false OR date() < datetime() AS r",
        "RETURN null AND date() < datetime() AS r",
        "RETURN null OR date() < datetime() AS r",
        "RETURN false XOR date() < datetime() AS r",
    ] {
        let err = db.run_err(q);
        assert!(err.contains("cannot compare"), "{q}: {err}");
    }
}

#[test]
fn a_type_guard_protects_a_comparison() {
    let db = TestDb::new();
    db.run(
        "CREATE (:E {k: 'date', x: date('2024-01-01')}), \
                (:E {k: 'datetime', x: datetime('2024-01-01T00:00Z')}), \
                (:E {k: 'old', x: date('2010-01-01')})",
    );
    assert_eq!(
        db.sorted_strings(
            "MATCH (e:E) WHERE type.of(e.x) = 'DATE' AND e.x >= date('2020-01-01') \
             RETURN e.k AS k",
            "k"
        ),
        vec!["date"]
    );
    assert_eq!(
        db.sorted_strings(
            "MATCH (e:E) WHERE type.of(e.x) <> 'DATE' OR e.x >= date('2020-01-01') \
             RETURN e.k AS k",
            "k"
        ),
        vec!["date", "datetime"]
    );
    // Without the guard the comparison still fails.
    let err = db.run_err("MATCH (e:E) WHERE e.x >= date('2020-01-01') RETURN e.k AS k");
    assert!(err.contains("cannot compare"), "{err}");
}
