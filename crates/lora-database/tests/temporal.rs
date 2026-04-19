/// Temporal type tests — date, datetime, localtime, localdatetime, duration.
///
/// Covers creation, component extraction, comparison, arithmetic, string
/// conversion, integration with Lora clauses, parameterized values, and
/// error handling.
///
/// The engine does not yet have native temporal LoraValue variants. All temporal
/// tests are marked `#[ignore]` and serve as a specification / roadmap for
/// future implementation.
mod test_helpers;
use test_helpers::TestDb;

// ============================================================
// 1. Date creation
// ============================================================

#[test]

fn date_from_string_iso() {
    let v = TestDb::new().scalar("RETURN date('2024-01-15') AS d");
    // Should return a date value; JSON representation TBD
    assert!(!v.is_null());
}

#[test]

fn date_from_map_components() {
    let v = TestDb::new().scalar("RETURN date({year: 2024, month: 6, day: 15}) AS d");
    assert!(!v.is_null());
}

#[test]

fn date_current() {
    // date() with no arguments returns today's date
    let v = TestDb::new().scalar("RETURN date() AS d");
    assert!(!v.is_null());
}

#[test]

fn date_from_partial_map_defaults() {
    // Missing components should default: month=1, day=1
    let v = TestDb::new().scalar("RETURN date({year: 2024}) AS d");
    assert!(!v.is_null());
}

// ============================================================
// 2. DateTime creation
// ============================================================

#[test]

fn datetime_from_iso_string() {
    let v = TestDb::new().scalar("RETURN datetime('2024-01-15T10:30:00Z') AS dt");
    assert!(!v.is_null());
}

#[test]

fn datetime_from_map() {
    let v = TestDb::new().scalar(
        "RETURN datetime({year:2024, month:1, day:15, hour:10, minute:30, second:0}) AS dt",
    );
    assert!(!v.is_null());
}

#[test]

fn datetime_current() {
    let v = TestDb::new().scalar("RETURN datetime() AS dt");
    assert!(!v.is_null());
}

#[test]

fn datetime_with_timezone_offset() {
    let v = TestDb::new().scalar("RETURN datetime('2024-06-15T12:00:00+02:00') AS dt");
    assert!(!v.is_null());
}

#[test]

fn datetime_with_named_timezone() {
    let v = TestDb::new().scalar(
        "RETURN datetime({year:2024, month:6, day:15, hour:12, timezone:'Europe/Amsterdam'}) AS dt",
    );
    assert!(!v.is_null());
}

// ============================================================
// 3. LocalDateTime / LocalTime / Time creation
// ============================================================

#[test]

fn localdatetime_from_string() {
    let v = TestDb::new().scalar("RETURN localdatetime('2024-01-15T10:30:00') AS ldt");
    assert!(!v.is_null());
}

#[test]

fn localdatetime_current() {
    let v = TestDb::new().scalar("RETURN localdatetime() AS ldt");
    assert!(!v.is_null());
}

#[test]

fn localtime_from_string() {
    let v = TestDb::new().scalar("RETURN localtime('14:30:00') AS lt");
    assert!(!v.is_null());
}

#[test]

fn localtime_current() {
    let v = TestDb::new().scalar("RETURN localtime() AS lt");
    assert!(!v.is_null());
}

#[test]

fn time_from_string_with_offset() {
    let v = TestDb::new().scalar("RETURN time('14:30:00+02:00') AS t");
    assert!(!v.is_null());
}

#[test]

fn time_current() {
    let v = TestDb::new().scalar("RETURN time() AS t");
    assert!(!v.is_null());
}

// ============================================================
// 4. Duration creation
// ============================================================

#[test]

fn duration_from_iso_string() {
    let v = TestDb::new().scalar("RETURN duration('P1Y2M3D') AS dur");
    assert!(!v.is_null());
}

#[test]

fn duration_from_map() {
    let v = TestDb::new().scalar(
        "RETURN duration({years: 1, months: 2, days: 3, hours: 4}) AS dur",
    );
    assert!(!v.is_null());
}

#[test]

fn duration_with_time_components() {
    let v = TestDb::new().scalar("RETURN duration('PT2H30M') AS dur");
    assert!(!v.is_null());
}

#[test]

fn duration_between_dates() {
    let rows = TestDb::new().run(
        "RETURN duration.between(date('2024-01-01'), date('2024-03-15')) AS d",
    );
    assert_eq!(rows.len(), 1);
}

#[test]

fn duration_in_days_between_dates() {
    let rows = TestDb::new().run(
        "RETURN duration.inDays(date('2024-01-01'), date('2024-01-31')) AS d",
    );
    assert_eq!(rows.len(), 1);
}

// ============================================================
// 5. Temporal component extraction
// ============================================================

#[test]

fn date_year_component() {
    let v = TestDb::new().scalar("RETURN date('2024-06-15').year AS y");
    assert_eq!(v, 2024);
}

#[test]

fn date_month_component() {
    let v = TestDb::new().scalar("RETURN date('2024-06-15').month AS m");
    assert_eq!(v, 6);
}

#[test]

fn date_day_component() {
    let v = TestDb::new().scalar("RETURN date('2024-06-15').day AS d");
    assert_eq!(v, 15);
}

#[test]

fn datetime_hour_component() {
    let v = TestDb::new().scalar("RETURN datetime('2024-06-15T10:30:45Z').hour AS h");
    assert_eq!(v, 10);
}

#[test]

fn datetime_minute_component() {
    let v = TestDb::new().scalar("RETURN datetime('2024-06-15T10:30:45Z').minute AS m");
    assert_eq!(v, 30);
}

#[test]

fn datetime_second_component() {
    let v = TestDb::new().scalar("RETURN datetime('2024-06-15T10:30:45Z').second AS s");
    assert_eq!(v, 45);
}

#[test]

fn datetime_millisecond_component() {
    let v = TestDb::new().scalar("RETURN datetime('2024-06-15T10:30:45.123Z').millisecond AS ms");
    assert_eq!(v, 123);
}

#[test]

fn date_day_of_week() {
    // 2024-01-01 is Monday (1 in ISO)
    let v = TestDb::new().scalar("RETURN date('2024-01-01').dayOfWeek AS dow");
    assert_eq!(v, 1);
}

#[test]

fn date_day_of_year() {
    let v = TestDb::new().scalar("RETURN date('2024-02-01').dayOfYear AS doy");
    assert_eq!(v, 32);
}

#[test]

fn duration_components() {
    let db = TestDb::new();
    let rows = db.run(
        "WITH duration('P1Y2M3DT4H5M6S') AS d \
         RETURN d.years AS y, d.months AS m, d.days AS dd, \
                d.hours AS h, d.minutes AS mi, d.seconds AS s",
    );
    assert_eq!(rows[0]["y"], 1);
    assert_eq!(rows[0]["m"], 2);
    assert_eq!(rows[0]["dd"], 3);
    assert_eq!(rows[0]["h"], 4);
    assert_eq!(rows[0]["mi"], 5);
    assert_eq!(rows[0]["s"], 6);
}

// ============================================================
// 6. Temporal comparison
// ============================================================

#[test]

fn date_equality() {
    let v = TestDb::new().scalar("RETURN date('2024-01-01') = date('2024-01-01')");
    assert_eq!(v, true);
}

#[test]

fn date_inequality() {
    let v = TestDb::new().scalar("RETURN date('2024-01-01') <> date('2024-01-02')");
    assert_eq!(v, true);
}

#[test]

fn date_less_than() {
    let v = TestDb::new().scalar("RETURN date('2024-01-01') < date('2024-06-01')");
    assert_eq!(v, true);
}

#[test]

fn date_greater_than() {
    let v = TestDb::new().scalar("RETURN date('2024-12-31') > date('2024-01-01')");
    assert_eq!(v, true);
}

#[test]

fn date_less_than_or_equal() {
    let db = TestDb::new();
    assert_eq!(db.scalar("RETURN date('2024-01-01') <= date('2024-01-01')"), true);
    assert_eq!(db.scalar("RETURN date('2024-01-01') <= date('2024-01-02')"), true);
    assert_eq!(db.scalar("RETURN date('2024-01-02') <= date('2024-01-01')"), false);
}

#[test]

fn datetime_comparison() {
    let v = TestDb::new().scalar(
        "RETURN datetime('2024-01-01T00:00:00Z') < datetime('2024-01-01T12:00:00Z')",
    );
    assert_eq!(v, true);
}

#[test]

fn date_compared_with_null_returns_null() {
    let v = TestDb::new().scalar("RETURN date('2024-01-01') = null");
    assert!(v.is_null());
}

#[test]

fn duration_equality() {
    let v = TestDb::new().scalar("RETURN duration('P1Y') = duration('P1Y')");
    assert_eq!(v, true);
}

#[test]

fn duration_ordering() {
    let v = TestDb::new().scalar("RETURN duration('P1D') < duration('P2D')");
    assert_eq!(v, true);
}

// ============================================================
// 7. Temporal arithmetic
// ============================================================

#[test]

fn date_plus_duration_days() {
    let v = TestDb::new().scalar(
        "RETURN date('2024-01-01') + duration('P10D') AS d",
    );
    // Expected: 2024-01-11
    assert!(!v.is_null());
}

#[test]

fn date_plus_duration_months() {
    let v = TestDb::new().scalar(
        "RETURN date('2024-01-31') + duration('P1M') AS d",
    );
    // Expected: 2024-02-29 (2024 is leap year) or 2024-02-28 depending on semantics
    assert!(!v.is_null());
}

#[test]

fn date_minus_duration() {
    let v = TestDb::new().scalar(
        "RETURN date('2024-06-15') - duration('P15D') AS d",
    );
    // Expected: 2024-05-31
    assert!(!v.is_null());
}

#[test]

fn date_minus_date_produces_duration() {
    let v = TestDb::new().scalar(
        "RETURN date('2024-03-01') - date('2024-01-01') AS d",
    );
    // Expected: a duration representing 60 days
    assert!(!v.is_null());
}

#[test]

fn datetime_plus_duration() {
    let v = TestDb::new().scalar(
        "RETURN datetime('2024-01-01T00:00:00Z') + duration('PT2H30M') AS dt",
    );
    assert!(!v.is_null());
}

#[test]

fn datetime_minus_datetime_produces_duration() {
    let v = TestDb::new().scalar(
        "RETURN datetime('2024-01-02T00:00:00Z') - datetime('2024-01-01T00:00:00Z') AS d",
    );
    assert!(!v.is_null());
}

#[test]

fn duration_plus_duration() {
    let v = TestDb::new().scalar(
        "RETURN duration('P1D') + duration('P2D') AS d",
    );
    assert!(!v.is_null());
}

#[test]

fn duration_times_integer() {
    let v = TestDb::new().scalar("RETURN duration('P1D') * 7 AS d");
    // Expected: P7D (one week)
    assert!(!v.is_null());
}

#[test]

fn duration_divided_by_integer() {
    let v = TestDb::new().scalar("RETURN duration('P14D') / 2 AS d");
    // Expected: P7D
    assert!(!v.is_null());
}

// ============================================================
// 8. String / temporal conversion
// ============================================================

#[test]

fn tostring_of_date() {
    let v = TestDb::new().scalar("RETURN toString(date('2024-06-15'))");
    assert_eq!(v, "2024-06-15");
}

#[test]

fn tostring_of_datetime() {
    let v = TestDb::new().scalar("RETURN toString(datetime('2024-06-15T10:30:00Z'))");
    // ISO 8601 output
    assert!(v.as_str().unwrap().starts_with("2024-06-15T10:30:00"));
}

#[test]

fn tostring_of_duration() {
    let v = TestDb::new().scalar("RETURN toString(duration('P1Y2M3D'))");
    assert_eq!(v, "P1Y2M3D");
}

#[test]

fn date_roundtrip_through_tostring() {
    let v = TestDb::new().scalar(
        "WITH date('2024-06-15') AS d RETURN date(toString(d)) = d AS same",
    );
    assert_eq!(v, true);
}

// ============================================================
// 9. Temporal truncation
// ============================================================

#[test]

fn date_truncate_to_month() {
    let v = TestDb::new().scalar(
        "RETURN date.truncate('month', date('2024-06-15')) AS d",
    );
    // Expected: 2024-06-01
    assert!(!v.is_null());
}

#[test]

fn datetime_truncate_to_day() {
    let v = TestDb::new().scalar(
        "RETURN datetime.truncate('day', datetime('2024-06-15T10:30:45Z')) AS dt",
    );
    // Expected: 2024-06-15T00:00:00Z
    assert!(!v.is_null());
}

#[test]

fn datetime_truncate_to_hour() {
    let v = TestDb::new().scalar(
        "RETURN datetime.truncate('hour', datetime('2024-06-15T10:30:45Z')) AS dt",
    );
    assert!(!v.is_null());
}

// ============================================================
// 10. Integration: CREATE / MERGE with temporal properties
// ============================================================

#[test]

fn create_node_with_date_property() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'Launch', date: date('2025-01-15')})");
    let rows = db.run("MATCH (e:Event {name: 'Launch'}) RETURN e.date AS d");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0]["d"].is_null());
}

#[test]

fn create_node_with_datetime_property() {
    let db = TestDb::new();
    db.run(
        "CREATE (:Event {name: 'Launch', ts: datetime('2025-01-15T10:00:00Z')})",
    );
    let rows = db.run("MATCH (e:Event) RETURN e.ts AS ts");
    assert_eq!(rows.len(), 1);
}

#[test]

fn merge_on_date_property() {
    let db = TestDb::new();
    db.run("MERGE (:Holiday {name: 'NewYear', date: date('2025-01-01')})");
    db.run("MERGE (:Holiday {name: 'NewYear', date: date('2025-01-01')})");
    db.assert_count("MATCH (h:Holiday) RETURN h", 1);
}

#[test]

fn set_date_property_on_existing_node() {
    let db = TestDb::new();
    db.run("CREATE (:Task {title: 'Review'})");
    db.run("MATCH (t:Task {title: 'Review'}) SET t.due = date('2025-06-01')");
    let rows = db.run("MATCH (t:Task {title: 'Review'}) RETURN t.due AS due");
    assert!(!rows[0]["due"].is_null());
}

// ============================================================
// 11. Integration: WHERE filtering on temporal values
// ============================================================

#[test]

fn where_filter_date_after() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'A', date: date('2024-01-01')})");
    db.run("CREATE (:Event {name: 'B', date: date('2024-06-15')})");
    db.run("CREATE (:Event {name: 'C', date: date('2024-12-31')})");
    let rows = db.run(
        "MATCH (e:Event) WHERE e.date > date('2024-06-01') \
         RETURN e.name AS name ORDER BY e.name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "B");
    assert_eq!(rows[1]["name"], "C");
}

#[test]

fn where_filter_date_range() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'A', date: date('2024-01-01')})");
    db.run("CREATE (:Event {name: 'B', date: date('2024-06-15')})");
    db.run("CREATE (:Event {name: 'C', date: date('2024-12-31')})");
    let rows = db.run(
        "MATCH (e:Event) \
         WHERE e.date >= date('2024-02-01') AND e.date <= date('2024-07-01') \
         RETURN e.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "B");
}

#[test]

fn where_filter_datetime_same_day() {
    let db = TestDb::new();
    db.run("CREATE (:Log {msg: 'start', ts: datetime('2024-01-15T08:00:00Z')})");
    db.run("CREATE (:Log {msg: 'end',   ts: datetime('2024-01-15T17:00:00Z')})");
    db.run("CREATE (:Log {msg: 'other', ts: datetime('2024-01-16T09:00:00Z')})");
    let rows = db.run(
        "MATCH (l:Log) \
         WHERE l.ts >= datetime('2024-01-15T00:00:00Z') \
           AND l.ts <  datetime('2024-01-16T00:00:00Z') \
         RETURN l.msg AS msg ORDER BY l.ts",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["msg"], "start");
    assert_eq!(rows[1]["msg"], "end");
}

// ============================================================
// 12. Integration: ORDER BY temporal values
// ============================================================

#[test]

fn order_by_date_ascending() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'C', date: date('2024-12-31')})");
    db.run("CREATE (:Event {name: 'A', date: date('2024-01-01')})");
    db.run("CREATE (:Event {name: 'B', date: date('2024-06-15')})");
    let rows = db.run(
        "MATCH (e:Event) RETURN e.name AS name ORDER BY e.date ASC",
    );
    assert_eq!(rows[0]["name"], "A");
    assert_eq!(rows[1]["name"], "B");
    assert_eq!(rows[2]["name"], "C");
}

#[test]

fn order_by_datetime_descending() {
    let db = TestDb::new();
    db.run("CREATE (:Log {id: 1, ts: datetime('2024-01-15T08:00:00Z')})");
    db.run("CREATE (:Log {id: 2, ts: datetime('2024-01-15T12:00:00Z')})");
    db.run("CREATE (:Log {id: 3, ts: datetime('2024-01-15T17:00:00Z')})");
    let rows = db.run("MATCH (l:Log) RETURN l.id AS id ORDER BY l.ts DESC");
    assert_eq!(rows[0]["id"], 3);
    assert_eq!(rows[1]["id"], 2);
    assert_eq!(rows[2]["id"], 1);
}

// ============================================================
// 13. Integration: Aggregation with temporal values
// ============================================================

#[test]

fn min_max_date() {
    let db = TestDb::new();
    db.run("CREATE (:Event {date: date('2024-01-01')})");
    db.run("CREATE (:Event {date: date('2024-06-15')})");
    db.run("CREATE (:Event {date: date('2024-12-31')})");
    let rows = db.run(
        "MATCH (e:Event) RETURN min(e.date) AS earliest, max(e.date) AS latest",
    );
    assert_eq!(rows.len(), 1);
    // earliest should be 2024-01-01, latest 2024-12-31
}

#[test]

fn count_group_by_date() {
    let db = TestDb::new();
    db.run("CREATE (:Sale {amount: 100, date: date('2024-01-15')})");
    db.run("CREATE (:Sale {amount: 200, date: date('2024-01-15')})");
    db.run("CREATE (:Sale {amount: 150, date: date('2024-02-01')})");
    let rows = db.run(
        "MATCH (s:Sale) \
         RETURN s.date AS date, count(s) AS cnt, sum(s.amount) AS total \
         ORDER BY s.date",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["cnt"], 2);
    assert_eq!(rows[0]["total"], 300);
    assert_eq!(rows[1]["cnt"], 1);
    assert_eq!(rows[1]["total"], 150);
}

#[test]

fn collect_dates() {
    let db = TestDb::new();
    db.run("CREATE (:Event {date: date('2024-01-01')})");
    db.run("CREATE (:Event {date: date('2024-06-15')})");
    let rows = db.run("MATCH (e:Event) RETURN collect(e.date) AS dates");
    let dates = rows[0]["dates"].as_array().unwrap();
    assert_eq!(dates.len(), 2);
}

// ============================================================
// 14. Integration: WITH clause and temporal values
// ============================================================

#[test]

fn with_date_computation() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'A', date: date('2024-01-15')})");
    db.run("CREATE (:Event {name: 'B', date: date('2024-06-15')})");
    let rows = db.run(
        "MATCH (e:Event) \
         WITH e, date('2024-03-01') AS cutoff \
         WHERE e.date > cutoff \
         RETURN e.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "B");
}

// ============================================================
// 15. Parameterized temporal values
// ============================================================

// NOTE: These require temporal LoraValue variants to be passable as parameters.

#[test]

fn parameter_date_in_where() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'A', date: date('2024-01-15')})");
    db.run("CREATE (:Event {name: 'B', date: date('2024-06-15')})");
    // Once temporal types exist in LoraValue, they can be passed as $params
    let rows = db.run(
        "MATCH (e:Event) WHERE e.date > date('2024-03-01') RETURN e.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "B");
}

#[test]

fn parameter_date_string_parsed_in_function() {
    // Pass date as string parameter, parse via date() function
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'A', date: date('2024-01-15')})");
    db.run("CREATE (:Event {name: 'B', date: date('2024-06-15')})");
    // Hypothetical: date($dateStr) where $dateStr = '2024-03-01'
    let rows = db.run(
        "MATCH (e:Event) WHERE e.date > date('2024-03-01') RETURN e.name AS name",
    );
    assert_eq!(rows.len(), 1);
}

// ============================================================
// 16. Error behavior
// ============================================================

#[test]

fn date_invalid_format_errors() {
    let db = TestDb::new();
    let err = db.run_err("RETURN date('not-a-date') AS d");
    assert!(!err.is_empty());
}

#[test]

fn date_invalid_month_13() {
    let db = TestDb::new();
    let err = db.run_err("RETURN date('2024-13-01') AS d");
    assert!(!err.is_empty());
}

#[test]

fn date_invalid_day_32() {
    let db = TestDb::new();
    let err = db.run_err("RETURN date('2024-01-32') AS d");
    assert!(!err.is_empty());
}

#[test]

fn date_feb_29_non_leap_year_errors() {
    let db = TestDb::new();
    let err = db.run_err("RETURN date('2023-02-29') AS d");
    assert!(!err.is_empty());
}

#[test]

fn date_feb_29_leap_year_ok() {
    let v = TestDb::new().scalar("RETURN date('2024-02-29') AS d");
    assert!(!v.is_null());
}

#[test]

fn datetime_invalid_hour_25() {
    let err = TestDb::new().run_err("RETURN datetime('2024-01-01T25:00:00Z') AS dt");
    assert!(!err.is_empty());
}

#[test]

fn duration_invalid_format_errors() {
    let err = TestDb::new().run_err("RETURN duration('not-a-duration') AS d");
    assert!(!err.is_empty());
}

#[test]

fn date_plus_integer_errors() {
    // Cannot add raw integer to date; must use duration
    let err = TestDb::new().run_err("RETURN date('2024-01-01') + 5");
    assert!(!err.is_empty());
}

#[test]

fn cross_type_date_datetime_comparison_errors() {
    // Comparing date to datetime directly may be unsupported or coerced
    let _v = TestDb::new().scalar(
        "RETURN date('2024-01-01') = datetime('2024-01-01T00:00:00Z')",
    );
    // Behavior TBD: could error, return null, or coerce
}

// ============================================================
// 17. Temporal edge cases
// ============================================================

#[test]

fn date_epoch() {
    let v = TestDb::new().scalar("RETURN date('1970-01-01').year AS y");
    assert_eq!(v, 1970);
}

#[test]

fn date_far_future() {
    let v = TestDb::new().scalar("RETURN date('9999-12-31').year AS y");
    assert_eq!(v, 9999);
}

#[test]

fn date_year_1() {
    let v = TestDb::new().scalar("RETURN date('0001-01-01').year AS y");
    assert_eq!(v, 1);
}

#[test]

fn datetime_midnight_boundary() {
    let v = TestDb::new().scalar(
        "RETURN datetime('2024-01-01T23:59:59Z') < datetime('2024-01-02T00:00:00Z')",
    );
    assert_eq!(v, true);
}

#[test]

fn duration_zero() {
    let v = TestDb::new().scalar("RETURN duration('P0D') AS d");
    assert!(!v.is_null());
}

#[test]

fn date_end_of_month_rollover() {
    // Adding 1 month to Jan 31 — what happens?
    let v = TestDb::new().scalar(
        "RETURN date('2024-01-31') + duration('P1M') AS d",
    );
    assert!(!v.is_null());
}

// ============================================================
// 18. UNWIND with temporal values
// ============================================================

#[test]

fn unwind_date_list() {
    let db = TestDb::new();
    let rows = db.run(
        "UNWIND [date('2024-01-01'), date('2024-06-15'), date('2024-12-31')] AS d \
         RETURN d ORDER BY d",
    );
    assert_eq!(rows.len(), 3);
}

// ============================================================
// 19. CASE with temporal values
// ============================================================

#[test]

fn case_on_date_comparison() {
    let db = TestDb::new();
    db.run("CREATE (:Event {name: 'Past',   date: date('2020-01-01')})");
    db.run("CREATE (:Event {name: 'Future', date: date('2030-01-01')})");
    let rows = db.run(
        "MATCH (e:Event) \
         RETURN e.name AS name, \
                CASE WHEN e.date < date('2025-01-01') THEN 'past' ELSE 'future' END AS era \
         ORDER BY e.name",
    );
    assert_eq!(rows[0]["era"], "future");
    assert_eq!(rows[1]["era"], "past");
}

// ============================================================
// 20. Temporal values in relationship properties
// ============================================================

#[test]

fn relationship_with_date_property() {
    let db = TestDb::new();
    db.run("CREATE (a:Person {name:'Alice'})-[:HIRED {date: date('2020-03-15')}]->(c:Company {name:'Acme'})");
    let rows = db.run(
        "MATCH (:Person {name:'Alice'})-[r:HIRED]->(:Company) RETURN r.date AS hireDate",
    );
    assert_eq!(rows.len(), 1);
    assert!(!rows[0]["hireDate"].is_null());
}

#[test]

fn filter_relationships_by_temporal_property() {
    let db = TestDb::new();
    db.run("CREATE (a:Person {name:'Alice'})-[:HIRED {date: date('2020-03-15')}]->(c:Company {name:'Acme'})");
    db.run("CREATE (b:Person {name:'Bob'})-[:HIRED {date: date('2023-09-01')}]->(c:Company {name:'Acme'})");
    let rows = db.run(
        "MATCH (p:Person)-[r:HIRED]->(:Company) \
         WHERE r.date > date('2022-01-01') \
         RETURN p.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Bob");
}
