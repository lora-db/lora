//! [`LoraDateTime`] (zoned) and [`LoraLocalDateTime`] (zone-naive)
//! combined date + time values.

use std::cmp::Ordering;
use std::fmt;

use super::calendar::{civil_from_days, days_from_civil, days_in_month, unix_now};
use super::date::LoraDate;
use super::duration::LoraDuration;
use super::format::{format_offset, format_subsecond};
use super::parsing::parse_time_string;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoraDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
    pub offset_seconds: i32,
}

impl LoraDateTime {
    #[allow(clippy::too_many_arguments)] // Structural datetime constructor — every field is required.
    pub fn new(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        nanosecond: u32,
        offset_seconds: i32,
    ) -> Result<Self, String> {
        LoraDate::new(year, month, day)?;
        if hour > 23 {
            return Err(format!("Invalid hour: {hour}"));
        }
        if minute > 59 {
            return Err(format!("Invalid minute: {minute}"));
        }
        if second > 59 {
            return Err(format!("Invalid second: {second}"));
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanosecond,
            offset_seconds,
        })
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let t_pos = s
            .find('T')
            .ok_or_else(|| format!("Invalid datetime: {s}"))?;
        let date_part = &s[..t_pos];
        let time_part = &s[t_pos + 1..];

        let date = LoraDate::parse(date_part)?;
        let (h, m, sec, ns, offset) = parse_time_string(time_part)?;
        let offset = offset.unwrap_or(0);

        Self::new(date.year, date.month, date.day, h, m, sec, ns, offset)
    }

    pub fn now() -> Self {
        let (secs, nanos) = unix_now();
        let days = (secs / 86400) as i64;
        let day_secs = secs % 86400;
        let (y, mo, d) = civil_from_days(days);
        Self {
            year: y,
            month: mo,
            day: d,
            hour: (day_secs / 3600) as u32,
            minute: ((day_secs % 3600) / 60) as u32,
            second: (day_secs % 60) as u32,
            nanosecond: nanos,
            offset_seconds: 0,
        }
    }

    /// Milliseconds since Unix epoch, normalized to UTC.
    pub fn to_epoch_millis(&self) -> i64 {
        let days = days_from_civil(self.year, self.month, self.day);
        let day_secs = self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64;
        let utc_secs = days * 86400 + day_secs - self.offset_seconds as i64;
        utc_secs * 1000 + self.nanosecond as i64 / 1_000_000
    }

    pub fn add_duration(&self, dur: &LoraDuration) -> Self {
        // Add months
        let total_months = self.year as i64 * 12 + (self.month as i64 - 1) + dur.months;
        let new_year = total_months.div_euclid(12) as i32;
        let new_month = (total_months.rem_euclid(12) + 1) as u32;
        let max_day = days_in_month(new_year, new_month);
        let new_day = self.day.min(max_day);

        // Add days + seconds
        let base_days = days_from_civil(new_year, new_month, new_day) + dur.days;
        let base_secs =
            self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64 + dur.seconds;

        let total_secs = base_days * 86400 + base_secs;
        let final_days = total_secs.div_euclid(86400);
        let rem = total_secs.rem_euclid(86400);
        let (y, m, d) = civil_from_days(final_days);

        Self {
            year: y,
            month: m,
            day: d,
            hour: (rem / 3600) as u32,
            minute: ((rem % 3600) / 60) as u32,
            second: (rem % 60) as u32,
            nanosecond: self.nanosecond,
            offset_seconds: self.offset_seconds,
        }
    }

    pub fn truncate_to_day(&self) -> Self {
        Self {
            year: self.year,
            month: self.month,
            day: self.day,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: 0,
            offset_seconds: self.offset_seconds,
        }
    }

    pub fn truncate_to_hour(&self) -> Self {
        Self {
            year: self.year,
            month: self.month,
            day: self.day,
            hour: self.hour,
            minute: 0,
            second: 0,
            nanosecond: 0,
            offset_seconds: self.offset_seconds,
        }
    }

    pub fn date(&self) -> LoraDate {
        LoraDate {
            year: self.year,
            month: self.month,
            day: self.day,
        }
    }
}

impl PartialOrd for LoraDateTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraDateTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_epoch_millis().cmp(&other.to_epoch_millis())
    }
}

impl fmt::Display for LoraDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )?;
        format_subsecond(f, self.nanosecond)?;
        format_offset(f, self.offset_seconds)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoraLocalDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
}

impl LoraLocalDateTime {
    pub fn parse(s: &str) -> Result<Self, String> {
        let t_pos = s
            .find('T')
            .ok_or_else(|| format!("Invalid localdatetime: {s}"))?;
        let date = LoraDate::parse(&s[..t_pos])?;
        let (h, m, sec, ns, _) = parse_time_string(&s[t_pos + 1..])?;
        if h > 23 {
            return Err(format!("Invalid hour: {h}"));
        }
        if m > 59 {
            return Err(format!("Invalid minute: {m}"));
        }
        if sec > 59 {
            return Err(format!("Invalid second: {sec}"));
        }
        Ok(Self {
            year: date.year,
            month: date.month,
            day: date.day,
            hour: h,
            minute: m,
            second: sec,
            nanosecond: ns,
        })
    }

    pub fn now() -> Self {
        let (secs, nanos) = unix_now();
        let days = (secs / 86400) as i64;
        let day_secs = secs % 86400;
        let (y, mo, d) = civil_from_days(days);
        Self {
            year: y,
            month: mo,
            day: d,
            hour: (day_secs / 3600) as u32,
            minute: ((day_secs % 3600) / 60) as u32,
            second: (day_secs % 60) as u32,
            nanosecond: nanos,
        }
    }
}

impl fmt::Display for LoraLocalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )?;
        format_subsecond(f, self.nanosecond)
    }
}
