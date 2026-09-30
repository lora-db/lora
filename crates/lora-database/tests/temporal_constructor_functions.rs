//! The namespaced Cypher temporal functions: `<type>.transaction()`,
//! `.statement()`, `.realtime()`, `<type>.truncate(unit, value[, map])`,
//! `datetime.fromepoch*` and `duration.between / inMonths / inDays /
//! inSeconds`. They were all unknown functions.

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
fn clock_functions_return_the_named_type() {
    let db = TestDb::new();
    for (ty, want) in [
        ("date", "DATE"),
        ("time", "TIME"),
        ("localtime", "LOCAL_TIME"),
        ("datetime", "DATETIME"),
        ("localdatetime", "LOCAL_DATETIME"),
    ] {
        for clock in ["transaction", "statement", "realtime"] {
            assert_eq!(
                db.scalar(&format!("RETURN type.of({ty}.{clock}())")),
                json!(want),
                "{ty}.{clock}()"
            );
        }
    }
    assert_eq!(db.scalar("RETURN date.transaction() = date()"), json!(true));
    assert!(db
        .run_err("RETURN date.statement(1, 2)")
        .contains("argument"));
}

#[test]
fn truncate_to_every_unit() {
    let d = "datetime('2017-11-11T12:31:14.645876123+01:00')";
    assert_strings(&[
        (&format!("date.truncate('millennium', {d})"), "2000-01-01"),
        (&format!("date.truncate('century', {d})"), "2000-01-01"),
        (&format!("date.truncate('decade', {d})"), "2010-01-01"),
        (&format!("date.truncate('year', {d})"), "2017-01-01"),
        (&format!("date.truncate('weekYear', {d})"), "2017-01-02"),
        (&format!("date.truncate('quarter', {d})"), "2017-10-01"),
        (&format!("date.truncate('month', {d})"), "2017-11-01"),
        (&format!("date.truncate('week', {d})"), "2017-11-06"),
        (&format!("date.truncate('day', {d})"), "2017-11-11"),
        (
            &format!("datetime.truncate('hour', {d})"),
            "2017-11-11T12:00:00+01:00",
        ),
        (
            &format!("datetime.truncate('minute', {d})"),
            "2017-11-11T12:31:00+01:00",
        ),
        (
            &format!("datetime.truncate('second', {d})"),
            "2017-11-11T12:31:14+01:00",
        ),
        (
            &format!("datetime.truncate('millisecond', {d})"),
            "2017-11-11T12:31:14.645+01:00",
        ),
        (
            &format!("datetime.truncate('microsecond', {d})"),
            "2017-11-11T12:31:14.645876000+01:00",
        ),
        (
            &format!("localdatetime.truncate('day', {d})"),
            "2017-11-11T00:00:00",
        ),
        (&format!("time.truncate('hour', {d})"), "12:00:00+01:00"),
        (&format!("localtime.truncate('minute', {d})"), "12:31:00"),
        (&format!("localtime.truncate('day', {d})"), "00:00:00"),
        ("date.truncate('month', date('2017-11-11'))", "2017-11-01"),
        (
            "datetime.truncate('year', date('2017-11-11'))",
            "2017-01-01T00:00:00Z",
        ),
    ]);
}

#[test]
fn truncate_then_apply_the_map() {
    let d = "datetime('2017-11-11T12:31:14.645876123+01:00')";
    assert_strings(&[
        (
            &format!("date.truncate('week', {d}, {{dayOfWeek: 2}})"),
            "2017-11-07",
        ),
        (
            &format!("localdatetime.truncate('weekYear', {d}, {{day: 5}})"),
            "2017-01-05T00:00:00",
        ),
        (
            &format!("datetime.truncate('day', {d}, {{hour: 3, timezone: '+02:00'}})"),
            // The zone is replaced, keeping the local time, as in Neo4j.
            "2017-11-11T03:00:00+02:00",
        ),
        (&format!("date.truncate('month', {d}, null)"), "2017-11-01"),
    ]);
}

#[test]
fn truncate_refuses_units_and_values_that_do_not_fit() {
    assert_errors(&[
        ("date.truncate('hour', date('2017-11-11'))", "hour"),
        ("date.truncate('parsec', date('2017-11-11'))", "parsec"),
        ("time.truncate('month', time('12:00Z'))", "month"),
        ("date.truncate('day', localtime('12:00'))", "LOCAL_TIME"),
        (
            "date.truncate('month', date('2017-11-11'), {hour: 1})",
            "`hour`",
        ),
    ]);
}

#[test]
fn temporal_truncate_gains_the_units() {
    assert_strings(&[
        (
            "temporal.truncate('week', date('2017-11-11'))",
            "2017-11-06",
        ),
        (
            "temporal.truncate('quarter', date('2017-11-11'))",
            "2017-10-01",
        ),
        (
            "temporal.truncate(datetime('2017-11-11T12:31Z'), 'minute')",
            "2017-11-11T12:31:00Z",
        ),
        (
            "temporal.truncate('hour', localdatetime('2017-11-11T12:31'))",
            "2017-11-11T12:00:00",
        ),
    ]);
}

#[test]
fn datetime_from_epoch() {
    assert_strings(&[
        (
            "datetime.fromepoch(1683000000, 123456789)",
            "2023-05-02T04:00:00.123456789Z",
        ),
        (
            "datetime.fromepochmillis(1724198400000)",
            "2024-08-21T00:00:00Z",
        ),
        ("datetime.fromEpochMillis(-1)", "1969-12-31T23:59:59.999Z"),
        (
            "datetime({epochSeconds: 1683000000, nanosecond: 5})",
            "2023-05-02T04:00:00.000000005Z",
        ),
        (
            "datetime({epochMillis: 1724198400000})",
            "2024-08-21T00:00:00Z",
        ),
        (
            "datetime({epochSeconds: 0, timezone: '+01:00'})",
            "1970-01-01T01:00:00+01:00",
        ),
    ]);
    assert_errors(&[
        ("datetime({epochSeconds: 1, year: 1984})", "`year`"),
        (
            "datetime({epochSeconds: 1, epochMillis: 1})",
            "`epochMillis`",
        ),
        ("localdatetime({epochSeconds: 1})", "`epochSeconds`"),
    ]);
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN datetime.fromepoch(null, 0)"), json!(null));
}

#[test]
fn duration_between_and_in_units() {
    assert_strings(&[
        (
            "duration.between(date('1984-10-11'), date('1985-11-25'))",
            "P1Y1M14D",
        ),
        (
            "duration.between(date('1985-11-25'), date('1984-10-11'))",
            "P-1Y-1M-14D",
        ),
        (
            "duration.between(date('1984-10-11'), datetime('1984-10-12T21:40:32.142+01:00'))",
            "P1DT21H40M32.142S",
        ),
        (
            "duration.between(datetime('2015-07-21T21:40:32.142+01:00'), localdatetime('2016-07-21T21:45:22.142'))",
            "P1YT4M50S",
        ),
        (
            "duration.between(localtime('12:00'), localtime('14:30'))",
            "PT2H30M",
        ),
        (
            "duration.between(time('12:00+01:00'), time('12:00Z'))",
            "PT1H",
        ),
        (
            "duration.inMonths(date('1984-10-11'), date('1985-11-25'))",
            "P1Y1M",
        ),
        (
            "duration.inDays(date('1984-10-11'), date('1985-11-25'))",
            "P410D",
        ),
        (
            "duration.inDays(date('1985-11-25'), date('1984-10-11'))",
            "P-410D",
        ),
        (
            "duration.inDays(date('1984-10-11'), datetime('1984-10-12T21:40:32.142+01:00'))",
            "P1D",
        ),
        (
            "duration.inSeconds(date('1984-10-11'), datetime('1984-10-12T01:00:32.142+01:00'))",
            "PT25H32.142S",
        ),
        (
            "duration.inMonths(localtime('12:00'), localtime('14:30'))",
            "P0D",
        ),
    ]);
    let db = TestDb::new();
    assert_eq!(
        db.scalar("RETURN duration.between(null, date('2020-01-01'))"),
        json!(null)
    );
    // The lora spelling keeps its days-only DATE result.
    assert_eq!(
        db.scalar("RETURN toString(temporal.in_days(date('2024-01-01'), date('2024-04-10')))"),
        json!("P100D")
    );
}
