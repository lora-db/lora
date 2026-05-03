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
                    months += n * 12;
                    num_buf.clear();
                }
                'M' if !in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    months += n;
                    num_buf.clear();
                }
                'W' if !in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    days += n * 7;
                    num_buf.clear();
                }
                'D' => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    days += n;
                    num_buf.clear();
                }
                'H' if in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    seconds += n * 3600;
                    num_buf.clear();
                }
                'M' if in_time => {
                    let n: i64 = num_buf
                        .parse()
                        .map_err(|_| format!("Invalid duration: {s}"))?;
                    seconds += n * 60;
                    num_buf.clear();
                }
                'S' if in_time => {
                    if num_buf.contains('.') {
                        let n: f64 = num_buf
                            .parse()
                            .map_err(|_| format!("Invalid duration: {s}"))?;
                        seconds += n.floor() as i64;
                        let frac = n - n.floor();
                        if frac > 0.0 {
                            nanoseconds += (frac * 1_000_000_000.0) as i64;
                        }
                    } else {
                        let n: i64 = num_buf
                            .parse()
                            .map_err(|_| format!("Invalid duration: {s}"))?;
                        seconds += n;
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
        Self {
            months: -self.months,
            days: -self.days,
            seconds: -self.seconds,
            nanoseconds: -self.nanoseconds,
        }
    }

    pub fn add(&self, other: &Self) -> Self {
        Self {
            months: self.months + other.months,
            days: self.days + other.days,
            seconds: self.seconds + other.seconds,
            nanoseconds: self.nanoseconds + other.nanoseconds,
        }
    }

    pub fn mul_int(&self, n: i64) -> Self {
        Self {
            months: self.months * n,
            days: self.days * n,
            seconds: self.seconds * n,
            nanoseconds: self.nanoseconds * n,
        }
    }

    pub fn div_int(&self, n: i64) -> Self {
        if n == 0 {
            return Self::zero();
        }
        Self {
            months: self.months / n,
            days: self.days / n,
            seconds: self.seconds / n,
            nanoseconds: self.nanoseconds / n,
        }
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
