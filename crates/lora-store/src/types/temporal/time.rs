//! [`LoraTime`] (zoned) and [`LoraLocalTime`] (zone-naive) wall-clock
//! times.

use std::cmp::Ordering;
use std::fmt;

use super::calendar::unix_now;
use super::datetime::split_zone_suffix;
use super::format::{format_offset, format_subsecond};
use super::parsing::parse_time_string;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoraTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
    pub offset_seconds: i32,
}

impl LoraTime {
    pub fn new(
        hour: u32,
        minute: u32,
        second: u32,
        nanosecond: u32,
        offset_seconds: i32,
    ) -> Result<Self, String> {
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
            hour,
            minute,
            second,
            nanosecond,
            offset_seconds,
        })
    }

    /// `12:00+01:00`; with a named zone (`12:00[Europe/Amsterdam]`) the
    /// offset is the zone's offset now, as in Neo4j: a time has no date to
    /// resolve daylight saving against.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (s, zone) = split_zone_suffix(s)?;
        let (h, m, sec, ns, offset) = parse_time_string(s)?;
        let offset = match (offset, zone) {
            (Some(offset), _) => offset,
            (None, Some(zone)) => zone.offset_at(unix_now().0 as i64),
            (None, None) => 0,
        };
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

impl LoraTime {
    /// Nanoseconds since midnight UTC (the offset applied, so it can fall
    /// outside one day). Comparisons and RANGE indexes order by it.
    pub fn order_nanos(&self) -> i128 {
        let secs = self.hour as i128 * 3600 + self.minute as i128 * 60 + self.second as i128;
        (secs - self.offset_seconds as i128) * 1_000_000_000 + self.nanosecond as i128
    }
}

impl PartialOrd for LoraTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraTime {
    fn cmp(&self, other: &Self) -> Ordering {
        // UTC-normalized time first; the offset only breaks ties so `Ord`
        // agrees with the field-wise `Eq`.
        self.order_nanos()
            .cmp(&other.order_nanos())
            .then(self.offset_seconds.cmp(&other.offset_seconds))
    }
}

impl fmt::Display for LoraTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        format_subsecond(f, self.nanosecond)?;
        format_offset(f, self.offset_seconds)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoraLocalTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub nanosecond: u32,
}

impl LoraLocalTime {
    pub fn new(hour: u32, minute: u32, second: u32, nanosecond: u32) -> Result<Self, String> {
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
            hour,
            minute,
            second,
            nanosecond,
        })
    }

    /// A local time has no zone: a string with an offset is refused
    /// rather than read without it.
    pub fn parse(s: &str) -> Result<Self, String> {
        match parse_time_string(s)? {
            (h, m, sec, ns, None) => Self::new(h, m, sec, ns),
            (.., Some(_)) => Err(format!("A local time has no offset: {s}")),
        }
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

impl LoraLocalTime {
    /// Nanoseconds since midnight. The total order used by comparisons
    /// and RANGE indexes.
    pub fn order_nanos(&self) -> i128 {
        let secs = self.hour as i128 * 3600 + self.minute as i128 * 60 + self.second as i128;
        secs * 1_000_000_000 + self.nanosecond as i128
    }
}

impl PartialOrd for LoraLocalTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraLocalTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order_nanos().cmp(&other.order_nanos())
    }
}

impl fmt::Display for LoraLocalTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        format_subsecond(f, self.nanosecond)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_fractional_seconds() {
        assert!(LoraLocalTime::parse("12:34:56.abc").is_err());
        assert!(LoraLocalTime::parse("12:34:56.").is_err());
    }
}
