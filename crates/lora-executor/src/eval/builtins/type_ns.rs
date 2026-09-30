//! `type.*` — runtime type inspection.
//!
//! Type operations are kept separate from `value.*` so value-polymorphic
//! helpers (`value.size`, `value.keys`, `value.id`) do not also become the
//! home for the type language. Casts live in `cast.*`. The display names
//! are the canonical lora spellings; parsing accepts common aliases such
//! as `INT`, `BOOL`, and `RELATIONSHIP`.

use std::borrow::Cow;

use lora_store::{
    parse_string_values, LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime,
    LoraTime, LoraVector, RawCoordinate, VectorCoordinateType,
};

use crate::value::LoraValue;

use super::super::errors::set_eval_error;
use super::super::point::build_point_from_map;
use super::temporal_build::{self, Kind};

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "of" => of(args),
        "is" => is(args),
        "cast" => cast_to(args),
        "can_cast" => cast_can(args),
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn known(op: &str) -> Option<()> {
    matches!(op, "of" | "is").then_some(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeType {
    Null,
    Boolean,
    Integer,
    Float,
    String,
    Binary,
    List(Option<Box<RuntimeType>>),
    Map,
    Node,
    Edge,
    Path,
    Date,
    Time,
    LocalTime,
    DateTime,
    LocalDateTime,
    Duration,
    Point,
    Vector {
        coord: Option<String>,
        dimension: Option<usize>,
    },
    Any,
}

impl RuntimeType {
    fn of(value: Option<&LoraValue>) -> Self {
        match value {
            Some(LoraValue::Null) | None => Self::Null,
            Some(LoraValue::Bool(_)) => Self::Boolean,
            Some(LoraValue::Int(_)) => Self::Integer,
            Some(LoraValue::Float(_)) => Self::Float,
            Some(LoraValue::String(_)) => Self::String,
            Some(LoraValue::Binary(_)) => Self::Binary,
            Some(LoraValue::List(items)) => {
                let element = list_element_type(items).unwrap_or(Self::Any);
                Self::List(Some(Box::new(element)))
            }
            Some(LoraValue::Map(_)) => Self::Map,
            Some(LoraValue::Node(_)) => Self::Node,
            Some(LoraValue::Relationship(_)) => Self::Edge,
            Some(LoraValue::Path(_)) => Self::Path,
            Some(LoraValue::Date(_)) => Self::Date,
            Some(LoraValue::Time(_)) => Self::Time,
            Some(LoraValue::LocalTime(_)) => Self::LocalTime,
            Some(LoraValue::DateTime(_)) => Self::DateTime,
            Some(LoraValue::LocalDateTime(_)) => Self::LocalDateTime,
            Some(LoraValue::Duration(_)) => Self::Duration,
            Some(LoraValue::Point(_)) => Self::Point,
            Some(LoraValue::Vector(v)) => Self::Vector {
                coord: Some(v.coordinate_type().as_str().to_string()),
                dimension: Some(v.dimension),
            },
        }
    }

    fn parse(input: &str) -> Option<Self> {
        let normalized = normalize_type_name(input);
        if normalized == "ANY" {
            return Some(Self::Any);
        }
        if normalized == "LIST" {
            return Some(Self::List(None));
        }
        if let Some(inner) = normalized
            .strip_prefix("LIST<")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            return Some(Self::List(Some(Box::new(Self::parse(inner)?))));
        }
        if normalized == "VECTOR" {
            return Some(Self::Vector {
                coord: None,
                dimension: None,
            });
        }
        if let Some(rest) = normalized.strip_prefix("VECTOR<") {
            let (coord, tail) = rest.split_once('>')?;
            let coord = coord.replace('_', " ");
            let coord = VectorCoordinateType::parse(coord.trim())?
                .as_str()
                .to_string();
            let dimension = if tail.is_empty() {
                None
            } else {
                Some(
                    tail.strip_prefix('(')
                        .and_then(|value| value.strip_suffix(')'))
                        .and_then(|value| value.parse::<usize>().ok())?,
                )
            };
            return Some(Self::Vector {
                coord: Some(coord),
                dimension,
            });
        }
        Some(match normalized.as_str() {
            "NULL" => Self::Null,
            "BOOLEAN" | "BOOL" => Self::Boolean,
            "INTEGER" | "INT" => Self::Integer,
            "FLOAT" | "REAL" | "DOUBLE" => Self::Float,
            "STRING" | "TEXT" => Self::String,
            "BINARY" | "BYTES" => Self::Binary,
            "MAP" => Self::Map,
            "NODE" => Self::Node,
            "EDGE" | "RELATIONSHIP" => Self::Edge,
            "PATH" => Self::Path,
            "DATE" => Self::Date,
            "TIME" | "ZONED_TIME" => Self::Time,
            "LOCAL_TIME" => Self::LocalTime,
            "DATETIME" | "DATE_TIME" | "ZONED_DATETIME" | "ZONED_DATE_TIME" => Self::DateTime,
            "LOCAL_DATETIME" | "LOCAL_DATE_TIME" => Self::LocalDateTime,
            "DURATION" => Self::Duration,
            "POINT" => Self::Point,
            _ => return None,
        })
    }

    fn matches(&self, actual: &Self) -> bool {
        match (self, actual) {
            (Self::Any, _) => true,
            (Self::List(None), Self::List(_)) => true,
            (Self::List(Some(expected)), Self::List(Some(actual))) => expected.matches(actual),
            (Self::List(Some(expected)), Self::List(None)) => matches!(**expected, Self::Any),
            (
                Self::Vector {
                    coord: expected_coord,
                    dimension: expected_dimension,
                },
                Self::Vector {
                    coord: actual_coord,
                    dimension: actual_dimension,
                },
            ) => {
                expected_coord
                    .as_ref()
                    .zip(actual_coord.as_ref())
                    .is_none_or(|(expected, actual)| expected == actual)
                    && expected_dimension
                        .zip(*actual_dimension)
                        .is_none_or(|(expected, actual)| expected == actual)
            }
            _ => self == actual,
        }
    }

    fn display(&self) -> Cow<'static, str> {
        match self {
            Self::Null => Cow::Borrowed("NULL"),
            Self::Boolean => Cow::Borrowed("BOOLEAN"),
            Self::Integer => Cow::Borrowed("INTEGER"),
            Self::Float => Cow::Borrowed("FLOAT"),
            Self::String => Cow::Borrowed("STRING"),
            Self::Binary => Cow::Borrowed("BINARY"),
            Self::List(None) => Cow::Borrowed("LIST"),
            Self::List(Some(inner)) => Cow::Owned(format!("LIST<{}>", inner.display())),
            Self::Map => Cow::Borrowed("MAP"),
            Self::Node => Cow::Borrowed("NODE"),
            Self::Edge => Cow::Borrowed("EDGE"),
            Self::Path => Cow::Borrowed("PATH"),
            Self::Date => Cow::Borrowed("DATE"),
            Self::Time => Cow::Borrowed("TIME"),
            Self::LocalTime => Cow::Borrowed("LOCAL_TIME"),
            Self::DateTime => Cow::Borrowed("DATETIME"),
            Self::LocalDateTime => Cow::Borrowed("LOCAL_DATETIME"),
            Self::Duration => Cow::Borrowed("DURATION"),
            Self::Point => Cow::Borrowed("POINT"),
            Self::Vector { coord, dimension } => match (coord, dimension) {
                (Some(coord), Some(dimension)) => {
                    Cow::Owned(format!("VECTOR<{coord}>({dimension})"))
                }
                (Some(coord), None) => Cow::Owned(format!("VECTOR<{coord}>")),
                _ => Cow::Borrowed("VECTOR"),
            },
            Self::Any => Cow::Borrowed("ANY"),
        }
    }
}

fn of(args: &[LoraValue]) -> LoraValue {
    LoraValue::String(RuntimeType::of(args.first()).display().into_owned())
}

fn is(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(expected)) = args.get(1) else {
        return LoraValue::Null;
    };
    let Some(expected) = RuntimeType::parse(expected) else {
        return LoraValue::Null;
    };
    LoraValue::Bool(expected.matches(&RuntimeType::of(args.first())))
}

pub(super) fn cast_can(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(target)) = args.get(1) else {
        return LoraValue::Null;
    };
    let Some(target) = RuntimeType::parse(target) else {
        return LoraValue::Null;
    };
    LoraValue::Bool(cast_value(args.first(), &target, false).is_ok())
}

pub(super) fn cast_to(args: &[LoraValue]) -> LoraValue {
    let Some(target_arg) = args.get(1) else {
        set_eval_error("cast.to requires a target type".to_string());
        return LoraValue::Null;
    };
    let LoraValue::String(target_name) = target_arg else {
        set_eval_error(format!(
            "cast.to target type must be a type literal or string, got {}",
            crate::errors::value_kind(target_arg)
        ));
        return LoraValue::Null;
    };
    let Some(target) = RuntimeType::parse(target_name) else {
        set_eval_error(format!("unknown cast target type `{target_name}`"));
        return LoraValue::Null;
    };
    match args.first() {
        None | Some(LoraValue::Null) => LoraValue::Null,
        Some(value) => match cast_value(Some(value), &target, true) {
            Ok(cast) => cast,
            Err(reason) => {
                let reason = reason.map(|r| format!(": {r}")).unwrap_or_default();
                set_eval_error(format!(
                    "cannot cast {} to {}{reason}",
                    crate::errors::value_kind(value),
                    target.display()
                ));
                LoraValue::Null
            }
        },
    }
}

pub(super) fn cast_try(args: &[LoraValue]) -> LoraValue {
    let Some(LoraValue::String(target)) = args.get(1) else {
        return LoraValue::Null;
    };
    let Some(target) = RuntimeType::parse(target) else {
        return LoraValue::Null;
    };
    cast_value(args.first(), &target, false).unwrap_or(LoraValue::Null)
}

/// A failed cast, with the reason when there is more to say than
/// "cannot cast X to Y".
type CastResult = Result<LoraValue, Option<String>>;

fn cast_value(value: Option<&LoraValue>, target: &RuntimeType, report_errors: bool) -> CastResult {
    let plain = |cast: Option<LoraValue>| cast.ok_or(None);
    match target {
        RuntimeType::String => plain(cast_string(value)),
        RuntimeType::Integer => plain(cast_integer(value)),
        RuntimeType::Float => plain(cast_float(value)),
        RuntimeType::Boolean => plain(cast_boolean(value)),
        RuntimeType::Date => cast_temporal(value, Kind::Date),
        RuntimeType::Time => cast_temporal(value, Kind::Time),
        RuntimeType::LocalTime => cast_temporal(value, Kind::LocalTime),
        RuntimeType::DateTime => cast_temporal(value, Kind::DateTime),
        RuntimeType::LocalDateTime => cast_temporal(value, Kind::LocalDateTime),
        RuntimeType::Duration => cast_duration(value),
        RuntimeType::Point => plain(cast_point(value, report_errors)),
        RuntimeType::Vector { coord, dimension } => {
            plain(cast_vector(value, coord.as_deref(), *dimension))
        }
        _ => Err(None),
    }
}

fn cast_string(value: Option<&LoraValue>) -> Option<LoraValue> {
    Some(match value? {
        LoraValue::Null => return None,
        LoraValue::String(s) => LoraValue::String(s.clone()),
        LoraValue::Int(i) => LoraValue::String(i.to_string()),
        LoraValue::Float(f) => LoraValue::String(f.to_string()),
        LoraValue::Bool(b) => LoraValue::String(b.to_string()),
        LoraValue::Date(d) => LoraValue::String(d.to_string()),
        LoraValue::DateTime(dt) => LoraValue::String(dt.to_string()),
        LoraValue::LocalDateTime(dt) => LoraValue::String(dt.to_string()),
        LoraValue::Time(t) => LoraValue::String(t.to_string()),
        LoraValue::LocalTime(t) => LoraValue::String(t.to_string()),
        LoraValue::Duration(dur) => LoraValue::String(dur.to_string()),
        _ => return None,
    })
}

fn cast_integer(value: Option<&LoraValue>) -> Option<LoraValue> {
    Some(match value? {
        LoraValue::Int(i) => LoraValue::Int(*i),
        LoraValue::Float(f) => LoraValue::Int(f64_to_i64(*f)?),
        LoraValue::String(s) => LoraValue::Int(s.parse::<i64>().ok()?),
        LoraValue::Bool(b) => LoraValue::Int(if *b { 1 } else { 0 }),
        _ => return None,
    })
}

fn cast_float(value: Option<&LoraValue>) -> Option<LoraValue> {
    Some(match value? {
        LoraValue::Float(f) => LoraValue::Float(*f),
        LoraValue::Int(i) => LoraValue::Float(*i as f64),
        LoraValue::String(s) => LoraValue::Float(s.parse::<f64>().ok()?),
        _ => return None,
    })
}

fn cast_boolean(value: Option<&LoraValue>) -> Option<LoraValue> {
    Some(match value? {
        LoraValue::Bool(b) => LoraValue::Bool(*b),
        LoraValue::String(s) if s.eq_ignore_ascii_case("true") => LoraValue::Bool(true),
        LoraValue::String(s) if s.eq_ignore_ascii_case("false") => LoraValue::Bool(false),
        LoraValue::Int(i) => LoraValue::Bool(*i != 0),
        _ => return None,
    })
}

// Temporal casts follow Cypher's constructors: `date(x)` is
// `cast.to(x, DATE)`. A string is parsed, a map gives the components (see
// `temporal_build`), and another temporal value contributes the
// components the target has: the date of a datetime, the wall clock of a
// zoned time. A value without a date or a time the target needs (a TIME
// cast to DATE) does not cast.

fn cast_temporal(value: Option<&LoraValue>, kind: Kind) -> CastResult {
    match value.ok_or(None)? {
        LoraValue::String(s) => parse_temporal(s, kind).map_err(Some),
        LoraValue::Map(m) => temporal_build::from_map(m, kind).map_err(Some),
        other => Ok(match (kind, other) {
            (Kind::Date, LoraValue::Date(d)) => LoraValue::Date(d.clone()),
            (Kind::Date, LoraValue::DateTime(dt)) => LoraValue::Date(dt.date()),
            (Kind::Date, LoraValue::LocalDateTime(dt)) => LoraValue::Date(local_datetime_date(dt)),
            (Kind::Time, LoraValue::Time(t)) => LoraValue::Time(t.clone()),
            (Kind::LocalTime, LoraValue::LocalTime(t)) => LoraValue::LocalTime(t.clone()),
            (Kind::DateTime, LoraValue::DateTime(dt)) => LoraValue::DateTime(dt.clone()),
            (Kind::LocalDateTime, LoraValue::LocalDateTime(dt)) => {
                LoraValue::LocalDateTime(dt.clone())
            }
            // Every other pair goes through the components.
            _ => temporal_build::convert(other, kind).ok_or(None)?,
        }),
    }
}

fn parse_temporal(s: &str, kind: Kind) -> Result<LoraValue, String> {
    Ok(match kind {
        Kind::Date => LoraValue::Date(match LoraDate::parse(s) {
            Ok(d) => d,
            // The date of a datetime string.
            Err(e) => LoraDateTime::parse(s).map(|dt| dt.date()).map_err(|_| e)?,
        }),
        Kind::Time => LoraValue::Time(LoraTime::parse(s)?),
        Kind::LocalTime => LoraValue::LocalTime(LoraLocalTime::parse(s)?),
        // A date alone is midnight UTC: `datetime('2026-10-01')`.
        Kind::DateTime => LoraValue::DateTime(match LoraDateTime::parse(s) {
            Ok(dt) => dt,
            Err(e) => {
                let d = LoraDate::parse(s).map_err(|_| e)?;
                LoraDateTime::new(d.year, d.month, d.day, 0, 0, 0, 0, 0)?
            }
        }),
        Kind::LocalDateTime => LoraValue::LocalDateTime(match LoraLocalDateTime::parse(s) {
            Ok(dt) => dt,
            Err(e) => {
                let d = LoraDate::parse(s).map_err(|_| e)?;
                LoraLocalDateTime {
                    year: d.year,
                    month: d.month,
                    day: d.day,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    nanosecond: 0,
                }
            }
        }),
    })
}

fn local_datetime_date(dt: &LoraLocalDateTime) -> LoraDate {
    LoraDate {
        year: dt.year,
        month: dt.month,
        day: dt.day,
    }
}

fn cast_duration(value: Option<&LoraValue>) -> CastResult {
    Ok(LoraValue::Duration(match value.ok_or(None)? {
        LoraValue::Duration(d) => d.clone(),
        LoraValue::String(s) => LoraDuration::parse(s).map_err(Some)?,
        LoraValue::Map(m) => temporal_build::duration_from_map(m).map_err(Some)?,
        _ => return Err(None),
    }))
}

fn cast_point(value: Option<&LoraValue>, report_errors: bool) -> Option<LoraValue> {
    Some(match value? {
        LoraValue::Point(p) => LoraValue::Point(p.clone()),
        LoraValue::Map(m) => match build_point_from_map(m) {
            Ok(Some(point)) => LoraValue::Point(point),
            Ok(None) => LoraValue::Null,
            Err(err) => {
                if report_errors {
                    set_eval_error(err);
                    LoraValue::Null
                } else {
                    return None;
                }
            }
        },
        other => {
            if report_errors {
                set_eval_error(format!(
                    "CAST( AS POINT) requires a map, got {}",
                    crate::errors::value_kind(other)
                ));
                LoraValue::Null
            } else {
                return None;
            }
        }
    })
}

fn cast_vector(
    value: Option<&LoraValue>,
    coord: Option<&str>,
    dimension: Option<usize>,
) -> Option<LoraValue> {
    let value = value?;
    if let LoraValue::Vector(v) = value {
        let actual = RuntimeType::of(Some(value));
        let expected = RuntimeType::Vector {
            coord: coord.map(str::to_string),
            dimension,
        };
        return expected
            .matches(&actual)
            .then(|| LoraValue::Vector(v.clone()));
    }

    let coord = VectorCoordinateType::parse(coord?)?;
    let dimension = i64::try_from(dimension?).ok()?;
    let raw = match value {
        LoraValue::List(items) => coerce_list_to_raw_coords(items).ok()?,
        LoraValue::String(s) => parse_string_values(s).ok()?,
        _ => return None,
    };
    LoraVector::try_new(raw, dimension, coord)
        .ok()
        .map(LoraValue::Vector)
}

fn coerce_list_to_raw_coords(items: &[LoraValue]) -> Result<Vec<RawCoordinate>, ()> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            LoraValue::Int(i) => out.push(RawCoordinate::Int(*i)),
            LoraValue::Float(f) if f.is_finite() => out.push(RawCoordinate::Float(*f)),
            _ => return Err(()),
        }
    }
    Ok(out)
}

fn list_element_type(items: &[LoraValue]) -> Option<RuntimeType> {
    let first = items.first()?;
    let first_type = RuntimeType::of(Some(first));
    if items
        .iter()
        .all(|item| RuntimeType::of(Some(item)) == first_type)
    {
        Some(first_type)
    } else {
        Some(RuntimeType::Any)
    }
}

fn normalize_type_name(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| match ch {
            '-' => '_',
            ch if ch.is_whitespace() => '_',
            _ => ch.to_ascii_uppercase(),
        })
        .collect()
}

fn f64_to_i64(value: f64) -> Option<i64> {
    (value.is_finite() && value >= i64::MIN as f64 && value < 9_223_372_036_854_775_808.0)
        .then_some(value as i64)
}
