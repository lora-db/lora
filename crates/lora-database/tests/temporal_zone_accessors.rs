//! `dt.timezone`, `dt.offset`, `dt.offsetMinutes`, `dt.offsetSeconds`,
//! `dt.epochSeconds` and `dt.epochMillis` read a DATETIME's zone, offset
//! and instant (and the offset fields a TIME's). They returned null.

mod test_helpers;
use serde_json::json;
use test_helpers::TestDb;

#[test]
fn datetime_zone_offset_and_instant() {
    let db = TestDb::new();
    let zoned = "datetime('2026-07-01T12:00:00.123+02:00[Europe/Amsterdam]')";
    let fixed = "datetime('2026-07-01T12:00:00-05:30')";
    let utc = "datetime('1970-01-01T00:00:01Z')";
    for (q, want) in [
        (format!("{zoned}.timezone"), json!("Europe/Amsterdam")),
        (format!("{zoned}.offset"), json!("+02:00")),
        (format!("{zoned}.offsetMinutes"), json!(120)),
        (format!("{zoned}.offsetSeconds"), json!(7200)),
        (format!("{zoned}.epochSeconds"), json!(1_782_900_000)),
        (format!("{zoned}.epochMillis"), json!(1_782_900_000_123i64)),
        (format!("{fixed}.timezone"), json!("-05:30")),
        (format!("{fixed}.offset"), json!("-05:30")),
        (format!("{fixed}.offsetMinutes"), json!(-330)),
        (format!("{utc}.timezone"), json!("Z")),
        (format!("{utc}.offset"), json!("Z")),
        (format!("{utc}.epochSeconds"), json!(1)),
        (format!("{utc}.epochMillis"), json!(1000)),
        ("time('12:00+01:00').timezone".to_string(), json!("+01:00")),
        ("time('12:00+01:00').offset".to_string(), json!("+01:00")),
        ("time('12:00+01:00').offsetSeconds".to_string(), json!(3600)),
        ("time('12:00-00:30').offsetMinutes".to_string(), json!(-30)),
    ] {
        assert_eq!(db.scalar(&format!("RETURN {q}")), want, "{q}");
    }
}
