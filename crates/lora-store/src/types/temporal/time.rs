//! [`LoraTime`] (zoned) and [`LoraLocalTime`] (zone-naive) wall-clock
//! times.

use std::fmt;

use super::calendar::unix_now;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_fractional_seconds() {
        assert!(LoraLocalTime::parse("12:34:56.abc").is_err());
        assert!(LoraLocalTime::parse("12:34:56.").is_err());
    }
}
