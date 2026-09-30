//! Temporal constructor strings: the ISO 8601 forms Neo4j accepts, and
//! none it refuses.
//!
//! The parsers read only `YYYY-MM-DD` and `HH:MM[:SS[.f]]` with a
//! `±HH:MM` offset, so `date('2015-W30-2')`, `date('2015-202')`,
//! `date('20150721')` and `time('21:40:32.142+0100')` failed. And the
//! local types dropped an offset instead of refusing it:
//! `localtime('12:00+01:00')` was `12:00`.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

fn assert_strings(cases: &[(&str, &str)]) {
    let db = TestDb::new();
    for (q, want) in cases {
        assert_eq!(
            db.scalar(&format!("RETURN toString({q})")),
            json!(want),
            "{q}"
        );
    }
}

fn assert_cast_errors(cases: &[&str]) {
    let db = TestDb::new();
    for q in cases {
        let err = db.run_err(&format!("RETURN {q}"));
        assert!(err.contains("cannot cast"), "{q}: {err}");
    }
}

#[test]
fn local_types_refuse_an_offset() {
    assert_cast_errors(&[
        "localdatetime('2026-10-01T12:00+02:00')",
        "localdatetime('2026-10-01T12:00Z')",
        "localtime('12:00+01:00')",
        "localtime('12:00Z')",
        "localtime('12:00:00.5-05:00')",
    ]);
    // The zoned types keep theirs.
    assert_strings(&[
        ("time('12:00+01:00')", "12:00:00+01:00"),
        (
            "datetime('2026-10-01T12:00+02:00')",
            "2026-10-01T12:00:00+02:00",
        ),
        ("localtime('12:00')", "12:00:00"),
    ]);
}

#[test]
fn every_iso_date_form() {
    assert_strings(&[
        ("date('2015-07-21')", "2015-07-21"),
        ("date('20150721')", "2015-07-21"),
        ("date('2015-07')", "2015-07-01"),
        ("date('201507')", "2015-07-01"),
        ("date('2015')", "2015-01-01"),
        ("date('2015-W30-2')", "2015-07-21"),
        ("date('2015W302')", "2015-07-21"),
        ("date('2015-W30')", "2015-07-20"),
        ("date('2015W30')", "2015-07-20"),
        ("date('2015-202')", "2015-07-21"),
        ("date('2015202')", "2015-07-21"),
        ("date('2015-Q3')", "2015-07-01"),
        ("date('2015-Q3-21')", "2015-07-21"),
        ("date('2015Q321')", "2015-07-21"),
    ]);
    assert_cast_errors(&[
        "date('2015-W54-1')",
        "date('2015-W30-8')",
        "date('2015-366')",
        "date('2015-13')",
        "date('2015-Q5')",
        "date('15-07-21')",
        "date('2015-7-21')",
        "date('2015-07-21x')",
    ]);
}

#[test]
fn every_iso_time_form() {
    assert_strings(&[
        ("time('21:40:32.142+0100')", "21:40:32.142+01:00"),
        ("time('214032.142+0100')", "21:40:32.142+01:00"),
        ("time('214032-0130')", "21:40:32-01:30"),
        ("time('21:40:32+01')", "21:40:32+01:00"),
        ("time('2140Z')", "21:40:00Z"),
        ("time('21Z')", "21:00:00Z"),
        ("localtime('2140')", "21:40:00"),
        ("localtime('21')", "21:00:00"),
        ("localtime('21:40:32.5')", "21:40:32.500"),
    ]);
    assert_cast_errors(&[
        "time('21:4')",
        "time('2140:32')",
        "time('21:40+1')",
        "time('24:00')",
        "localtime('214')",
    ]);
}

#[test]
fn datetimes_combine_any_date_and_time_form() {
    assert_strings(&[
        (
            "datetime('20150721T214032.142+0100')",
            "2015-07-21T21:40:32.142+01:00",
        ),
        (
            "datetime('2015-W30-2T214032.142Z')",
            "2015-07-21T21:40:32.142Z",
        ),
        (
            "datetime('2015-07-21T21:40:32.142-01:30')",
            "2015-07-21T21:40:32.142-01:30",
        ),
        ("localdatetime('2015-202T21:40')", "2015-07-21T21:40:00"),
        ("localdatetime('2015-07T21')", "2015-07-01T21:00:00"),
        ("date('2015-W30-2T21:40Z')", "2015-07-21"),
    ]);
}
