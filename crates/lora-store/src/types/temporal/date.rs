//! [`LoraDate`] — a calendar date in the proleptic Gregorian calendar.

use std::cmp::Ordering;
use std::fmt;

use super::calendar::{civil_from_days, days_from_civil, days_in_month, unix_now};
use super::duration::LoraDuration;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoraDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl LoraDate {
    pub fn new(year: i32, month: u32, day: u32) -> Result<Self, String> {
        if !(1..=12).contains(&month) {
            return Err(format!("Invalid month: {month}"));
        }
        let max = days_in_month(year, month);
        if day < 1 || day > max {
            return Err(format!("Invalid day {day} for {year}-{month:02}"));
        }
        Ok(Self { year, month, day })
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 3 {
            return Err(format!("Invalid date format: {s}"));
        }
        let year = parts[0]
            .parse::<i32>()
            .map_err(|_| format!("Invalid date: {s}"))?;
        let month = parts[1]
            .parse::<u32>()
            .map_err(|_| format!("Invalid date: {s}"))?;
        let day = parts[2]
            .parse::<u32>()
            .map_err(|_| format!("Invalid date: {s}"))?;
        Self::new(year, month, day)
    }

    pub fn today() -> Self {
        let (secs, _) = unix_now();
        let days = (secs / 86400) as i64;
        Self::from_epoch_days(days)
    }

    pub fn to_epoch_days(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn from_epoch_days(days: i64) -> Self {
        let (y, m, d) = civil_from_days(days);
        Self {
            year: y,
            month: m,
            day: d,
        }
    }

    pub fn day_of_week(&self) -> u32 {
        let z = self.to_epoch_days();
        (((z % 7) + 7 + 3) % 7 + 1) as u32
    }

    pub fn day_of_year(&self) -> u32 {
        let mut doy = self.day;
        for m in 1..self.month {
            doy += days_in_month(self.year, m);
        }
        doy
    }

    pub fn add_duration(&self, dur: &LoraDuration) -> Self {
        self.try_add_duration(dur)
            .unwrap_or_else(|| clamp_duration_overflow(dur))
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
        let epoch = days_from_civil(new_year, new_month, new_day).checked_add(dur.days)?;
        let (year, month, day) = civil_from_days_checked(epoch)?;
        Some(Self { year, month, day })
    }

    pub fn sub_duration(&self, dur: &LoraDuration) -> Self {
        self.try_sub_duration(dur)
            .unwrap_or_else(|| clamp_duration_overflow(&dur.negate()))
    }

    pub fn try_sub_duration(&self, dur: &LoraDuration) -> Option<Self> {
        self.try_add_duration(&dur.try_negate()?)
    }

    pub fn truncate_to_month(&self) -> Self {
        Self {
            year: self.year,
            month: self.month,
            day: 1,
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

fn clamp_duration_overflow(dur: &LoraDuration) -> LoraDate {
    if dur.months < 0 || (dur.months == 0 && dur.days < 0) {
        LoraDate {
            year: i32::MIN,
            month: 1,
            day: 1,
        }
    } else {
        LoraDate {
            year: i32::MAX,
            month: 12,
            day: 31,
        }
    }
}

impl PartialOrd for LoraDate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraDate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.year
            .cmp(&other.year)
            .then(self.month.cmp(&other.month))
            .then(self.day.cmp(&other.day))
    }
}

impl fmt::Display for LoraDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_duration_add_rejects_year_overflow() {
        let date = LoraDate {
            year: i32::MAX,
            month: 12,
            day: 31,
        };
        let duration = LoraDuration {
            months: 1,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        };

        assert!(date.try_add_duration(&duration).is_none());
    }
}
