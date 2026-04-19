use std::fmt;
use std::cmp::Ordering;

// ===== Calendar helpers =====

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 => 31,
        2 => if is_leap_year(year) { 29 } else { 28 },
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => 0,
    }
}

/// Days since 1970-01-01 (Unix epoch) from a civil date.
/// Uses Howard Hinnant's algorithms.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64 - if m <= 2 { 1 } else { 0 };
    let m = m as i64;
    let d = d as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe as i64 * 365 + yoe as i64 / 4 - yoe as i64 / 100 + doy;
    era * 146097 + doe - 719468
}

/// Civil date from days since 1970-01-01.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

/// Seconds and nanoseconds since the Unix epoch.
///
/// On native targets this reads `SystemTime::now()`; on
/// `wasm32-unknown-unknown` (where `SystemTime::now()` panics) it falls
/// back to `js_sys::Date::now()`, which returns UTC milliseconds. The
/// browser clock is millisecond-granular, so the returned nanoseconds are
/// only filled to millisecond precision on wasm32.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn unix_now() -> (u64, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (dur.as_secs(), dur.subsec_nanos())
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn unix_now() -> (u64, u32) {
    let ms = js_sys::Date::now();
    if !ms.is_finite() || ms < 0.0 {
        return (0, 0);
    }
    let secs = (ms / 1_000.0).floor();
    let nanos = ((ms - secs * 1_000.0) * 1_000_000.0).round();
    (secs as u64, nanos as u32)
}

// ===== LoraDate =====

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let year = parts[0].parse::<i32>().map_err(|_| format!("Invalid date: {s}"))?;
        let month = parts[1].parse::<u32>().map_err(|_| format!("Invalid date: {s}"))?;
        let day = parts[2].parse::<u32>().map_err(|_| format!("Invalid date: {s}"))?;
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
        Self { year: y, month: m, day: d }
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
        Self { year: self.year, month: self.month, day: 1 }
    }
}

impl PartialOrd for LoraDate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraDate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.year.cmp(&other.year)
            .then(self.month.cmp(&other.month))
            .then(self.day.cmp(&other.day))
    }
}

impl fmt::Display for LoraDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

// ===== LoraTime =====

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoraTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
    pub offset_seconds: i32,
}

impl LoraTime {
    pub fn new(hour: u32, minute: u32, second: u32, nanosecond: u32, offset_seconds: i32) -> Result<Self, String> {
        if hour > 23 { return Err(format!("Invalid hour: {hour}")); }
        if minute > 59 { return Err(format!("Invalid minute: {minute}")); }
        if second > 59 { return Err(format!("Invalid second: {second}")); }
        Ok(Self { hour, minute, second, nanosecond, offset_seconds })
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let (h, m, sec, ns, offset) = parse_time_string(s)?;
        let offset = offset.unwrap_or(0);
        Self::new(h, m, sec, ns, offset)
    }

    pub fn now() -> Self {
        let (secs, nanos) = unix_now();
        let day_secs = secs % 86400;
        Self {
            hour: (day_secs / 3600) as u32,
            minute: ((day_secs % 3600) / 60) as u32,
            second: (day_secs % 60) as u32,
            nanosecond: nanos,
            offset_seconds: 0,
        }
    }
}

impl fmt::Display for LoraTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        format_subsecond(f, self.nanosecond)?;
        format_offset(f, self.offset_seconds)
    }
}

// ===== LoraLocalTime =====

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoraLocalTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
}

impl LoraLocalTime {
    pub fn new(hour: u32, minute: u32, second: u32, nanosecond: u32) -> Result<Self, String> {
        if hour > 23 { return Err(format!("Invalid hour: {hour}")); }
        if minute > 59 { return Err(format!("Invalid minute: {minute}")); }
        if second > 59 { return Err(format!("Invalid second: {second}")); }
        Ok(Self { hour, minute, second, nanosecond })
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let (h, m, sec, ns, _) = parse_time_string(s)?;
        Self::new(h, m, sec, ns)
    }

    pub fn now() -> Self {
        let (secs, nanos) = unix_now();
        let day_secs = secs % 86400;
        Self {
            hour: (day_secs / 3600) as u32,
            minute: ((day_secs % 3600) / 60) as u32,
            second: (day_secs % 60) as u32,
            nanosecond: nanos,
        }
    }
}

impl fmt::Display for LoraLocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        format_subsecond(f, self.nanosecond)
    }
}

// ===== LoraDateTime =====

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn new(
        year: i32, month: u32, day: u32,
        hour: u32, minute: u32, second: u32, nanosecond: u32,
        offset_seconds: i32,
    ) -> Result<Self, String> {
        LoraDate::new(year, month, day)?;
        if hour > 23 { return Err(format!("Invalid hour: {hour}")); }
        if minute > 59 { return Err(format!("Invalid minute: {minute}")); }
        if second > 59 { return Err(format!("Invalid second: {second}")); }
        Ok(Self { year, month, day, hour, minute, second, nanosecond, offset_seconds })
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let t_pos = s.find('T').ok_or_else(|| format!("Invalid datetime: {s}"))?;
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
            year: y, month: mo, day: d,
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
        let base_secs = self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64 + dur.seconds;

        let total_secs = base_days * 86400 + base_secs;
        let final_days = total_secs.div_euclid(86400);
        let rem = total_secs.rem_euclid(86400);
        let (y, m, d) = civil_from_days(final_days);

        Self {
            year: y, month: m, day: d,
            hour: (rem / 3600) as u32,
            minute: ((rem % 3600) / 60) as u32,
            second: (rem % 60) as u32,
            nanosecond: self.nanosecond,
            offset_seconds: self.offset_seconds,
        }
    }

    pub fn truncate_to_day(&self) -> Self {
        Self {
            year: self.year, month: self.month, day: self.day,
            hour: 0, minute: 0, second: 0, nanosecond: 0,
            offset_seconds: self.offset_seconds,
        }
    }

    pub fn truncate_to_hour(&self) -> Self {
        Self {
            year: self.year, month: self.month, day: self.day,
            hour: self.hour, minute: 0, second: 0, nanosecond: 0,
            offset_seconds: self.offset_seconds,
        }
    }

    pub fn date(&self) -> LoraDate {
        LoraDate { year: self.year, month: self.month, day: self.day }
    }
}

impl PartialOrd for LoraDateTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.to_epoch_millis().cmp(&other.to_epoch_millis()))
    }
}

impl Ord for LoraDateTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_epoch_millis().cmp(&other.to_epoch_millis())
    }
}

impl fmt::Display for LoraDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day,
            self.hour, self.minute, self.second)?;
        format_subsecond(f, self.nanosecond)?;
        format_offset(f, self.offset_seconds)
    }
}

// ===== LoraLocalDateTime =====

#[derive(Debug, Clone, PartialEq, Eq)]
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
        let t_pos = s.find('T').ok_or_else(|| format!("Invalid localdatetime: {s}"))?;
        let date = LoraDate::parse(&s[..t_pos])?;
        let (h, m, sec, ns, _) = parse_time_string(&s[t_pos + 1..])?;
        if h > 23 { return Err(format!("Invalid hour: {h}")); }
        if m > 59 { return Err(format!("Invalid minute: {m}")); }
        if sec > 59 { return Err(format!("Invalid second: {sec}")); }
        Ok(Self {
            year: date.year, month: date.month, day: date.day,
            hour: h, minute: m, second: sec, nanosecond: ns,
        })
    }

    pub fn now() -> Self {
        let (secs, nanos) = unix_now();
        let days = (secs / 86400) as i64;
        let day_secs = secs % 86400;
        let (y, mo, d) = civil_from_days(days);
        Self {
            year: y, month: mo, day: d,
            hour: (day_secs / 3600) as u32,
            minute: ((day_secs % 3600) / 60) as u32,
            second: (day_secs % 60) as u32,
            nanosecond: nanos,
        }
    }
}

impl fmt::Display for LoraLocalDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day,
            self.hour, self.minute, self.second)?;
        format_subsecond(f, self.nanosecond)
    }
}

// ===== LoraDuration =====

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoraDuration {
    pub months: i64,
    pub days: i64,
    pub seconds: i64,
    pub nanoseconds: i64,
}

impl LoraDuration {
    pub fn zero() -> Self {
        Self { months: 0, days: 0, seconds: 0, nanoseconds: 0 }
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
                    let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                    months += n * 12;
                    num_buf.clear();
                }
                'M' if !in_time => {
                    let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                    months += n;
                    num_buf.clear();
                }
                'W' if !in_time => {
                    let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                    days += n * 7;
                    num_buf.clear();
                }
                'D' => {
                    let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                    days += n;
                    num_buf.clear();
                }
                'H' if in_time => {
                    let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                    seconds += n * 3600;
                    num_buf.clear();
                }
                'M' if in_time => {
                    let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                    seconds += n * 60;
                    num_buf.clear();
                }
                'S' if in_time => {
                    if num_buf.contains('.') {
                        let n: f64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
                        seconds += n.floor() as i64;
                        let frac = n - n.floor();
                        if frac > 0.0 {
                            nanoseconds += (frac * 1_000_000_000.0) as i64;
                        }
                    } else {
                        let n: i64 = num_buf.parse().map_err(|_| format!("Invalid duration: {s}"))?;
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

        Ok(Self { months, days, seconds, nanoseconds })
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
        if n == 0 { return Self::zero(); }
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
        let intermediate = earlier.add_duration(&LoraDuration { months, days: 0, seconds: 0, nanoseconds: 0 });
        if intermediate.to_epoch_days() > later.to_epoch_days() {
            months -= 1;
        }
        let intermediate = earlier.add_duration(&LoraDuration { months, days: 0, seconds: 0, nanoseconds: 0 });
        let remaining_days = later.to_epoch_days() - intermediate.to_epoch_days();

        Self { months: months * sign, days: remaining_days * sign, seconds: 0, nanoseconds: 0 }
    }

    /// Duration from date1 to date2 expressed purely in days.
    pub fn in_days(from: &LoraDate, to: &LoraDate) -> Self {
        let days = to.to_epoch_days() - from.to_epoch_days();
        Self { months: 0, days, seconds: 0, nanoseconds: 0 }
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

    pub fn years_component(&self) -> i64 { self.months / 12 }
    pub fn months_component(&self) -> i64 { self.months % 12 }
    pub fn days_component(&self) -> i64 { self.days }
    pub fn hours_component(&self) -> i64 { self.seconds / 3600 }
    pub fn minutes_component(&self) -> i64 { (self.seconds % 3600) / 60 }
    pub fn seconds_component(&self) -> i64 { self.seconds % 60 }
}

impl PartialOrd for LoraDuration {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.total_seconds_approx().partial_cmp(&other.total_seconds_approx())
    }
}

impl Ord for LoraDuration {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap_or(Ordering::Equal)
    }
}

impl fmt::Display for LoraDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P")?;
        let years = self.months / 12;
        let months = self.months % 12;
        if years != 0 { write!(f, "{}Y", years)?; }
        if months != 0 { write!(f, "{}M", months)?; }
        if self.days != 0 { write!(f, "{}D", self.days)?; }

        let hours = self.seconds / 3600;
        let minutes = (self.seconds % 3600) / 60;
        let secs = self.seconds % 60;

        if hours != 0 || minutes != 0 || secs != 0 || self.nanoseconds != 0 {
            write!(f, "T")?;
            if hours != 0 { write!(f, "{}H", hours)?; }
            if minutes != 0 { write!(f, "{}M", minutes)?; }
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

// ===== Parsing helpers =====

/// Parse a time string returning (hour, minute, second, nanosecond, optional offset_seconds).
fn parse_time_string(s: &str) -> Result<(u32, u32, u32, u32, Option<i32>), String> {
    // Find offset suffix: Z, +HH:MM, -HH:MM
    let (time_str, offset) = if s.ends_with('Z') {
        (&s[..s.len() - 1], Some(0i32))
    } else if let Some(pos) = s.rfind('+') {
        if pos >= 2 {
            let off = parse_offset(&s[pos..])?;
            (&s[..pos], Some(off))
        } else {
            (s, None)
        }
    } else {
        // Look for a '-' that is part of an offset (after HH:MM:SS portion)
        // Time format is at least HH:MM = 5 chars
        let search_start = 5.min(s.len());
        if let Some(rel_pos) = s[search_start..].rfind('-') {
            let pos = search_start + rel_pos;
            let off = parse_offset(&s[pos..])?;
            (&s[..pos], Some(off))
        } else {
            (s, None)
        }
    };

    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return Err(format!("Invalid time: {s}"));
    }

    let hour = parts[0].parse::<u32>().map_err(|_| format!("Invalid time: {s}"))?;
    let minute = parts[1].parse::<u32>().map_err(|_| format!("Invalid time: {s}"))?;

    let (second, nanosecond) = if parts.len() == 3 {
        parse_seconds_and_fraction(parts[2])?
    } else {
        (0, 0)
    };

    Ok((hour, minute, second, nanosecond, offset))
}

fn parse_seconds_and_fraction(s: &str) -> Result<(u32, u32), String> {
    if let Some(dot_pos) = s.find('.') {
        let sec = s[..dot_pos].parse::<u32>().map_err(|_| format!("Invalid seconds: {s}"))?;
        let frac = &s[dot_pos + 1..];
        // Pad/truncate to 9 digits for nanoseconds
        let padded = format!("{:0<9}", frac);
        let ns = padded[..9].parse::<u32>().unwrap_or(0);
        Ok((sec, ns))
    } else {
        let sec = s.parse::<u32>().map_err(|_| format!("Invalid seconds: {s}"))?;
        Ok((sec, 0))
    }
}

fn parse_offset(s: &str) -> Result<i32, String> {
    let sign = if s.starts_with('+') { 1 } else if s.starts_with('-') { -1 } else {
        return Err(format!("Invalid offset: {s}"));
    };
    let rest = &s[1..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid offset: {s}"));
    }
    let h = parts[0].parse::<i32>().map_err(|_| format!("Invalid offset: {s}"))?;
    let m = parts[1].parse::<i32>().map_err(|_| format!("Invalid offset: {s}"))?;
    Ok(sign * (h * 3600 + m * 60))
}

fn format_offset(f: &mut fmt::Formatter<'_>, offset_seconds: i32) -> fmt::Result {
    if offset_seconds == 0 {
        write!(f, "Z")
    } else {
        let sign = if offset_seconds >= 0 { '+' } else { '-' };
        let abs = offset_seconds.unsigned_abs();
        let h = abs / 3600;
        let m = (abs % 3600) / 60;
        write!(f, "{}{:02}:{:02}", sign, h, m)
    }
}

fn format_subsecond(f: &mut fmt::Formatter<'_>, nanosecond: u32) -> fmt::Result {
    if nanosecond > 0 {
        let ms = nanosecond / 1_000_000;
        if ms > 0 && nanosecond % 1_000_000 == 0 {
            write!(f, ".{:03}", ms)
        } else {
            write!(f, ".{:09}", nanosecond)
        }
    } else {
        Ok(())
    }
}
