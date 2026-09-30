//! [`LoraDateTime`] (zoned) and [`LoraLocalDateTime`] (zone-naive)
//! combined date + time values.

use std::cmp::Ordering;
use std::fmt;

use super::calendar::{civil_from_days, days_from_civil, days_in_month, unix_now};
use super::date::LoraDate;
use super::duration::LoraDuration;
use super::format::{format_offset, format_subsecond};
use super::parsing::parse_time_string;
use super::zone::{LocalOffset, ZoneId};

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
    /// The named zone (`Europe/Amsterdam`) the value is in, if any.
    /// `offset_seconds` is always the zone's offset at this instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<ZoneId>,
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
            zone: None,
        })
    }

    /// `2026-07-01T12:00:00+02:00`, and with a named zone
    /// `2026-07-01T12:00:00+02:00[Europe/Amsterdam]` or
    /// `2026-07-01T12:00:00[Europe/Amsterdam]` (the zone gives the offset).
    /// An offset the zone does not have at that instant is an error.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (s, zone) = split_zone_suffix(s)?;
        let t_pos = s
            .find('T')
            .ok_or_else(|| format!("Invalid datetime: {s}"))?;
        let date_part = &s[..t_pos];
        let time_part = &s[t_pos + 1..];

        let date = LoraDate::parse(date_part)?;
        let (h, m, sec, ns, offset) = parse_time_string(time_part)?;
        match (zone, offset) {
            (None, offset) => Self::new(
                date.year,
                date.month,
                date.day,
                h,
                m,
                sec,
                ns,
                offset.unwrap_or(0),
            ),
            (Some(zone), None) => Self::in_zone(&date, h, m, sec, ns, zone, None),
            (Some(zone), Some(offset)) => {
                let dt = Self::new(date.year, date.month, date.day, h, m, sec, ns, offset)?;
                if zone.offset_at(dt.epoch_seconds()) != offset {
                    return Err(format!("The offset does not match {zone}: {s}"));
                }
                Ok(Self {
                    zone: Some(zone),
                    ..dt
                })
            }
        }
    }

    /// The local date-time in `zone`, at the offset the zone has then. A
    /// local time in a daylight-saving gap moves forward by the length of
    /// the gap; one in an overlap takes the earlier offset, unless
    /// `prefer` is the other one (as when arithmetic keeps an offset that
    /// is still valid). This is how Neo4j (java.time) resolves them.
    #[allow(clippy::too_many_arguments)] // Structural datetime constructor.
    pub fn in_zone(
        date: &LoraDate,
        hour: u32,
        minute: u32,
        second: u32,
        nanosecond: u32,
        zone: ZoneId,
        prefer: Option<i32>,
    ) -> Result<Self, String> {
        let (y, mo, d) = (date.year, date.month, date.day);
        let offset = match zone.local_offset(y, mo, d, hour, minute, second) {
            LocalOffset::Unique(offset) => offset,
            LocalOffset::Overlap { earlier, later } => {
                if prefer == Some(later) {
                    later
                } else {
                    earlier
                }
            }
            LocalOffset::Gap { before, after } => {
                // The instant the local time names at the old offset, shown
                // at the new one: 02:30 in a 02:00 to 03:00 gap is 03:30.
                let dt = Self::new(y, mo, d, hour, minute, second, nanosecond, before)?;
                let moved = dt
                    .shift_local_seconds((after - before) as i64)
                    .ok_or("datetime out of range")?;
                return Ok(Self {
                    offset_seconds: after,
                    zone: Some(zone),
                    ..moved
                });
            }
        };
        Ok(Self {
            zone: Some(zone),
            ..Self::new(y, mo, d, hour, minute, second, nanosecond, offset)?
        })
    }

    /// The same instant, shown in `zone`.
    pub fn to_zone(&self, zone: ZoneId) -> Option<Self> {
        let offset = zone.offset_at(self.epoch_seconds());
        let moved = self.shift_local_seconds(offset as i64 - self.offset_seconds as i64)?;
        Some(Self {
            offset_seconds: offset,
            zone: Some(zone),
            ..moved
        })
    }

    /// The same instant at a fixed `offset`, without a named zone.
    pub fn to_offset(&self, offset: i32) -> Option<Self> {
        let moved = self.shift_local_seconds(offset as i64 - self.offset_seconds as i64)?;
        Some(Self {
            offset_seconds: offset,
            zone: None,
            ..moved
        })
    }

    /// Whole seconds since the Unix epoch (UTC).
    pub fn epoch_seconds(&self) -> i64 {
        let days = days_from_civil(self.year, self.month, self.day);
        days * 86_400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
            - self.offset_seconds as i64
    }

    /// The wall clock moved by `seconds`, offset and zone unchanged.
    fn shift_local_seconds(&self, seconds: i64) -> Option<Self> {
        self.add_fixed(&LoraDuration {
            seconds,
            ..LoraDuration::zero()
        })
    }

    /// This value with the clock set to the given fields: re-resolved in
    /// its zone, keeping the offset when it is still valid.
    fn with_clock(&self, hour: u32, minute: u32, second: u32, nanosecond: u32) -> Self {
        let fixed = Self {
            hour,
            minute,
            second,
            nanosecond,
            ..self.clone()
        };
        match self.zone {
            Some(zone) => Self::in_zone(
                &self.date(),
                hour,
                minute,
                second,
                nanosecond,
                zone,
                Some(self.offset_seconds),
            )
            .unwrap_or(fixed),
            None => fixed,
        }
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
            zone: None,
        }
    }

    /// Nanoseconds since the Unix epoch, normalized to UTC: the instant
    /// this value denotes. Comparisons and RANGE indexes order by it.
    pub fn order_nanos(&self) -> i128 {
        let days = days_from_civil(self.year, self.month, self.day) as i128;
        let day_secs = self.hour as i128 * 3600 + self.minute as i128 * 60 + self.second as i128;
        (days * 86_400 + day_secs - self.offset_seconds as i128) * 1_000_000_000
            + self.nanosecond as i128
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

    /// In a named zone, the months and days move the wall clock and the
    /// result is re-resolved in the zone (keeping the offset if it is still
    /// valid); the seconds and nanoseconds then move the instant. This is
    /// Neo4j's (java.time's) `ZonedDateTime.plus`.
    pub fn try_add_duration(&self, dur: &LoraDuration) -> Option<Self> {
        let Some(zone) = self.zone else {
            return self.add_fixed(dur);
        };
        let mut out = self.clone();
        if dur.months != 0 || dur.days != 0 {
            let local = self.add_fixed(&LoraDuration {
                months: dur.months,
                days: dur.days,
                ..LoraDuration::zero()
            })?;
            out = Self::in_zone(
                &local.date(),
                local.hour,
                local.minute,
                local.second,
                local.nanosecond,
                zone,
                Some(self.offset_seconds),
            )
            .ok()?;
        }
        if dur.seconds != 0 || dur.nanoseconds != 0 {
            out = out
                .add_fixed(&LoraDuration {
                    seconds: dur.seconds,
                    nanoseconds: dur.nanoseconds,
                    ..LoraDuration::zero()
                })?
                .to_zone(zone)?;
        }
        Some(out)
    }

    /// `dur` added to the wall clock at a fixed offset (and zone).
    fn add_fixed(&self, dur: &LoraDuration) -> Option<Self> {
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
            zone: self.zone,
        })
    }

    pub fn truncate_to_day(&self) -> Self {
        self.with_clock(0, 0, 0, 0)
    }

    pub fn truncate_to_hour(&self) -> Self {
        self.with_clock(self.hour, 0, 0, 0)
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
            zone: None,
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
            zone: None,
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
        // Instant first; the offset and zone only break ties so `Ord`
        // agrees with the field-wise `Eq`.
        self.order_nanos()
            .cmp(&other.order_nanos())
            .then(self.offset_seconds.cmp(&other.offset_seconds))
            .then(self.zone.cmp(&other.zone))
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
        format_offset(f, self.offset_seconds)?;
        match self.zone {
            Some(zone) => write!(f, "[{zone}]"),
            None => Ok(()),
        }
    }
}

/// Split a trailing `[Region/City]` zone off a datetime or time string.
pub(super) fn split_zone_suffix(s: &str) -> Result<(&str, Option<ZoneId>), String> {
    let Some(body) = s.strip_suffix(']') else {
        return Ok((s, None));
    };
    let open = body
        .rfind('[')
        .ok_or_else(|| format!("Invalid time zone in: {s}"))?;
    let name = &body[open + 1..];
    let zone = ZoneId::lookup(name).ok_or_else(|| format!("Unknown time zone `{name}`"))?;
    Ok((&body[..open], Some(zone)))
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
        // A local datetime has no zone: an offset is refused rather than
        // dropped.
        let (h, m, sec, ns, None) = parse_time_string(&s[t_pos + 1..])? else {
            return Err(format!("A local datetime has no offset: {s}"));
        };
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

impl LoraLocalDateTime {
    /// Nanoseconds since the Unix epoch, reading the wall clock as UTC.
    /// The total order used by comparisons and RANGE indexes.
    pub fn order_nanos(&self) -> i128 {
        let days = days_from_civil(self.year, self.month, self.day) as i128;
        let day_secs = self.hour as i128 * 3600 + self.minute as i128 * 60 + self.second as i128;
        (days * 86_400 + day_secs) * 1_000_000_000 + self.nanosecond as i128
    }
}

impl PartialOrd for LoraLocalDateTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LoraLocalDateTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order_nanos().cmp(&other.order_nanos())
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
            zone: None,
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
            zone: None,
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

    fn dt(s: &str) -> String {
        LoraDateTime::parse(s)
            .map(|d| d.to_string())
            .unwrap_or_else(|e| e)
    }

    #[test]
    fn named_zones_parse_and_print() {
        for (input, want) in [
            (
                "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
                "2026-07-01T12:00:00+02:00[Europe/Amsterdam]",
            ),
            (
                "2026-01-15T12:00[Europe/Amsterdam]",
                "2026-01-15T12:00:00+01:00[Europe/Amsterdam]",
            ),
            (
                "2026-07-01T12:00[Europe/London]",
                "2026-07-01T12:00:00+01:00[Europe/London]",
            ),
            (
                "2026-01-15T12:00[europe/london]",
                "2026-01-15T12:00:00Z[Europe/London]",
            ),
            (
                "2026-07-01T12:00[Asia/Kolkata]",
                "2026-07-01T12:00:00+05:30[Asia/Kolkata]",
            ),
            // A gap moves forward, an overlap takes the earlier offset.
            (
                "2026-03-29T02:30[Europe/Amsterdam]",
                "2026-03-29T03:30:00+02:00[Europe/Amsterdam]",
            ),
            (
                "2026-10-25T02:30[Europe/Amsterdam]",
                "2026-10-25T02:30:00+02:00[Europe/Amsterdam]",
            ),
            (
                "2026-10-25T02:30+01:00[Europe/Amsterdam]",
                "2026-10-25T02:30:00+01:00[Europe/Amsterdam]",
            ),
        ] {
            assert_eq!(dt(input), want, "{input}");
            // The printed form parses back to the same value.
            let v = LoraDateTime::parse(want).unwrap();
            assert_eq!(LoraDateTime::parse(&v.to_string()).unwrap(), v);
        }
        assert!(LoraDateTime::parse("2026-07-01T12:00+01:00[Europe/Amsterdam]").is_err());
        assert!(LoraDateTime::parse("2026-07-01T12:00[Mars/Olympus]").is_err());
    }

    #[test]
    fn arithmetic_keeps_the_zone_across_dst() {
        let add = |s: &str, dur: &str| {
            LoraDateTime::parse(s)
                .unwrap()
                .try_add_duration(&LoraDuration::parse(dur).unwrap())
                .unwrap()
                .to_string()
        };
        // A day is a calendar day: same wall clock after the spring jump.
        assert_eq!(
            add("2026-03-28T12:00[Europe/Amsterdam]", "P1D"),
            "2026-03-29T12:00:00+02:00[Europe/Amsterdam]"
        );
        // 24 hours is 24 hours on the clock of the instant.
        assert_eq!(
            add("2026-03-28T12:00[Europe/Amsterdam]", "PT24H"),
            "2026-03-29T13:00:00+02:00[Europe/Amsterdam]"
        );
        assert_eq!(
            add("2026-10-25T01:30[Europe/Amsterdam]", "PT1H"),
            "2026-10-25T02:30:00+02:00[Europe/Amsterdam]"
        );
        assert_eq!(
            add("2026-10-25T02:30+02:00[Europe/Amsterdam]", "PT1H"),
            "2026-10-25T02:30:00+01:00[Europe/Amsterdam]"
        );
    }
}
