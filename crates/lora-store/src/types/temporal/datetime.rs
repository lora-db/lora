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
        self.try_add_duration(dur)
            .unwrap_or_else(|| clamp_duration_overflow(self.offset_seconds, dur))
    }

    pub fn try_add_duration(&self, dur: &LoraDuration) -> Option<Self> {
        let current_months = (self.year as i64)
            .checked_mul(12)?
            .checked_add(self.month as i64 - 1)?;
        let total_months = current_months.checked_add(dur.months)?;
        let year = total_months.div_euclid(12);
        let new_year = i32::try_from(year).ok()?;
        let new_month = (total_months.rem_euclid(12) + 1) as u32;
        let max_day = days_in_month(new_year, new_month);
        let new_day = self.day.min(max_day);

        let base_days = days_from_civil(new_year, new_month, new_day).checked_add(dur.days)?;
        let day_secs = (self.hour as i64)
            .checked_mul(3600)?
            .checked_add((self.minute as i64).checked_mul(60)?)?
            .checked_add(self.second as i64)?;
        let base_secs = day_secs.checked_add(dur.seconds)?;
        let total_nanos = (self.nanosecond as i64).checked_add(dur.nanoseconds)?;
        let extra_secs = total_nanos.div_euclid(1_000_000_000);
        let final_nanos = total_nanos.rem_euclid(1_000_000_000) as u32;
        let total_secs = base_days
            .checked_mul(86400)?
            .checked_add(base_secs)?
            .checked_add(extra_secs)?;
        let final_days = total_secs.div_euclid(86400);
        let rem = total_secs.rem_euclid(86400);
        let (y, m, d) = civil_from_days_checked(final_days)?;

        Some(Self {
            year: y,
            month: m,
            day: d,
            hour: (rem / 3600) as u32,
            minute: ((rem % 3600) / 60) as u32,
            second: (rem % 60) as u32,
            nanosecond: final_nanos,
            offset_seconds: self.offset_seconds,
        })
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

fn civil_from_days_checked(days: i64) -> Option<(i32, u32, u32)> {
    let z = days.checked_add(719_468)?;
    let era = if z >= 0 { z } else { z.checked_sub(146_096)? }.checked_div(146_097)?;
    let era_days = era.checked_mul(146_097)?;
    let doe = u64::try_from(z.checked_sub(era_days)?).ok()?;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64).checked_add(era.checked_mul(400)?)?;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y.checked_add(1)? } else { y };
    Some((i32::try_from(y).ok()?, m as u32, d as u32))
}

fn clamp_duration_overflow(offset_seconds: i32, dur: &LoraDuration) -> LoraDateTime {
    if dur.months < 0
        || (dur.months == 0 && dur.days < 0)
        || (dur.months == 0 && dur.days == 0 && dur.seconds < 0)
        || (dur.months == 0 && dur.days == 0 && dur.seconds == 0 && dur.nanoseconds < 0)
    {
        LoraDateTime {
            year: i32::MIN,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            nanosecond: 0,
            offset_seconds,
        }
    } else {
        LoraDateTime {
            year: i32::MAX,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            nanosecond: 999_999_999,
            offset_seconds,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_duration_add_rejects_year_overflow() {
        let datetime = LoraDateTime {
            year: i32::MAX,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            nanosecond: 0,
            offset_seconds: 0,
        };
        let duration = LoraDuration {
            months: 1,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        };

        assert!(datetime.try_add_duration(&duration).is_none());
    }

    #[test]
    fn checked_duration_add_carries_nanoseconds() {
        let datetime = LoraDateTime {
            year: 2026,
            month: 5,
            day: 11,
            hour: 23,
            minute: 59,
            second: 59,
            nanosecond: 900_000_000,
            offset_seconds: 0,
        };
        let duration = LoraDuration {
            months: 0,
            days: 0,
            seconds: 0,
            nanoseconds: 200_000_000,
        };

        let out = datetime.try_add_duration(&duration).unwrap();
        assert_eq!(
            (out.day, out.hour, out.minute, out.second, out.nanosecond),
            (12, 0, 0, 0, 100_000_000)
        );
    }
}
