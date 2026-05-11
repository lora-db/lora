//! [`LoraDuration`] — ISO-8601 calendar/clock duration.
//!
//! Stores months, days, seconds, and nanoseconds as four signed
//! integers. Months and days are deliberately *not* normalised against
//! seconds (a month is not a fixed number of seconds), so the
//! constructor preserves whatever the caller wrote.

use std::cmp::Ordering;
use std::fmt;

use super::date::LoraDate;
use super::datetime::LoraDateTime;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoraDuration {
    pub months: i64,
    pub days: i64,
    pub seconds: i64,
    pub nanoseconds: i64,
}

impl LoraDuration {
    pub fn zero() -> Self {
        Self {
            months: 0,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if !s.starts_with('P') {
            return Err(format!("Invalid duration format: {s}"));
        }
        let rest = &s[1..];
        if rest.is_empty() {
            return Err(format!("Invalid duration: {s}"));
        }

        let mut months: i64 = 0;
        let mut days: i64 = 0;
        let mut seconds: i64 = 0;
        let mut nanoseconds: i64 = 0;
        let mut in_time = false;
        let mut num_buf = String::new();

        for c in rest.chars() {
            match c {
                'T' => {
                    in_time = true;
                }
                '0'..='9' | '.' => {
                    num_buf.push(c);
                }
                'Y' if !in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    let years = n
                        .checked_mul(12)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    months = months
                        .checked_add(years)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    num_buf.clear();
                }
                'M' if !in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    months = months
                        .checked_add(n)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    num_buf.clear();
                }
                'W' if !in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    let weeks = n
                        .checked_mul(7)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    days = days
                        .checked_add(weeks)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    num_buf.clear();
                }
                'D' => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    days = days
                        .checked_add(n)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    num_buf.clear();
                }
                'H' if in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    let hours = n
                        .checked_mul(3600)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    seconds = seconds
                        .checked_add(hours)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    num_buf.clear();
                }
                'M' if in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    let minutes = n
                        .checked_mul(60)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    seconds = seconds
                        .checked_add(minutes)
                        .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    num_buf.clear();
                }
                'S' if in_time => {
                    if num_buf.contains('.') {
                        let n: f64 = num_buf
                            .parse()
                            .map_err(|_| format!("Invalid duration: {s}"))?;
                        seconds = seconds
                            .checked_add(n.floor() as i64)
                            .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                        let frac = n - n.floor();
                        if frac > 0.0 {
                            nanoseconds = nanoseconds
                                .checked_add((frac * 1_000_000_000.0) as i64)
                                .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                        }
                    } else {
                        let n: i64 = num_buf
                            .parse()
                            .map_err(|_| format!("Invalid duration: {s}"))?;
                        seconds = seconds
                            .checked_add(n)
                            .ok_or_else(|| format!("Duration component overflow: {s}"))?;
                    }
                    num_buf.clear();
                }
                _ => return Err(format!("Invalid duration format: {s}")),
            }
        }

        if !num_buf.is_empty() {
            return Err(format!("Trailing number in duration: {s}"));
        }

        Ok(Self {
            months,
            days,
            seconds,
            nanoseconds,
        })
    }

    pub fn negate(&self) -> Self {
        self.try_negate().unwrap_or_else(|| Self {
            months: self.months.saturating_neg(),
            days: self.days.saturating_neg(),
            seconds: self.seconds.saturating_neg(),
            nanoseconds: self.nanoseconds.saturating_neg(),
        })
    }

    pub fn try_negate(&self) -> Option<Self> {
        Self {
            months: self.months.checked_neg()?,
            days: self.days.checked_neg()?,
            seconds: self.seconds.checked_neg()?,
            nanoseconds: self.nanoseconds.checked_neg()?,
        }
        .into()
    }

    pub fn add(&self, other: &Self) -> Self {
        self.try_add(other).unwrap_or_else(|| Self {
            months: self.months.saturating_add(other.months),
            days: self.days.saturating_add(other.days),
            seconds: self.seconds.saturating_add(other.seconds),
            nanoseconds: self.nanoseconds.saturating_add(other.nanoseconds),
        })
    }

    pub fn try_add(&self, other: &Self) -> Option<Self> {
        Self {
            months: self.months.checked_add(other.months)?,
            days: self.days.checked_add(other.days)?,
            seconds: self.seconds.checked_add(other.seconds)?,
            nanoseconds: self.nanoseconds.checked_add(other.nanoseconds)?,
        }
        .into()
    }

    pub fn mul_int(&self, n: i64) -> Self {
        self.try_mul_int(n).unwrap_or_else(|| Self {
            months: self.months.saturating_mul(n),
            days: self.days.saturating_mul(n),
            seconds: self.seconds.saturating_mul(n),
            nanoseconds: self.nanoseconds.saturating_mul(n),
        })
    }

    pub fn try_mul_int(&self, n: i64) -> Option<Self> {
        Self {
            months: self.months.checked_mul(n)?,
            days: self.days.checked_mul(n)?,
            seconds: self.seconds.checked_mul(n)?,
            nanoseconds: self.nanoseconds.checked_mul(n)?,
        }
        .into()
    }

    pub fn div_int(&self, n: i64) -> Self {
        self.try_div_int(n).unwrap_or_else(Self::zero)
    }

    pub fn try_div_int(&self, n: i64) -> Option<Self> {
        if n == 0 {
            return None;
        }
        Self {
            months: self.months.checked_div(n)?,
            days: self.days.checked_div(n)?,
            seconds: self.seconds.checked_div(n)?,
            nanoseconds: self.nanoseconds.checked_div(n)?,
        }
        .into()
    }

    /// Duration from date1 to date2 expressed as months + days.
    pub fn between_dates(from: &LoraDate, to: &LoraDate) -> Self {
        let sign: i64 = if from <= to { 1 } else { -1 };
        let (earlier, later) = if from <= to { (from, to) } else { (to, from) };

        // Count full months
        let mut months = (later.year as i64 - earlier.year as i64) * 12
            + (later.month as i64 - earlier.month as i64);

        // Apply months to earlier and check if we overshot
        let intermediate = earlier.add_duration(&LoraDuration {
            months,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        });
        if intermediate.to_epoch_days() > later.to_epoch_days() {
            months -= 1;
        }
        let intermediate = earlier.add_duration(&LoraDuration {
            months,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        });
        let remaining_days = later.to_epoch_days() - intermediate.to_epoch_days();

        Self {
            months: months * sign,
            days: remaining_days * sign,
            seconds: 0,
            nanoseconds: 0,
        }
    }

    /// Duration from date1 to date2 expressed purely in days.
    pub fn in_days(from: &LoraDate, to: &LoraDate) -> Self {
        let days = to.to_epoch_days() - from.to_epoch_days();
        Self {
            months: 0,
            days,
            seconds: 0,
            nanoseconds: 0,
        }
    }

    /// Duration between two datetimes, expressed in days + seconds.
    pub fn between_datetimes(from: &LoraDateTime, to: &LoraDateTime) -> Self {
        let ms_diff = to.to_epoch_millis() - from.to_epoch_millis();
        let total_secs = ms_diff / 1000;
        let remaining_ms = ms_diff % 1000;
        Self {
            months: 0,
            days: total_secs / 86400,
            seconds: total_secs % 86400,
            nanoseconds: remaining_ms * 1_000_000,
        }
    }

    /// Approximate total seconds for ordering purposes.
    pub fn total_seconds_approx(&self) -> f64 {
        // 1 month ≈ 30.4375 days
        self.months as f64 * 2_629_800.0
            + self.days as f64 * 86400.0
            + self.seconds as f64
            + self.nanoseconds as f64 / 1_000_000_000.0
    }

    pub fn years_component(&self) -> i64 {
        self.months / 12
    }
    pub fn months_component(&self) -> i64 {
        self.months % 12
    }
    pub fn days_component(&self) -> i64 {
        self.days
    }
    pub fn hours_component(&self) -> i64 {
        self.seconds / 3600
    }
    pub fn minutes_component(&self) -> i64 {
        (self.seconds % 3600) / 60
    }
    pub fn seconds_component(&self) -> i64 {
        self.seconds % 60
    }
}

impl PartialOrd for LoraDuration {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraDuration {
    fn cmp(&self, other: &Self) -> Ordering {
        // total_seconds_approx returns f64; total_cmp gives a total order
        // (NaN comparisons become deterministic) so Ord can be authoritative.
        self.total_seconds_approx()
            .total_cmp(&other.total_seconds_approx())
    }
}

impl fmt::Display for LoraDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P")?;
        let years = self.months / 12;
        let months = self.months % 12;
        if years != 0 {
            write!(f, "{}Y", years)?;
        }
        if months != 0 {
            write!(f, "{}M", months)?;
        }
        if self.days != 0 {
            write!(f, "{}D", self.days)?;
        }

        let hours = self.seconds / 3600;
        let minutes = (self.seconds % 3600) / 60;
        let secs = self.seconds % 60;

        if hours != 0 || minutes != 0 || secs != 0 || self.nanoseconds != 0 {
            write!(f, "T")?;
            if hours != 0 {
                write!(f, "{}H", hours)?;
            }
            if minutes != 0 {
                write!(f, "{}M", minutes)?;
            }
            if secs != 0 {
                write!(f, "{}S", secs)?;
            } else if self.nanoseconds != 0 {
                write!(f, "0.{:09}S", self.nanoseconds)?;
            }
        }

        // Zero duration
        if self.months == 0 && self.days == 0 && self.seconds == 0 && self.nanoseconds == 0 {
            write!(f, "0D")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_component_overflow() {
        let err = LoraDuration::parse("P9223372036854775807Y").unwrap_err();
        assert!(err.contains("Duration component overflow"));
    }

    #[test]
    fn checked_arithmetic_reports_overflow() {
        let duration = LoraDuration {
            months: i64::MAX,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        };

        assert!(duration.try_add(&duration).is_none());
        assert!(duration.try_mul_int(2).is_none());
        assert!(duration.try_negate().is_some());

        let min_duration = LoraDuration {
            months: i64::MIN,
            days: 0,
            seconds: 0,
            nanoseconds: 0,
        };
        assert!(min_duration.try_negate().is_none());
    }
}
