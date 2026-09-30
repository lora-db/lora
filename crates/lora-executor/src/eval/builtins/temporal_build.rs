//! Building temporal values the way Cypher's constructors do: from a map
//! of components, by truncating another temporal, from an epoch count,
//! and the duration between two temporals.
//!
//! A map may give a calendar date (`year, month, day`), an ISO week date
//! (`year, week, dayOfWeek`; `year` is then the week-based year), an
//! ordinal date (`year, ordinalDay`) or a quarter date (`year, quarter,
//! dayOfQuarter`); a clock (`hour, minute, second, millisecond,
//! microsecond, nanosecond`); a `timezone`; and a temporal to start from
//! (`date`, `time`, `datetime`). A component needs the larger ones above
//! it (`day` needs `month`, `second` needs `minute`) unless a temporal to
//! start from supplies them, and any key the target type has no use for
//! is an error that names it.

use std::collections::BTreeMap;

use lora_store::{
    LoraDate, LoraDateTime, LoraDuration, LoraLocalDateTime, LoraLocalTime, LoraTime,
};

use crate::value::LoraValue;

use super::super::point::timezone_name_to_offset;

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NANOS_PER_DAY: i128 = 86_400 * NANOS_PER_SECOND;
/// Neo4j's average Gregorian month, 30.436875 days, for fractional months.
const NANOS_PER_AVERAGE_MONTH: f64 = 2_629_746.0 * 1e9;

/// The five temporal instant types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Date,
    Time,
    LocalTime,
    DateTime,
    LocalDateTime,
}

impl Kind {
    /// The kind a `cast.to` type name denotes.
    pub(super) fn from_type_name(name: &str) -> Option<Self> {
        Some(match name {
            "DATE" => Self::Date,
            "TIME" => Self::Time,
            "LOCAL_TIME" => Self::LocalTime,
            "DATETIME" => Self::DateTime,
            "LOCAL_DATETIME" => Self::LocalDateTime,
            _ => return None,
        })
    }

    fn of(value: &LoraValue) -> Option<Self> {
        Some(match value {
            LoraValue::Date(_) => Self::Date,
            LoraValue::Time(_) => Self::Time,
            LoraValue::LocalTime(_) => Self::LocalTime,
            LoraValue::DateTime(_) => Self::DateTime,
            LoraValue::LocalDateTime(_) => Self::LocalDateTime,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Date => "DATE",
            Self::Time => "TIME",
            Self::LocalTime => "LOCAL_TIME",
            Self::DateTime => "DATETIME",
            Self::LocalDateTime => "LOCAL_DATETIME",
        }
    }

    fn has_date(self) -> bool {
        matches!(self, Self::Date | Self::DateTime | Self::LocalDateTime)
    }

    fn has_time(self) -> bool {
        !matches!(self, Self::Date)
    }

    fn has_zone(self) -> bool {
        matches!(self, Self::Time | Self::DateTime)
    }
}

/// The name of a value's type in errors: `LOCAL_TIME`, `INTEGER`, ….
fn type_name(value: &LoraValue) -> String {
    match Kind::of(value) {
        Some(kind) => kind.name().to_string(),
        None => crate::errors::value_kind(value).to_ascii_uppercase(),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Clock {
    hour: u32,
    minute: u32,
    second: u32,
    nanosecond: u32,
}

impl Clock {
    fn nanos_of_day(self) -> i128 {
        (self.hour as i128 * 3600 + self.minute as i128 * 60 + self.second as i128)
            * NANOS_PER_SECOND
            + self.nanosecond as i128
    }

    /// `nanos` must be within one day.
    fn from_nanos_of_day(nanos: i128) -> Self {
        let secs = (nanos / NANOS_PER_SECOND) as u32;
        Self {
            hour: secs / 3600,
            minute: secs % 3600 / 60,
            second: secs % 60,
            nanosecond: (nanos % NANOS_PER_SECOND) as u32,
        }
    }
}

/// The components a temporal value has.
#[derive(Debug, Clone, Default)]
struct Parts {
    date: Option<LoraDate>,
    clock: Option<Clock>,
    offset: Option<i32>,
}

impl Parts {
    fn of(value: &LoraValue) -> Option<Self> {
        let clock = |hour, minute, second, nanosecond| {
            Some(Clock {
                hour,
                minute,
                second,
                nanosecond,
            })
        };
        Some(match value {
            LoraValue::Date(d) => Self {
                date: Some(d.clone()),
                ..Self::default()
            },
            LoraValue::Time(t) => Self {
                date: None,
                clock: clock(t.hour, t.minute, t.second, t.nanosecond),
                offset: Some(t.offset_seconds),
            },
            LoraValue::LocalTime(t) => Self {
                date: None,
                clock: clock(t.hour, t.minute, t.second, t.nanosecond),
                offset: None,
            },
            LoraValue::DateTime(dt) => Self {
                date: Some(dt.date()),
                clock: clock(dt.hour, dt.minute, dt.second, dt.nanosecond),
                offset: Some(dt.offset_seconds),
            },
            LoraValue::LocalDateTime(dt) => Self {
                date: Some(LoraDate {
                    year: dt.year,
                    month: dt.month,
                    day: dt.day,
                }),
                clock: clock(dt.hour, dt.minute, dt.second, dt.nanosecond),
                offset: None,
            },
            _ => return None,
        })
    }

    /// Move a zoned value to `offset`, keeping its instant.
    fn shift_to(&mut self, offset: i32) -> Result<(), String> {
        let Some(from) = self.offset else {
            self.offset = Some(offset);
            return Ok(());
        };
        let delta = (offset as i128 - from as i128) * NANOS_PER_SECOND;
        let nanos = self.clock.unwrap_or_default().nanos_of_day() + delta;
        if let Some(date) = &self.date {
            let days = date.to_epoch_days() as i128 + nanos.div_euclid(NANOS_PER_DAY);
            self.date = Some(date_from_epoch_days(days)?);
        }
        if self.clock.is_some() {
            self.clock = Some(Clock::from_nanos_of_day(nanos.rem_euclid(NANOS_PER_DAY)));
        }
        self.offset = Some(offset);
        Ok(())
    }

    /// The value of type `kind`; a missing clock is midnight, a missing
    /// offset UTC.
    fn build(self, kind: Kind) -> Result<LoraValue, String> {
        let offset = self.offset.unwrap_or(0);
        let date = || {
            self.date
                .clone()
                .ok_or_else(|| "a date is required".to_string())
        };
        let clock = self.clock.unwrap_or_default();
        if clock.nanosecond >= 1_000_000_000 {
            return Err(format!("nanosecond out of range: {}", clock.nanosecond));
        }
        let Clock {
            hour,
            minute,
            second,
            nanosecond,
        } = clock;
        Ok(match kind {
            Kind::Date => LoraValue::Date(date()?),
            Kind::Time => LoraValue::Time(LoraTime::new(hour, minute, second, nanosecond, offset)?),
            Kind::LocalTime => {
                LoraValue::LocalTime(LoraLocalTime::new(hour, minute, second, nanosecond)?)
            }
            Kind::DateTime => {
                let d = date()?;
                LoraValue::DateTime(LoraDateTime::new(
                    d.year, d.month, d.day, hour, minute, second, nanosecond, offset,
                )?)
            }
            Kind::LocalDateTime => {
                let d = date()?;
                LoraLocalTime::new(hour, minute, second, nanosecond)?;
                LoraValue::LocalDateTime(LoraLocalDateTime {
                    year: d.year,
                    month: d.month,
                    day: d.day,
                    hour,
                    minute,
                    second,
                    nanosecond,
                })
            }
        })
    }
}

/// Another temporal as `kind`: the date of a datetime, the wall clock of a
/// zoned time, midnight UTC for a date as a datetime. `None` when `value`
/// lacks the date or the time `kind` needs.
pub(super) fn convert(value: &LoraValue, kind: Kind) -> Option<LoraValue> {
    let parts = Parts::of(value)?;
    let fits = if kind.has_date() {
        parts.date.is_some()
    } else {
        parts.clock.is_some()
    };
    fits.then(|| parts.build(kind).ok()).flatten()
}

fn date_from_epoch_days(days: i128) -> Result<LoraDate, String> {
    // About ±5.8 million years: the year stays well inside an i32.
    if days.unsigned_abs() > 2_000_000_000 {
        return Err("date out of range".to_string());
    }
    Ok(LoraDate::from_epoch_days(days as i64))
}

// --- component maps ----------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Year,
    Month,
    Day,
    Week,
    DayOfWeek,
    OrdinalDay,
    Quarter,
    DayOfQuarter,
    Hour,
    Minute,
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
    Timezone,
    Date,
    Time,
    DateTime,
    EpochSeconds,
    EpochMillis,
}

const FIELD_COUNT: usize = 20;

const FIELDS: [(&str, Field); FIELD_COUNT] = [
    ("year", Field::Year),
    ("month", Field::Month),
    ("day", Field::Day),
    ("week", Field::Week),
    ("dayOfWeek", Field::DayOfWeek),
    ("ordinalDay", Field::OrdinalDay),
    ("quarter", Field::Quarter),
    ("dayOfQuarter", Field::DayOfQuarter),
    ("hour", Field::Hour),
    ("minute", Field::Minute),
    ("second", Field::Second),
    ("millisecond", Field::Millisecond),
    ("microsecond", Field::Microsecond),
    ("nanosecond", Field::Nanosecond),
    ("timezone", Field::Timezone),
    ("date", Field::Date),
    ("time", Field::Time),
    ("datetime", Field::DateTime),
    ("epochSeconds", Field::EpochSeconds),
    ("epochMillis", Field::EpochMillis),
];

impl Field {
    /// Keys match case-insensitively, as in Neo4j; the exact spelling is
    /// the fast path.
    fn parse(key: &str) -> Option<Self> {
        FIELDS
            .iter()
            .find(|(name, _)| *name == key)
            .or_else(|| {
                FIELDS
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(key))
            })
            .map(|(_, field)| *field)
    }

    fn name(self) -> &'static str {
        FIELDS[self as usize].0
    }

    fn is_date(self) -> bool {
        (self as usize) <= Field::DayOfQuarter as usize
    }

    fn is_clock(self) -> bool {
        (Field::Hour as usize..=Field::Nanosecond as usize).contains(&(self as usize))
    }

    fn allowed(self, kind: Kind, truncating: bool) -> bool {
        match self {
            _ if self.is_date() => kind.has_date(),
            _ if self.is_clock() => kind.has_time(),
            Field::Timezone => kind.has_zone(),
            Field::Date => !truncating && kind.has_date(),
            Field::Time => !truncating && kind.has_time(),
            Field::DateTime => !truncating && matches!(kind, Kind::DateTime | Kind::LocalDateTime),
            _ => !truncating && kind == Kind::DateTime,
        }
    }
}

/// The non-null values a map gives, by field.
struct Given<'a> {
    values: [Option<&'a LoraValue>; FIELD_COUNT],
}

impl<'a> Given<'a> {
    fn collect(
        m: &'a BTreeMap<String, LoraValue>,
        kind: Kind,
        truncating: bool,
    ) -> Result<Self, String> {
        let mut values = [None; FIELD_COUNT];
        for (key, value) in m {
            let Some(field) = Field::parse(key) else {
                return Err(format!("unknown key `{key}`"));
            };
            if !field.allowed(kind, truncating) {
                return Err(format!("`{key}` is not a component of {}", kind.name()));
            }
            if !matches!(value, LoraValue::Null) {
                values[field as usize] = Some(value);
            }
        }
        Ok(Self { values })
    }

    fn has(&self, field: Field) -> bool {
        self.values[field as usize].is_some()
    }

    fn int(&self, field: Field) -> Result<Option<i64>, String> {
        match self.values[field as usize] {
            None => Ok(None),
            Some(LoraValue::Int(v)) => Ok(Some(*v)),
            Some(other) => Err(format!(
                "`{}` must be an integer, got {}",
                field.name(),
                type_name(other)
            )),
        }
    }

    fn u32(&self, field: Field) -> Result<Option<u32>, String> {
        self.int(field)?
            .map(|v| u32::try_from(v).map_err(|_| format!("`{}` out of range: {v}", field.name())))
            .transpose()
    }

    fn temporal(&self, field: Field) -> Result<Option<Parts>, String> {
        self.values[field as usize]
            .map(|v| {
                Parts::of(v).ok_or_else(|| {
                    format!(
                        "`{}` must be a temporal value, got {}",
                        field.name(),
                        type_name(v)
                    )
                })
            })
            .transpose()
    }

    /// The first of `fields` the map gives.
    fn first_of(&self, fields: &[Field]) -> Option<Field> {
        fields.iter().copied().find(|f| self.has(*f))
    }
}

/// `date({…})`, `datetime({…})`, … for the target `kind`.
pub(super) fn from_map(m: &BTreeMap<String, LoraValue>, kind: Kind) -> Result<LoraValue, String> {
    let given = Given::collect(m, kind, false)?;
    if let Some(epoch) = given.first_of(&[Field::EpochSeconds, Field::EpochMillis]) {
        return from_epoch_map(&given, epoch);
    }

    let mut base = Parts::default();
    if let Some(datetime) = given.temporal(Field::DateTime)? {
        if let Some(other) = given.first_of(&[Field::Date, Field::Time]) {
            return Err(format!("cannot combine `{}` with `datetime`", other.name()));
        }
        base = datetime;
    }
    if let Some(date) = given.temporal(Field::Date)? {
        base.date = Some(date.date.ok_or("`date` must have a date")?);
    }
    if let Some(time) = given.temporal(Field::Time)? {
        base.clock = Some(time.clock.ok_or("`time` must have a time")?);
        base.offset = time.offset;
    }
    // A zoned value given a new zone keeps its instant.
    if let Some(zone) = given.values[Field::Timezone as usize] {
        base.shift_to(parse_zone(zone)?)?;
    }
    apply_fields(&given, base, kind, false)
}

/// `datetime({epochSeconds[, nanosecond]})`, `datetime({epochMillis})`,
/// with an optional `timezone` to show the instant in.
fn from_epoch_map(given: &Given<'_>, epoch: Field) -> Result<LoraValue, String> {
    for (_, field) in FIELDS {
        let companion = field == Field::Timezone
            || (field == Field::Nanosecond && epoch == Field::EpochSeconds);
        if field != epoch && !companion && given.has(field) {
            return Err(format!(
                "cannot combine `{}` with `{}`",
                field.name(),
                epoch.name()
            ));
        }
    }
    let count = given.int(epoch)?.unwrap_or(0) as i128;
    let nanos = match epoch {
        Field::EpochSeconds => {
            count * NANOS_PER_SECOND + given.int(Field::Nanosecond)?.unwrap_or(0) as i128
        }
        _ => count * 1_000_000,
    };
    let offset = match given.values[Field::Timezone as usize] {
        Some(zone) => parse_zone(zone)?,
        None => 0,
    };
    datetime_from_instant(nanos, offset).map(LoraValue::DateTime)
}

/// Resolve the map's components over `base` and build `kind`.
fn apply_fields(
    given: &Given<'_>,
    mut base: Parts,
    kind: Kind,
    truncating: bool,
) -> Result<LoraValue, String> {
    if kind.has_date() {
        base.date = Some(resolve_date(given, base.date.as_ref())?);
    }
    if kind.has_time() {
        base.clock = Some(resolve_clock(given, base.clock, kind, truncating)?);
    }
    base.build(kind)
}

const CALENDAR: [Field; 2] = [Field::Month, Field::Day];
const WEEK: [Field; 2] = [Field::Week, Field::DayOfWeek];
const ORDINAL: [Field; 1] = [Field::OrdinalDay];
const QUARTER: [Field; 2] = [Field::Quarter, Field::DayOfQuarter];

fn resolve_date(given: &Given<'_>, base: Option<&LoraDate>) -> Result<LoraDate, String> {
    let mut group = None;
    for fields in [&CALENDAR[..], &WEEK[..], &ORDINAL[..], &QUARTER[..]] {
        if let Some(first) = given.first_of(fields) {
            if let Some((_, other)) = group {
                return Err(format!(
                    "cannot combine `{}` with `{}`",
                    first.name(),
                    Field::name(other)
                ));
            }
            group = Some((fields[0], first));
        }
    }
    let year = |base_year: Option<i32>| -> Result<i32, String> {
        match given.int(Field::Year)? {
            Some(y) => i32::try_from(y).map_err(|_| format!("`year` out of range: {y}")),
            None => base_year.ok_or_else(|| "`year` is required".to_string()),
        }
    };
    // A smaller component without the larger one, and nothing to take it from.
    let needs = |small: Field, large: Field| -> Result<(), String> {
        if base.is_none() && given.has(small) && !given.has(large) {
            return Err(format!("`{}` needs `{}`", small.name(), large.name()));
        }
        Ok(())
    };
    match group.map(|(g, _)| g) {
        Some(Field::Week) => {
            needs(Field::DayOfWeek, Field::Week)?;
            let (base_year, base_week) = base.map(LoraDate::iso_week).unzip();
            let year = year(base_year)?;
            let week = given.u32(Field::Week)?.or(base_week).unwrap_or(1);
            let dow = given
                .u32(Field::DayOfWeek)?
                .or(base.map(LoraDate::day_of_week))
                .unwrap_or(1);
            LoraDate::from_iso_week(year, week, dow)
        }
        Some(Field::OrdinalDay) => {
            let year = year(base.map(|d| d.year))?;
            LoraDate::from_ordinal(year, given.u32(Field::OrdinalDay)?.unwrap_or(1))
        }
        Some(Field::Quarter) => {
            needs(Field::DayOfQuarter, Field::Quarter)?;
            let year = year(base.map(|d| d.year))?;
            let quarter = given
                .u32(Field::Quarter)?
                .or(base.map(LoraDate::quarter))
                .unwrap_or(1);
            let day = given
                .u32(Field::DayOfQuarter)?
                .or(base.map(LoraDate::day_of_quarter))
                .unwrap_or(1);
            LoraDate::from_quarter(year, quarter, day)
        }
        _ => {
            needs(Field::Day, Field::Month)?;
            let year = year(base.map(|d| d.year))?;
            let month = given
                .u32(Field::Month)?
                .or(base.map(|d| d.month))
                .unwrap_or(1);
            let day = given.u32(Field::Day)?.or(base.map(|d| d.day)).unwrap_or(1);
            LoraDate::new(year, month, day)
        }
    }
}

fn resolve_clock(
    given: &Given<'_>,
    base: Option<Clock>,
    kind: Kind,
    truncating: bool,
) -> Result<Clock, String> {
    let sub_second = [Field::Millisecond, Field::Microsecond, Field::Nanosecond];
    if base.is_none() {
        for (small, large) in [(Field::Minute, Field::Hour), (Field::Second, Field::Minute)] {
            if given.has(small) && !given.has(large) {
                return Err(format!("`{}` needs `{}`", small.name(), large.name()));
            }
        }
        if let Some(small) = given.first_of(&sub_second) {
            if !given.has(Field::Second) {
                return Err(format!("`{}` needs `second`", small.name()));
            }
        }
        if !kind.has_date() && !given.has(Field::Hour) {
            return Err("`hour` is required".to_string());
        }
    }
    let mut clock = base.unwrap_or_default();
    if let Some(h) = given.u32(Field::Hour)? {
        clock.hour = h;
    }
    if let Some(m) = given.u32(Field::Minute)? {
        clock.minute = m;
    }
    if let Some(s) = given.u32(Field::Second)? {
        clock.second = s;
    }
    if given.first_of(&sub_second).is_some() {
        let mut nanos = 0u64;
        for (field, scale) in sub_second.into_iter().zip([1_000_000u64, 1_000, 1]) {
            nanos += given.u32(field)?.unwrap_or(0) as u64 * scale;
        }
        // Truncation keeps the sub-second part it left and adds to it.
        if truncating {
            nanos += clock.nanosecond as u64;
        }
        if nanos >= 1_000_000_000 {
            return Err(format!("sub-second components add up to {nanos}ns"));
        }
        clock.nanosecond = nanos as u32;
    }
    Ok(clock)
}

/// A `timezone`: an offset (`'+01:00'`, `'Z'`) or one of the zone names
/// the engine knows. An unknown zone is an error rather than UTC.
fn parse_zone(value: &LoraValue) -> Result<i32, String> {
    let LoraValue::String(tz) = value else {
        return Err(format!(
            "`timezone` must be a string, got {}",
            type_name(value)
        ));
    };
    if let Ok(t) = LoraTime::parse(&format!("00:00{tz}")) {
        return Ok(t.offset_seconds);
    }
    let offset = timezone_name_to_offset(tz);
    if offset != 0 || matches!(tz.as_str(), "UTC" | "GMT" | "Europe/London") {
        Ok(offset)
    } else {
        Err(format!("unknown time zone `{tz}`"))
    }
}

// --- durations -----------------------------------------------------------------

/// A whole part and a fraction still to cascade into smaller units.
#[derive(Default)]
struct Amount {
    whole: i128,
    fraction: f64,
}

impl Amount {
    fn add(&mut self, key: &str, value: &LoraValue, scale: i64) -> Result<(), String> {
        match value {
            LoraValue::Null => {}
            LoraValue::Int(v) => self.whole += *v as i128 * scale as i128,
            LoraValue::Float(f) if f.is_finite() => {
                let x = f * scale as f64;
                self.whole += x.trunc() as i128;
                self.fraction += x.fract();
            }
            other => {
                return Err(format!(
                    "`{key}` must be a finite number, got {}",
                    type_name(other)
                ))
            }
        }
        Ok(())
    }

    /// Move the whole units the fractions added up to into `whole`.
    fn settle(&mut self) {
        let whole = self.fraction.trunc();
        self.whole += whole as i128;
        self.fraction -= whole;
    }
}

/// Each duration key: the amount it adds to (months, days, seconds,
/// nanoseconds) and how many of that amount one unit is.
const DURATION_KEYS: [(&str, usize, i64); 10] = [
    ("years", 0, 12),
    ("months", 0, 1),
    ("weeks", 1, 7),
    ("days", 1, 1),
    ("hours", 2, 3600),
    ("minutes", 2, 60),
    ("seconds", 2, 1),
    ("milliseconds", 3, 1_000_000),
    ("microseconds", 3, 1_000),
    ("nanoseconds", 3, 1),
];

/// `duration({…})`: `years, months, weeks, days, hours, minutes, seconds,
/// milliseconds, microseconds, nanoseconds`, each an integer or a float.
/// A fraction cascades down as in Neo4j: a fractional month into days (an
/// average month is 30.436875 days) and the rest into seconds, a
/// fractional day or second into the smaller units.
pub(super) fn duration_from_map(m: &BTreeMap<String, LoraValue>) -> Result<LoraDuration, String> {
    // Months, days, seconds and nanoseconds, each summed over its keys.
    let mut amounts: [Amount; 4] = Default::default();
    for (key, value) in m {
        let (slot, scale) = DURATION_KEYS
            .iter()
            .find(|(name, ..)| *name == key.as_str())
            .or_else(|| {
                DURATION_KEYS
                    .iter()
                    .find(|(name, ..)| name.eq_ignore_ascii_case(key))
            })
            .map(|(_, slot, scale)| (*slot, *scale))
            .ok_or_else(|| format!("unknown key `{key}`"))?;
        amounts[slot].add(key, value, scale)?;
    }
    let [mut months, mut days, seconds, nanos] = amounts;

    months.settle();
    let month_nanos = (months.fraction * NANOS_PER_AVERAGE_MONTH).round() as i128;
    days.whole += month_nanos / NANOS_PER_DAY;
    days.settle();
    let time_nanos = month_nanos % NANOS_PER_DAY
        + (days.fraction * NANOS_PER_DAY as f64).round() as i128
        + seconds.whole * NANOS_PER_SECOND
        + (seconds.fraction * NANOS_PER_SECOND as f64).round() as i128
        + nanos.whole
        + nanos.fraction.round() as i128;
    duration(months.whole, days.whole, time_nanos)
}

fn duration(months: i128, days: i128, nanos: i128) -> Result<LoraDuration, String> {
    let out_of_range = |_| "duration out of range".to_string();
    Ok(LoraDuration {
        months: i64::try_from(months).map_err(out_of_range)?,
        days: i64::try_from(days).map_err(out_of_range)?,
        seconds: i64::try_from(nanos / NANOS_PER_SECOND).map_err(out_of_range)?,
        nanoseconds: (nanos % NANOS_PER_SECOND) as i64,
    })
}

/// What `duration.between` and its `in*` variants measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Between {
    /// Months, then days, then the time: `duration.between`.
    Full,
    Months,
    Days,
    Seconds,
}

/// The duration from `a` to `b`, as Neo4j computes it: with both zoned,
/// `b` is read in `a`'s zone; a value without a time is at midnight;
/// with either lacking a date only the times of day count.
pub(super) fn between(a: &LoraValue, b: &LoraValue, what: Between) -> Result<LoraDuration, String> {
    let parts = |v: &LoraValue| {
        Parts::of(v).ok_or_else(|| format!("expected a temporal value, got {}", type_name(v)))
    };
    let from = parts(a)?;
    let mut to = parts(b)?;
    if let (Some(offset), Some(_)) = (from.offset, to.offset) {
        to.shift_to(offset)?;
    }
    let from_nod = from.clock.unwrap_or_default().nanos_of_day();
    let to_nod = to.clock.unwrap_or_default().nanos_of_day();
    let (Some(from_date), Some(to_date)) = (from.date, to.date) else {
        return match what {
            Between::Full | Between::Seconds => duration(0, 0, to_nod - from_nod),
            Between::Months | Between::Days => Ok(LoraDuration::zero()),
        };
    };
    let to_day = to_date.to_epoch_days() as i128;
    match what {
        Between::Months => duration(months_until(&from_date, from_nod, &to_date, to_nod), 0, 0),
        Between::Days => duration(0, days_until(&from_date, from_nod, &to_date, to_nod), 0),
        Between::Seconds => duration(
            0,
            0,
            (to_day - from_date.to_epoch_days() as i128) * NANOS_PER_DAY + to_nod - from_nod,
        ),
        Between::Full => {
            let months = months_until(&from_date, from_nod, &to_date, to_nod);
            let shifted = from_date
                .try_add_duration(&LoraDuration {
                    months: i64::try_from(months).map_err(|_| "duration out of range")?,
                    ..LoraDuration::zero()
                })
                .ok_or("duration out of range")?;
            let days = days_until(&shifted, from_nod, &to_date, to_nod);
            let from_day = shifted.to_epoch_days() as i128 + days;
            duration(
                months,
                days,
                (to_day - from_day) * NANOS_PER_DAY + to_nod - from_nod,
            )
        }
    }
}

/// `to`'s date, a day nearer `from` when its time of day has not yet
/// reached `from`'s: whole units count only complete days.
fn complete_end(from: &LoraDate, from_nod: i128, to: &LoraDate, to_nod: i128) -> i64 {
    let (start, end) = (from.to_epoch_days(), to.to_epoch_days());
    if end > start && to_nod < from_nod {
        end - 1
    } else if end < start && to_nod > from_nod {
        end + 1
    } else {
        end
    }
}

fn months_until(from: &LoraDate, from_nod: i128, to: &LoraDate, to_nod: i128) -> i128 {
    let end = LoraDate::from_epoch_days(complete_end(from, from_nod, to, to_nod));
    let packed = |d: &LoraDate| (d.year as i128 * 12 + d.month as i128 - 1) * 32 + d.day as i128;
    (packed(&end) - packed(from)) / 32
}

fn days_until(from: &LoraDate, from_nod: i128, to: &LoraDate, to_nod: i128) -> i128 {
    (complete_end(from, from_nod, to, to_nod) - from.to_epoch_days()) as i128
}

// --- epoch and truncation ------------------------------------------------------

/// The instant `nanos` after the epoch, shown at `offset`.
fn datetime_from_instant(nanos: i128, offset: i32) -> Result<LoraDateTime, String> {
    let local = nanos + offset as i128 * NANOS_PER_SECOND;
    let date = date_from_epoch_days(local.div_euclid(NANOS_PER_DAY))?;
    let clock = Clock::from_nanos_of_day(local.rem_euclid(NANOS_PER_DAY));
    LoraDateTime::new(
        date.year,
        date.month,
        date.day,
        clock.hour,
        clock.minute,
        clock.second,
        clock.nanosecond,
        offset,
    )
}

/// `datetime.fromepoch(seconds, nanoseconds)`, in UTC.
pub(super) fn from_epoch(seconds: i64, nanoseconds: i64) -> Result<LoraValue, String> {
    datetime_from_instant(seconds as i128 * NANOS_PER_SECOND + nanoseconds as i128, 0)
        .map(LoraValue::DateTime)
}

/// `datetime.fromepochmillis(milliseconds)`, in UTC.
pub(super) fn from_epoch_millis(millis: i64) -> Result<LoraValue, String> {
    datetime_from_instant(millis as i128 * 1_000_000, 0).map(LoraValue::DateTime)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Unit {
    Millennium,
    Century,
    Decade,
    Year,
    WeekYear,
    Quarter,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Microsecond,
}

impl Unit {
    fn parse(unit: &str) -> Option<Self> {
        Some(match unit.to_ascii_lowercase().as_str() {
            "millennium" => Self::Millennium,
            "century" => Self::Century,
            "decade" => Self::Decade,
            "year" => Self::Year,
            "weekyear" => Self::WeekYear,
            "quarter" => Self::Quarter,
            "month" => Self::Month,
            "week" => Self::Week,
            "day" => Self::Day,
            "hour" => Self::Hour,
            "minute" => Self::Minute,
            "second" => Self::Second,
            "millisecond" => Self::Millisecond,
            "microsecond" => Self::Microsecond,
            _ => return None,
        })
    }

    fn truncate_date(self, d: &LoraDate) -> Result<LoraDate, String> {
        let year_floor = |n: i32| LoraDate::new(d.year - d.year.rem_euclid(n), 1, 1);
        match self {
            Self::Millennium => year_floor(1000),
            Self::Century => year_floor(100),
            Self::Decade => year_floor(10),
            Self::Year => year_floor(1),
            Self::WeekYear => LoraDate::from_iso_week(d.iso_week().0, 1, 1),
            Self::Quarter => LoraDate::from_quarter(d.year, d.quarter(), 1),
            Self::Month => Ok(d.truncate_to_month()),
            Self::Week => {
                date_from_epoch_days((d.to_epoch_days() - (d.day_of_week() as i64 - 1)) as i128)
            }
            _ => Ok(d.clone()),
        }
    }

    fn truncate_clock(self, c: Clock) -> Clock {
        let keep = |hour, minute, second, nanosecond| Clock {
            hour,
            minute,
            second,
            nanosecond,
        };
        match self {
            Self::Hour => keep(c.hour, 0, 0, 0),
            Self::Minute => keep(c.hour, c.minute, 0, 0),
            Self::Second => keep(c.hour, c.minute, c.second, 0),
            Self::Millisecond => keep(
                c.hour,
                c.minute,
                c.second,
                c.nanosecond / 1_000_000 * 1_000_000,
            ),
            Self::Microsecond => keep(c.hour, c.minute, c.second, c.nanosecond / 1_000 * 1_000),
            _ => Clock::default(),
        }
    }
}

/// `<type>.truncate(unit, value[, map])`: `value` cut down to `unit`,
/// then the map's components applied over it. The type is `kind`, or the
/// value's own for `temporal.truncate(unit, value)`. A `timezone` in the
/// map replaces the zone, keeping the local time; sub-second components
/// add to what truncation kept.
pub(super) fn truncate(
    unit: &str,
    value: &LoraValue,
    map: Option<&BTreeMap<String, LoraValue>>,
    kind: Option<Kind>,
) -> Result<LoraValue, String> {
    let kind = match kind.or_else(|| Kind::of(value)) {
        Some(kind) => kind,
        None => return Err(format!("cannot truncate {}", type_name(value))),
    };
    let unit_name = unit;
    let unit = Unit::parse(unit).ok_or_else(|| format!("unknown truncation unit `{unit}`"))?;
    if (!kind.has_time() && unit > Unit::Day) || (!kind.has_date() && unit < Unit::Day) {
        return Err(format!("cannot truncate {} to `{unit_name}`", kind.name()));
    }
    let mut parts = Parts::of(value)
        .filter(|p| {
            (!kind.has_date() || p.date.is_some()) && (kind.has_date() || p.clock.is_some())
        })
        .ok_or_else(|| format!("cannot truncate {} to {}", type_name(value), kind.name()))?;
    if let Some(date) = &parts.date {
        parts.date = Some(unit.truncate_date(date)?);
    }
    parts.clock = Some(unit.truncate_clock(parts.clock.unwrap_or_default()));

    let Some(m) = map.filter(|m| !m.is_empty()) else {
        return parts.build(kind);
    };
    let given = Given::collect(m, kind, true)?;
    if let Some(zone) = given.values[Field::Timezone as usize] {
        parts.offset = Some(parse_zone(zone)?);
    }
    apply_fields(&given, parts, kind, true)
}
