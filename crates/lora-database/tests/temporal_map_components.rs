//! Temporal constructor maps take every component Neo4j accepts, and
//! refuse the ones they do not know.
//!
//! `date()` and `duration()` read only `year/month/day` and
//! `years/months/days/hours/minutes/seconds` (integers only) and ignored
//! every other key, so `date({year: 1984, week: 10, dayOfWeek: 3})` was
//! 1984-01-01 and `duration({hours: 1.5})` was `P0D`. A map now builds
//! ISO week, ordinal and quarter dates, takes fractional duration
//! components (cascading down like Neo4j) and the missing duration keys,
//! and any other key, or a component without the larger ones it needs,
//! is an error that names it.

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

fn assert_errors(cases: &[(&str, &str)]) {
    let db = TestDb::new();
    for (q, needle) in cases {
        let err = db.run_err(&format!("RETURN {q}"));
        assert!(err.contains(needle), "{q}: {err}");
    }
}

#[test]
fn iso_week_dates() {
    assert_strings(&[
        ("date({year: 1984, week: 10, dayOfWeek: 3})", "1984-03-07"),
        ("date({year: 1984, week: 10})", "1984-03-05"),
        // Week 1 of 2015 starts in 2014; `year` is the week-based year.
        ("date({year: 2015, week: 1, dayOfWeek: 1})", "2014-12-29"),
        ("date({year: 2020, week: 53, dayOfWeek: 7})", "2021-01-03"),
    ]);
    assert_errors(&[
        // 2021 has 52 ISO weeks.
        ("date({year: 2021, week: 53})", "week"),
        ("date({year: 2021, week: 1, dayOfWeek: 8})", "dayOfWeek"),
    ]);
}

#[test]
fn ordinal_and_quarter_dates() {
    assert_strings(&[
        ("date({year: 1984, ordinalDay: 202})", "1984-07-20"),
        (
            "date({year: 1984, quarter: 3, dayOfQuarter: 45})",
            "1984-08-14",
        ),
        ("date({year: 1984, quarter: 2})", "1984-04-01"),
    ]);
    assert_errors(&[
        ("date({year: 1983, ordinalDay: 366})", "ordinalDay"),
        // Q1 1984 has 91 days.
        (
            "date({year: 1984, quarter: 1, dayOfQuarter: 92})",
            "dayOfQuarter",
        ),
        ("date({year: 1984, quarter: 5})", "quarter"),
    ]);
}

#[test]
fn datetime_maps_take_the_same_date_forms() {
    assert_strings(&[
        (
            "datetime({year: 1984, week: 10, dayOfWeek: 3, hour: 12, timezone: '+01:00'})",
            "1984-03-07T12:00:00+01:00",
        ),
        (
            "localdatetime({year: 1984, ordinalDay: 202, hour: 12, minute: 31})",
            "1984-07-20T12:31:00",
        ),
        (
            "localdatetime({year: 1984, quarter: 3, dayOfQuarter: 45, hour: 1})",
            "1984-08-14T01:00:00",
        ),
    ]);
}

#[test]
fn maps_can_start_from_another_temporal() {
    assert_strings(&[
        ("date({date: date('1984-10-11'), day: 28})", "1984-10-28"),
        (
            "date({date: date('1984-10-11'), week: 1, dayOfWeek: 1})",
            "1984-01-02",
        ),
        (
            "localdatetime({date: date('1984-10-11'), time: localtime('12:31:14')})",
            "1984-10-11T12:31:14",
        ),
        (
            "datetime({datetime: localdatetime('1984-10-11T12:31'), timezone: '+01:00'})",
            "1984-10-11T12:31:00+01:00",
        ),
        // A zoned value moved to another zone keeps its instant.
        (
            "datetime({datetime: datetime('1984-10-11T12:00Z'), timezone: '+01:00'})",
            "1984-10-11T13:00:00+01:00",
        ),
        (
            "time({time: time('12:00+01:00'), timezone: '+02:00'})",
            "13:00:00+02:00",
        ),
        (
            "localtime({time: localtime('12:31:14'), second: 0})",
            "12:31:00",
        ),
    ]);
}

#[test]
fn unknown_and_misplaced_keys_are_named() {
    assert_errors(&[
        ("date({year: 1984, foo: 1})", "`foo`"),
        ("date({year: 1984, hour: 1})", "`hour`"),
        ("date({year: 1984, weekYear: 1984})", "`weekYear`"),
        ("localtime({hour: 1, timezone: 'Z'})", "`timezone`"),
        ("localdatetime({year: 1984, timezone: 'Z'})", "`timezone`"),
        ("time({hour: 1, year: 1984})", "`year`"),
        ("date({year: 1984, month: 3, week: 10})", "`week`"),
        ("date({year: 1984, ordinalDay: 3, day: 3})", "`day`"),
        ("date({year: 1984.5})", "`year`"),
        ("duration({hours: 1, foo: 2})", "`foo`"),
        ("duration({hours: 'x'})", "`hours`"),
    ]);
}

#[test]
fn smaller_components_need_the_larger_ones() {
    assert_errors(&[
        ("date({year: 1984, day: 3})", "`month`"),
        ("date({year: 1984, dayOfWeek: 3})", "`week`"),
        ("date({year: 1984, dayOfQuarter: 3})", "`quarter`"),
        ("date({month: 3})", "`year`"),
        ("time({minute: 30})", "`hour`"),
        ("localtime({hour: 1, second: 5})", "`minute`"),
        (
            "localtime({hour: 1, minute: 1, millisecond: 5})",
            "`second`",
        ),
        ("datetime({year: 1984, minute: 5})", "`hour`"),
        ("time({})", "`hour`"),
    ]);
    // Every error is still a failed cast.
    assert_errors(&[("time({minute: 30})", "cannot cast map to TIME")]);
}

#[test]
fn duration_maps_take_every_component() {
    assert_strings(&[
        ("duration({hours: 1.5})", "PT1H30M"),
        ("duration({weeks: 2})", "P14D"),
        ("duration({weeks: 2.5})", "P17DT12H"),
        ("duration({years: 1.5})", "P1Y6M"),
        ("duration({days: 1.5})", "P1DT12H"),
        // A month is 30.436875 days, as in Neo4j.
        ("duration({months: 0.75})", "P22DT19H51M49.5S"),
        ("duration({milliseconds: 1500})", "PT1.5S"),
        ("duration({microseconds: 2})", "PT0.000002S"),
        ("duration({nanoseconds: 5})", "PT0.000000005S"),
        (
            "duration({minutes: 1.5, seconds: 1, milliseconds: 123, microseconds: 456, nanoseconds: 789})",
            "PT1M31.123456789S",
        ),
        ("duration({seconds: -1.5})", "PT-1.5S"),
        ("duration({days: 2, hours: 3})", "P2DT3H"),
    ]);
    let db = TestDb::new();
    assert_eq!(
        db.scalar("RETURN duration({hours: 1.5}) = duration('PT1H30M')"),
        json!(true)
    );
    assert_eq!(
        db.scalar("RETURN toString(date('2026-01-01') + duration({weeks: 1}))"),
        json!("2026-01-08")
    );
}
