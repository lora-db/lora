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
        // Add months first
        let total_months = self.year as i64 * 12 + (self.month as i64 - 1) + dur.months;
        let new_year = total_months.div_euclid(12) as i32;
        let new_month = (total_months.rem_euclid(12) + 1) as u32;
        let max_day = days_in_month(new_year, new_month);
        let new_day = self.day.min(max_day);
        // Then add days
        let epoch = days_from_civil(new_year, new_month, new_day) + dur.days;
        Self::from_epoch_days(epoch)
    }

    pub fn sub_duration(&self, dur: &LoraDuration) -> Self {
        self.add_duration(&dur.negate())
    }

    pub fn truncate_to_month(&self) -> Self {
        Self {
            year: self.year,
            month: self.month,
            day: 1,
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
