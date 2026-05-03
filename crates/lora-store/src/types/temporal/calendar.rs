//! Calendar arithmetic and a clock helper.
//!
//! `is_leap_year` and `days_in_month` are part of the public surface
//! (re-exported through `crate::types::temporal`); the day↔civil and
//! `unix_now` helpers are crate-internal building blocks.

pub fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 => 31,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
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
pub(super) fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
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
pub(super) fn civil_from_days(z: i64) -> (i32, u32, u32) {
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
pub(super) fn unix_now() -> (u64, u32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (dur.as_secs(), dur.subsec_nanos())
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(super) fn unix_now() -> (u64, u32) {
    let ms = js_sys::Date::now();
    if !ms.is_finite() || ms < 0.0 {
        return (0, 0);
    }
    let secs = (ms / 1_000.0).floor();
    let nanos = ((ms - secs * 1_000.0) * 1_000_000.0).round();
    (secs as u64, nanos as u32)
}
