//! `temporal.*` — date / time / datetime / duration operations.
//!
//! Value construction is handled by `cast.to(value, TYPE)` / `CAST(value AS
//! TYPE)`. This namespace is reserved for current-time helpers and temporal
//! operations.

use std::collections::BTreeMap;

use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
};

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        // Current-instant
        "now" => now(args),
        "today" => LoraValue::Date(LoraDate::today()),
        "timestamp" => timestamp(),
        "timezone" => timezone(),
        // Operations
        "parse" => parse(args),
        "format" => format_temporal(args),
        "reformat" => reformat(args),
        "convert" => convert(args),
        "add" => add(args),
        "get" => get(args),
        "fields" => fields(args),
        "truncate" => truncate(args),
        "between" => between(args),
        "in_days" => in_days(args),
        _ => return None,
    })
}

// --- current-instant -------------------------------------------------------

fn now(args: &[LoraValue]) -> LoraValue {
    let kind = match args.first() {
        Some(LoraValue::String(s)) => s.to_ascii_lowercase(),
        _ => "datetime".to_string(),
    };
    match kind.as_str() {
        "date" => LoraValue::Date(LoraDate::today()),
        "datetime" => LoraValue::DateTime(LoraDateTime::now()),
        "time" => LoraValue::Time(LoraTime::now()),
        "local_time" => LoraValue::LocalTime(LoraLocalTime::now()),
        "local_datetime" => LoraValue::LocalDateTime(LoraLocalDateTime::now()),
        _ => LoraValue::Null,
    }
}

fn timestamp() -> LoraValue {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    LoraValue::Int(millis)
}

fn timezone() -> LoraValue {
    LoraValue::String("UTC".to_string())
}

// --- operations ------------------------------------------------------------

fn parse(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(s)) = args.first() else {
        return LoraValue::Null;
    };
    let kind = match args.get(2) {
        Some(LoraValue::String(k)) => k.to_ascii_lowercase(),
        _ => detect_kind(s),
    };
    match kind.as_str() {
        "date" => LoraDate::parse(s)
            .map(LoraValue::Date)
            .unwrap_or(LoraValue::Null),
        "datetime" => LoraDateTime::parse(s)
            .map(LoraValue::DateTime)
            .unwrap_or(LoraValue::Null),
        "duration" => LoraDuration::parse(s)
            .map(LoraValue::Duration)
            .unwrap_or(LoraValue::Null),
        _ => LoraValue::Null,
    }
}

fn detect_kind(s: &str) -> String {
    if s.starts_with('P') {
        "duration".to_string()
    } else if s.contains('T') {
        "datetime".to_string()
    } else {
        "date".to_string()
    }
}

fn format_temporal(args: &[LoraValue]) -> LoraValue {
    let Some(v) = args.first() else {
        return LoraValue::Null;
    };
    LoraValue::String(match v {
        LoraValue::Date(d) => d.to_string(),
        LoraValue::DateTime(dt) => dt.to_string(),
        LoraValue::LocalDateTime(dt) => dt.to_string(),
        LoraValue::Time(t) => t.to_string(),
        LoraValue::LocalTime(t) => t.to_string(),
        LoraValue::Duration(d) => d.to_string(),
        _ => return LoraValue::Null,
    })
}

fn reformat(args: &[LoraValue]) -> LoraValue {
    let (Some(LoraValue::String(s)), Some(LoraValue::String(_from)), Some(LoraValue::String(_to))) =
        (args.first(), args.get(1), args.get(2))
    else {
        return LoraValue::Null;
    };
    if let Ok(dt) = LoraDateTime::parse(s) {
        return LoraValue::String(dt.to_string());
    }
    if let Ok(d) = LoraDate::parse(s) {
        return LoraValue::String(d.to_string());
    }
    LoraValue::Null
}

fn convert(args: &[LoraValue]) -> LoraValue {
    let (Some(n), Some(LoraValue::String(from)), Some(LoraValue::String(to))) = (
        args.first().and_then(LoraValue::as_i64),
        args.get(1),
        args.get(2),
    ) else {
        return LoraValue::Null;
    };
    match to_nanos(n, from).and_then(|ns| from_nanos(ns, to)) {
        Some(out) => LoraValue::Int(out),
        None => LoraValue::Null,
    }
}

fn to_nanos(n: i64, unit: &str) -> Option<i128> {
    let v: i64 = match unit.to_ascii_lowercase().as_str() {
        "ns" | "nanos" | "nanoseconds" => 1,
        "us" | "micros" | "microseconds" => 1_000,
        "ms" | "millis" | "milliseconds" => 1_000_000,
        "s" | "seconds" => 1_000_000_000,
        "m" | "minutes" => 60_000_000_000,
        "h" | "hours" => 3_600_000_000_000,
        "d" | "days" => 86_400_000_000_000,
        "w" | "weeks" => 604_800_000_000_000,
        _ => return None,
    };
    Some(n as i128 * v as i128)
}

fn from_nanos(ns: i128, unit: &str) -> Option<i64> {
    let divisor: i64 = match unit.to_ascii_lowercase().as_str() {
        "ns" | "nanos" | "nanoseconds" => 1,
        "us" | "micros" | "microseconds" => 1_000,
        "ms" | "millis" | "milliseconds" => 1_000_000,
        "s" | "seconds" => 1_000_000_000,
        "m" | "minutes" => 60_000_000_000,
        "h" | "hours" => 3_600_000_000_000,
        "d" | "days" => 86_400_000_000_000,
        "w" | "weeks" => 604_800_000_000_000,
        _ => return None,
    };
    i64::try_from(ns / divisor as i128).ok()
}

fn add(args: &[LoraValue]) -> LoraValue {
    let (Some(t), Some(LoraValue::Duration(d))) = (args.first(), args.get(1)) else {
        return LoraValue::Null;
    };
    match t {
        LoraValue::Date(date) => LoraValue::Date(date.add_duration(d)),
        LoraValue::DateTime(dt) => LoraValue::DateTime(dt.add_duration(d)),
        _ => LoraValue::Null,
    }
}

fn get(args: &[LoraValue]) -> LoraValue {
    let (Some(t), Some(LoraValue::String(field))) = (args.first(), args.get(1)) else {
        return LoraValue::Null;
    };
    let field = field.to_ascii_lowercase();
    match t {
        LoraValue::Date(d) => match field.as_str() {
            "year" => LoraValue::Int(d.year as i64),
            "month" => LoraValue::Int(d.month as i64),
            "day" => LoraValue::Int(d.day as i64),
            "day_of_week" => LoraValue::Int(d.day_of_week() as i64),
            "day_of_year" => LoraValue::Int(d.day_of_year() as i64),
            _ => LoraValue::Null,
        },
        LoraValue::DateTime(dt) => match field.as_str() {
            "year" => LoraValue::Int(dt.year as i64),
            "month" => LoraValue::Int(dt.month as i64),
            "day" => LoraValue::Int(dt.day as i64),
            "hour" => LoraValue::Int(dt.hour as i64),
            "minute" => LoraValue::Int(dt.minute as i64),
            "second" => LoraValue::Int(dt.second as i64),
            "nanosecond" => LoraValue::Int(dt.nanosecond as i64),
            "offset_seconds" => LoraValue::Int(dt.offset_seconds as i64),
            _ => LoraValue::Null,
        },
        LoraValue::Duration(d) => match field.as_str() {
            "years" => LoraValue::Int(d.years_component()),
            "months" => LoraValue::Int(d.months_component()),
            "days" => LoraValue::Int(d.days_component()),
            "hours" => LoraValue::Int(d.hours_component()),
            "minutes" => LoraValue::Int(d.minutes_component()),
            "seconds" => LoraValue::Int(d.seconds_component()),
            _ => LoraValue::Null,
        },
        _ => LoraValue::Null,
    }
}

fn fields(args: &[LoraValue]) -> LoraValue {
    let mut m: BTreeMap<String, LoraValue> = BTreeMap::new();
    match args.first() {
        Some(LoraValue::Date(d)) => {
            m.insert("year".into(), LoraValue::Int(d.year as i64));
            m.insert("month".into(), LoraValue::Int(d.month as i64));
            m.insert("day".into(), LoraValue::Int(d.day as i64));
        }
        Some(LoraValue::DateTime(dt)) => {
            m.insert("year".into(), LoraValue::Int(dt.year as i64));
            m.insert("month".into(), LoraValue::Int(dt.month as i64));
            m.insert("day".into(), LoraValue::Int(dt.day as i64));
            m.insert("hour".into(), LoraValue::Int(dt.hour as i64));
            m.insert("minute".into(), LoraValue::Int(dt.minute as i64));
            m.insert("second".into(), LoraValue::Int(dt.second as i64));
            m.insert("nanosecond".into(), LoraValue::Int(dt.nanosecond as i64));
            m.insert(
                "offset_seconds".into(),
                LoraValue::Int(dt.offset_seconds as i64),
            );
        }
        _ => return LoraValue::Null,
    }
    LoraValue::Map(m)
}

fn truncate(args: &[LoraValue]) -> LoraValue {
    // Accept either argument order — `temporal.truncate('month', d)` reads
    // best in Cypher; `temporal.truncate(d, 'month')` matches the
    // value-first convention of the rest of `temporal.*`.
    let (unit_str, value) = match (args.first(), args.get(1)) {
        (Some(LoraValue::String(u)), Some(v)) => (u.clone(), v),
        (Some(v), Some(LoraValue::String(u))) => (u.clone(), v),
        _ => return LoraValue::Null,
    };
    let unit = unit_str.to_ascii_lowercase();
    let t = value;
    match t {
        LoraValue::Date(d) => match unit.as_str() {
            "year" => LoraValue::Date(LoraDate {
                year: d.year,
                month: 1,
                day: 1,
            }),
            "month" => LoraValue::Date(d.truncate_to_month()),
            "day" => LoraValue::Date(d.clone()),
            _ => LoraValue::Null,
        },
        LoraValue::DateTime(dt) => match unit.as_str() {
            "year" => LoraValue::DateTime(LoraDateTime {
                year: dt.year,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                nanosecond: 0,
                offset_seconds: dt.offset_seconds,
            }),
            "month" => LoraValue::DateTime(LoraDateTime {
                year: dt.year,
                month: dt.month,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                nanosecond: 0,
                offset_seconds: dt.offset_seconds,
            }),
            "day" => LoraValue::DateTime(dt.truncate_to_day()),
            "hour" => LoraValue::DateTime(dt.truncate_to_hour()),
            _ => LoraValue::Null,
        },
        _ => LoraValue::Null,
    }
}

fn between(args: &[LoraValue]) -> LoraValue {
    match (args.first(), args.get(1)) {
        (Some(LoraValue::Date(a)), Some(LoraValue::Date(b))) => {
            LoraValue::Duration(LoraDuration::between_dates(a, b))
        }
        (Some(LoraValue::DateTime(a)), Some(LoraValue::DateTime(b))) => {
            LoraValue::Duration(LoraDuration::between_datetimes(a, b))
        }
        _ => LoraValue::Null,
    }
}

fn in_days(args: &[LoraValue]) -> LoraValue {
    match (args.first(), args.get(1)) {
        (Some(LoraValue::Date(a)), Some(LoraValue::Date(b))) => {
            LoraValue::Duration(LoraDuration::in_days(a, b))
        }
        _ => LoraValue::Null,
    }
}
