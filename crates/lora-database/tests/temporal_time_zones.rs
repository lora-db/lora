//! Named time zones (`Europe/Amsterdam`) resolve against the IANA
//! database, with daylight saving, and a DATETIME keeps the zone's name:
//! `2026-07-01T12:00:00+02:00[Europe/Amsterdam]`.
//!
//! Zones used to be a fixed table with no daylight saving (Amsterdam was
//! always +01:00) and the name was dropped, leaving only the offset.

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

fn at(zone: &str, date: &str, time: &str) -> String {
    let (y, m, d) = (&date[0..4], &date[5..7], &date[8..10]);
    let (h, mi) = (&time[0..2], &time[3..5]);
    format!(
        "datetime({{year: {y}, month: {}, day: {}, hour: {}, minute: {}, timezone: '{zone}'}})",
        m.trim_start_matches('0'),
        d.trim_start_matches('0'),
        h.trim_start_matches('0').parse::<u32>().unwrap_or(0),
        mi.trim_start_matches('0').parse::<u32>().unwrap_or(0),
    )
}

#[test]
fn summer_and_winter_offsets() {
    assert_strings(&[
        (
            &at("Europe/Amsterdam", "2026-07-01", "12:00"),
            "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
        ),
        (
            &at("Europe/Amsterdam", "2026-01-15", "12:00"),
            "2026-01-15T12:00:00+01:00[Europe/Amsterdam]",
        ),
        (
            &at("Europe/London", "2026-07-01", "12:00"),
            "2026-07-01T12:00:00+01:00[Europe/London]",
        ),
        (
            &at("Europe/London", "2026-01-15", "12:00"),
            "2026-01-15T12:00:00Z[Europe/London]",
        ),
        (
            &at("Asia/Kolkata", "2026-07-01", "12:00"),
            "2026-07-01T12:00:00+05:30[Asia/Kolkata]",
        ),
        (
            &at("Asia/Kolkata", "2026-01-15", "12:00"),
            "2026-01-15T12:00:00+05:30[Asia/Kolkata]",
        ),
        // Zone names match case-insensitively and print canonically.
        (
            &at("europe/amsterdam", "2026-07-01", "12:00"),
            "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
        ),
    ]);
}

#[test]
fn gaps_move_forward_and_overlaps_take_the_earlier_offset() {
    assert_strings(&[
        // 02:00 to 03:00 does not exist on 2026-03-29 in Amsterdam.
        (
            &at("Europe/Amsterdam", "2026-03-29", "02:30"),
            "2026-03-29T03:30:00+02:00[Europe/Amsterdam]",
        ),
        // 02:00 to 03:00 happens twice on 2026-10-25.
        (
            &at("Europe/Amsterdam", "2026-10-25", "02:30"),
            "2026-10-25T02:30:00+02:00[Europe/Amsterdam]",
        ),
        (
            "datetime('2026-10-25T02:30[Europe/Amsterdam]')",
            "2026-10-25T02:30:00+02:00[Europe/Amsterdam]",
        ),
        // An explicit offset picks the second occurrence.
        (
            "datetime('2026-10-25T02:30+01:00[Europe/Amsterdam]')",
            "2026-10-25T02:30:00+01:00[Europe/Amsterdam]",
        ),
        (
            "datetime('2026-03-29T01:30[Europe/London]')",
            "2026-03-29T02:30:00+01:00[Europe/London]",
        ),
    ]);
}

#[test]
fn strings_round_trip() {
    let db = TestDb::new();
    for s in [
        "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
        "2026-01-15T12:00:00Z[Europe/London]",
        "2026-07-01T12:00:00.5+05:30[Asia/Kolkata]",
    ] {
        assert_eq!(
            db.scalar(&format!("RETURN toString(datetime('{s}'))")),
            json!(s.replace(":00.5", ":00.500")),
        );
        assert_eq!(
            db.scalar(&format!(
                "WITH datetime('{s}') AS d RETURN datetime(toString(d)) = d"
            )),
            json!(true),
            "{s}"
        );
    }
    // The zone gives the offset when the string has none.
    assert_strings(&[(
        "datetime('2026-07-01T12:00[Europe/Amsterdam]')",
        "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
    )]);
    for q in [
        // Amsterdam is +02:00 in July.
        "RETURN datetime('2026-07-01T12:00+01:00[Europe/Amsterdam]')",
        "RETURN datetime('2026-07-01T12:00[Mars/Olympus]')",
        "RETURN datetime({year: 2026, timezone: 'Mars/Olympus'})",
        "RETURN localdatetime('2026-07-01T12:00[Europe/Amsterdam]')",
    ] {
        let err = db.run_err(q);
        assert!(err.contains("cannot cast"), "{q}: {err}");
    }
}

#[test]
fn a_new_zone_keeps_the_instant() {
    assert_strings(&[
        (
            "datetime({datetime: datetime('2026-07-01T10:00Z'), timezone: 'Europe/Amsterdam'})",
            "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
        ),
        (
            "datetime({datetime: datetime('2026-07-01T12:00[Europe/Amsterdam]'), timezone: 'Asia/Kolkata'})",
            "2026-07-01T15:30:00+05:30[Asia/Kolkata]",
        ),
        (
            "datetime({datetime: datetime('2026-07-01T12:00[Europe/Amsterdam]'), timezone: '+00:00'})",
            "2026-07-01T10:00:00Z",
        ),
        // A local value is placed in the zone.
        (
            "datetime({datetime: localdatetime('2026-07-01T12:00'), timezone: 'Europe/London'})",
            "2026-07-01T12:00:00+01:00[Europe/London]",
        ),
        (
            "datetime({epochSeconds: 0, timezone: 'Europe/Amsterdam'})",
            "1970-01-01T01:00:00+01:00[Europe/Amsterdam]",
        ),
        // A time takes the zone's offset now; Kolkata has no daylight saving.
        ("time({hour: 12, timezone: 'Asia/Kolkata'})", "12:00:00+05:30"),
    ]);
}

#[test]
fn arithmetic_and_truncation_keep_the_zone() {
    assert_strings(&[
        // A day is a calendar day across the spring jump...
        (
            "datetime('2026-03-28T12:00[Europe/Amsterdam]') + duration('P1D')",
            "2026-03-29T12:00:00+02:00[Europe/Amsterdam]",
        ),
        // ...24 hours is 24 hours of the instant.
        (
            "datetime('2026-03-28T12:00[Europe/Amsterdam]') + duration('PT24H')",
            "2026-03-29T13:00:00+02:00[Europe/Amsterdam]",
        ),
        (
            "datetime('2026-10-26T12:00[Europe/Amsterdam]') - duration('P1D')",
            "2026-10-25T12:00:00+01:00[Europe/Amsterdam]",
        ),
        (
            "datetime.truncate('day', datetime('2026-03-29T12:00[Europe/Amsterdam]'))",
            "2026-03-29T00:00:00+01:00[Europe/Amsterdam]",
        ),
        (
            "datetime.truncate('month', datetime('2026-07-15T12:00[Europe/London]'))",
            "2026-07-01T00:00:00+01:00[Europe/London]",
        ),
        (
            "datetime.truncate('day', datetime('2026-07-15T12:00+02:00'), {timezone: 'Europe/London'})",
            "2026-07-15T00:00:00+01:00[Europe/London]",
        ),
    ]);
    let db = TestDb::new();
    assert_eq!(
        db.scalar(
            "RETURN duration.between(datetime('2026-03-28T12:00[Europe/Amsterdam]'), \
             datetime('2026-03-29T12:00[Europe/Amsterdam]')) = duration('PT23H')"
        ),
        json!(true)
    );
}

#[test]
fn stored_zoned_values_compare_by_instant_and_match_by_zone() {
    for indexed in [false, true] {
        let db = TestDb::new();
        if indexed {
            db.run("CREATE INDEX e_at FOR (e:E) ON (e.at)");
        }
        db.run(
            "CREATE (:E {k: 'zoned', at: datetime('2026-07-01T12:00[Europe/Amsterdam]')}), \
                    (:E {k: 'fixed', at: datetime('2026-07-01T12:00+02:00')}), \
                    (:E {k: 'later', at: datetime('2026-07-01T13:00[Europe/Amsterdam]')})",
        );
        assert_eq!(
            db.run("MATCH (e:E {k: 'zoned'}) RETURN toString(e.at) AS s"),
            vec![json!({ "s": "2026-07-01T12:00:00+02:00[Europe/Amsterdam]" })],
            "indexed: {indexed}"
        );
        // Same instant and offset, different zone: not equal.
        assert_eq!(
            db.sorted_strings(
                "MATCH (e:E) WHERE e.at = datetime('2026-07-01T12:00[Europe/Amsterdam]') \
                 RETURN e.k AS k",
                "k"
            ),
            vec!["zoned"],
            "indexed: {indexed}"
        );
        // Ranges order by instant, whatever the zone.
        assert_eq!(
            db.sorted_strings(
                "MATCH (e:E) WHERE e.at >= datetime('2026-07-01T10:00Z') \
                 AND e.at < datetime('2026-07-01T11:00Z') RETURN e.k AS k",
                "k"
            ),
            vec!["fixed", "zoned"],
            "indexed: {indexed}"
        );
    }
}
