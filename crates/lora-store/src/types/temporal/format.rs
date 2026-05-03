//! ISO-8601 fragment formatters shared by every temporal type's
//! `Display` impl.

use std::fmt;

pub(super) fn format_offset(f: &mut fmt::Formatter<'_>, offset_seconds: i32) -> fmt::Result {
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

pub(super) fn format_subsecond(f: &mut fmt::Formatter<'_>, nanosecond: u32) -> fmt::Result {
    if nanosecond > 0 {
        let ms = nanosecond / 1_000_000;
        if ms > 0 && nanosecond.is_multiple_of(1_000_000) {
            write!(f, ".{:03}", ms)
        } else {
            write!(f, ".{:09}", nanosecond)
        }
    } else {
        Ok(())
    }
}
