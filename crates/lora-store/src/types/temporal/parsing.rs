//! ISO-8601 fragment parsers shared by [`super::date::LoraDate`],
//! [`super::time::LoraTime`], [`super::time::LoraLocalTime`],
//! [`super::datetime::LoraDateTime`], and
//! [`super::datetime::LoraLocalDateTime`].
//!
//! Every form Cypher accepts, in the extended and the basic format:
//!
//! * dates: `YYYY-MM-DD` / `YYYYMMDD`, `YYYY-MM` / `YYYYMM`, `YYYY`,
//!   `YYYY-Www-D` / `YYYYWwwD`, `YYYY-Www` / `YYYYWww`, `YYYY-DDD` /
//!   `YYYYDDD`, `YYYY-Qq-DD` / `YYYYQqDD`, `YYYY-Qq` / `YYYYQq`. The
//!   extended format also takes a signed or longer year (`-0044-03-15`,
//!   `12345-01-01`).
//! * times: `HH:MM:SS.f` / `HHMMSS.f`, `HH:MM:SS` / `HHMMSS`, `HH:MM` /
//!   `HHMM`, `HH`, each with an optional offset `Z`, `±HH:MM`, `±HHMM` or
//!   `±HH`.
//!
//! Fields are fixed width, so `2015-7-21` and `9:00` are refused.

use super::date::LoraDate;

/// Parse a date in any of the ISO forms above.
pub(super) fn parse_date(s: &str) -> Result<LoraDate, String> {
    let b = s.as_bytes();
    let err = || format!("Invalid date: {s}");
    // Fast path for the common `YYYY-MM-DD`.
    if b.len() == 10 && b[4] == b'-' && b[7] == b'-' {
        if let (Some(year), Some(month), Some(day)) =
            (digits(&b[0..4]), digits(&b[5..7]), digits(&b[8..10]))
        {
            return LoraDate::new(year as i32, month, day);
        }
    }

    let (negative, unsigned) = match b.first() {
        Some(b'+') => (false, &b[1..]),
        Some(b'-') => (true, &b[1..]),
        _ => (false, b),
    };
    let signed = unsigned.len() != b.len();
    let run = unsigned.iter().take_while(|c| c.is_ascii_digit()).count();
    let (head, rest) = unsigned.split_at(run);
    let year_of = |digits_: &[u8]| -> Result<i32, String> {
        let y = digits_
            .iter()
            .try_fold(0i64, |acc, d| {
                acc.checked_mul(10)?.checked_add((d - b'0') as i64)
            })
            .and_then(|y| i32::try_from(if negative { -y } else { y }).ok())
            .ok_or_else(err)?;
        Ok(y)
    };

    match rest.first() {
        // Basic, digits only: YYYY, YYYYMM, YYYYDDD, YYYYMMDD.
        None if !signed => match run {
            4 => LoraDate::new(year_of(head)?, 1, 1),
            6 => LoraDate::new(year_of(&head[..4])?, digits(&head[4..]).ok_or_else(err)?, 1),
            7 => LoraDate::from_ordinal(year_of(&head[..4])?, digits(&head[4..]).ok_or_else(err)?),
            8 => LoraDate::new(
                year_of(&head[..4])?,
                digits(&head[4..6]).ok_or_else(err)?,
                digits(&head[6..]).ok_or_else(err)?,
            ),
            _ => Err(err()),
        },
        None if run >= 1 => LoraDate::new(year_of(head)?, 1, 1),
        // Extended: YYYY-…, the year possibly signed or longer.
        Some(b'-') if run >= 4 || (signed && run >= 1) => {
            let year = year_of(head)?;
            let rest = &rest[1..];
            match rest.first() {
                Some(b'W') => week_date(year, &rest[1..], true).ok_or_else(err)?,
                Some(b'Q') => quarter_date(year, &rest[1..], true).ok_or_else(err)?,
                _ => match rest {
                    [m1, m2] => LoraDate::new(year, digits(&[*m1, *m2]).ok_or_else(err)?, 1),
                    [m1, m2, b'-', d1, d2] => LoraDate::new(
                        year,
                        digits(&[*m1, *m2]).ok_or_else(err)?,
                        digits(&[*d1, *d2]).ok_or_else(err)?,
                    ),
                    [d1, d2, d3] => {
                        LoraDate::from_ordinal(year, digits(&[*d1, *d2, *d3]).ok_or_else(err)?)
                    }
                    _ => Err(err()),
                },
            }
        }
        // Basic week and quarter dates: YYYYWwwD, YYYYQqDD.
        Some(b'W') if run == 4 && !signed => {
            week_date(year_of(head)?, &rest[1..], false).ok_or_else(err)?
        }
        Some(b'Q') if run == 4 && !signed => {
            quarter_date(year_of(head)?, &rest[1..], false).ok_or_else(err)?
        }
        _ => Err(err()),
    }
}

/// `ww[-D]` (extended) or `ww[D]` (basic) after the `W`.
fn week_date(year: i32, s: &[u8], extended: bool) -> Option<Result<LoraDate, String>> {
    let (week, dow) = match (s, extended) {
        ([w1, w2], _) => (digits(&[*w1, *w2])?, 1),
        ([w1, w2, b'-', d], true) | ([w1, w2, d], false) => (digits(&[*w1, *w2])?, digits(&[*d])?),
        _ => return None,
    };
    Some(LoraDate::from_iso_week(year, week, dow))
}

/// `q[-DD]` (extended) or `q[DD]` (basic) after the `Q`.
fn quarter_date(year: i32, s: &[u8], extended: bool) -> Option<Result<LoraDate, String>> {
    let (quarter, day) = match (s, extended) {
        ([q], _) => (digits(&[*q])?, 1),
        ([q, b'-', d1, d2], true) | ([q, d1, d2], false) => (digits(&[*q])?, digits(&[*d1, *d2])?),
        _ => return None,
    };
    Some(LoraDate::from_quarter(year, quarter, day))
}

/// A run of ASCII digits (at most 9) as a number.
fn digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() || b.len() > 9 {
        return None;
    }
    b.iter().try_fold(0u32, |acc, d| {
        if d.is_ascii_digit() {
            Some(acc * 10 + (d - b'0') as u32)
        } else {
            None
        }
    })
}

/// Parse a time string returning (hour, minute, second, nanosecond,
/// optional offset_seconds). The clock is not range-checked here.
pub(super) fn parse_time_string(s: &str) -> Result<(u32, u32, u32, u32, Option<i32>), String> {
    let err = || format!("Invalid time: {s}");
    if !s.is_ascii() {
        return Err(err());
    }
    // A clock never contains a sign, so the first one starts the offset.
    let (clock, offset) = if let Some(stripped) = s.strip_suffix('Z') {
        (stripped, Some(0i32))
    } else if let Some(pos) = s.find(['+', '-']) {
        (&s[..pos], Some(parse_offset(&s[pos..])?))
    } else {
        (s, None)
    };

    let b = clock.as_bytes();
    let (hms, fraction) = match b.iter().position(|&c| c == b'.') {
        Some(dot) => (&b[..dot], Some(&b[dot + 1..])),
        None => (b, None),
    };
    let (hour, minute, second) = match hms {
        [h1, h2] => (digits(&[*h1, *h2]), Some(0), Some(0)),
        [h1, h2, m1, m2] | [h1, h2, b':', m1, m2] => {
            (digits(&[*h1, *h2]), digits(&[*m1, *m2]), Some(0))
        }
        [h1, h2, m1, m2, s1, s2] | [h1, h2, b':', m1, m2, b':', s1, s2] => (
            digits(&[*h1, *h2]),
            digits(&[*m1, *m2]),
            digits(&[*s1, *s2]),
        ),
        _ => return Err(err()),
    };
    let (Some(hour), Some(minute), Some(second)) = (hour, minute, second) else {
        return Err(err());
    };
    // A fraction only follows the seconds.
    let nanosecond = match fraction {
        None => 0,
        Some(f) if hms.len() >= 6 && !f.is_empty() && f.iter().all(u8::is_ascii_digit) => {
            // Nanosecond precision; further digits are dropped.
            f.iter()
                .chain(std::iter::repeat(&b'0'))
                .take(9)
                .fold(0u32, |acc, d| acc * 10 + (d - b'0') as u32)
        }
        Some(_) => return Err(format!("Invalid fractional seconds: {s}")),
    };

    Ok((hour, minute, second, nanosecond, offset))
}

/// `±HH:MM`, `±HHMM` or `±HH`, at most 18 hours.
fn parse_offset(s: &str) -> Result<i32, String> {
    let err = || format!("Invalid offset: {s}");
    let b = s.as_bytes();
    let sign = match b.first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(err()),
    };
    let (h, m) = match &b[1..] {
        [h1, h2] => (digits(&[*h1, *h2]), Some(0)),
        [h1, h2, m1, m2] | [h1, h2, b':', m1, m2] => (digits(&[*h1, *h2]), digits(&[*m1, *m2])),
        _ => return Err(err()),
    };
    match (h, m) {
        (Some(h), Some(m)) if m < 60 && h * 3600 + m * 60 <= 18 * 3600 => {
            Ok(sign * (h * 3600 + m * 60) as i32)
        }
        _ => Err(err()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(s: &str) -> String {
        parse_date(s).map(|d| d.to_string()).unwrap_or_else(|e| e)
    }

    #[test]
    fn dates_in_every_form() {
        for (s, want) in [
            ("2015-07-21", "2015-07-21"),
            ("20150721", "2015-07-21"),
            ("2015-07", "2015-07-01"),
            ("201507", "2015-07-01"),
            ("2015", "2015-01-01"),
            ("2015-W30-2", "2015-07-21"),
            ("2015W302", "2015-07-21"),
            ("2015-W30", "2015-07-20"),
            ("2015W30", "2015-07-20"),
            ("2015-202", "2015-07-21"),
            ("2015202", "2015-07-21"),
            ("2015-Q3", "2015-07-01"),
            ("2015-Q3-21", "2015-07-21"),
            ("2015Q321", "2015-07-21"),
            ("2015Q3", "2015-07-01"),
            ("12345-01-02", "12345-01-02"),
            ("-0044-03-15", "-044-03-15"),
        ] {
            assert_eq!(date(s), want, "{s}");
        }
        for s in [
            "",
            "2015-7-21",
            "15-07-21",
            "2015-07-21x",
            "2015-W54-1",
            "2015-W30-0",
            "2015-366",
            "2015-13",
            "2015-Q5",
            "2015-Q1-92",
            "20150",
            "2015W3-2",
            "2015-W302",
            "-2015W302",
        ] {
            assert!(parse_date(s).is_err(), "{s}");
        }
    }

    #[test]
    fn times_in_every_form() {
        for (s, want) in [
            ("21:40:32.142+0100", (21, 40, 32, 142_000_000, Some(3600))),
            ("214032.142+01:00", (21, 40, 32, 142_000_000, Some(3600))),
            ("214032-0130", (21, 40, 32, 0, Some(-5400))),
            ("21:40:32+01", (21, 40, 32, 0, Some(3600))),
            ("2140Z", (21, 40, 0, 0, Some(0))),
            ("21", (21, 0, 0, 0, None)),
            ("21:40:32.1234567891", (21, 40, 32, 123_456_789, None)),
        ] {
            assert_eq!(parse_time_string(s), Ok(want), "{s}");
        }
        for s in [
            "21:4",
            "2140:32",
            "21:40+1",
            "214",
            "21:40.5",
            "21:40:32.",
            "21:40:32+19:00",
            "21:40:32+01:60",
            "é",
        ] {
            assert!(parse_time_string(s).is_err(), "{s}");
        }
    }
}
