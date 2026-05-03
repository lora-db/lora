//! Point map decoder and the named-timezone offset table used by the
//! `point()` and `datetime()` constructors in
//! [`super::functions::eval_function`].

use std::collections::BTreeMap;

use lora_store::{resolve_srid, srid_is_3d, LoraPoint, PointKeyFamily};

use crate::value::LoraValue;

/// Parse a `point(map)` argument into a `LoraPoint`.
///
/// - `Ok(Some(p))` → construction succeeded.
/// - `Ok(None)`    → null propagation: the map contained a null on one of the
///   recognised coordinate/crs/srid keys, so the call should
///   return `null` *without* signalling an error.
/// - `Err(msg)`    → validation failure (unknown key, bad type, conflicting
///   crs/srid, dimensionality mismatch, missing coords, …).
pub(super) fn build_point_from_map(
    map: &BTreeMap<String, LoraValue>,
) -> Result<Option<LoraPoint>, String> {
    const KNOWN_KEYS: &[&str] = &[
        "x",
        "y",
        "z",
        "longitude",
        "latitude",
        "height",
        "crs",
        "srid",
    ];

    // Reject unknown keys up front — strictness is preferred over silently
    // ignoring typos like `{lon: 4, lat: 52}`.
    for k in map.keys() {
        if !KNOWN_KEYS.iter().any(|known| known.eq_ignore_ascii_case(k)) {
            return Err(format!("point() got unknown key '{k}'"));
        }
    }

    // Pull every recognised coordinate slot. `Some(None)` means the key was
    // present but held a null (→ null propagation); `None` means absent.
    let x = take_numeric(map, "x")?;
    let y = take_numeric(map, "y")?;
    let z = take_numeric(map, "z")?;
    let longitude = take_numeric(map, "longitude")?;
    let latitude = take_numeric(map, "latitude")?;
    let height = take_numeric(map, "height")?;
    let crs = take_string(map, "crs")?;
    let srid = take_integer(map, "srid")?;

    // Null propagation: any null on a recognised key → return null.
    if matches!(x, Some(None))
        || matches!(y, Some(None))
        || matches!(z, Some(None))
        || matches!(longitude, Some(None))
        || matches!(latitude, Some(None))
        || matches!(height, Some(None))
        || matches!(crs, Some(None))
        || matches!(srid, Some(None))
    {
        return Ok(None);
    }

    // Flatten `Option<Option<T>>` now that null-propagation is resolved.
    let x = x.and_then(|v| v);
    let y = y.and_then(|v| v);
    let z = z.and_then(|v| v);
    let longitude = longitude.and_then(|v| v);
    let latitude = latitude.and_then(|v| v);
    let height = height.and_then(|v| v);
    let crs = crs.and_then(|v| v);
    let srid = srid.and_then(|v| v);

    // Detect coordinate family. Mixing x/y with longitude/latitude is
    // ambiguous and rejected.
    let has_cartesian = x.is_some() || y.is_some();
    let has_geographic = longitude.is_some() || latitude.is_some();
    if has_cartesian && has_geographic {
        return Err(
            "point() cannot mix cartesian (x/y) and geographic (longitude/latitude) keys"
                .to_string(),
        );
    }

    let (family, first, second) = if has_geographic {
        (
            PointKeyFamily::Geographic,
            longitude.ok_or_else(|| "point() is missing longitude".to_string())?,
            latitude.ok_or_else(|| "point() is missing latitude".to_string())?,
        )
    } else if has_cartesian {
        (
            PointKeyFamily::Cartesian,
            x.ok_or_else(|| "point() is missing x".to_string())?,
            y.ok_or_else(|| "point() is missing y".to_string())?,
        )
    } else {
        return Err(
            "point() requires coordinates — either {x, y} or {longitude, latitude}".to_string(),
        );
    };

    // Third dimension. `z` and `height` are aliases; specifying both is an
    // error even if they agree, to keep the input unambiguous.
    let third = match (z, height) {
        (Some(_), Some(_)) => {
            return Err(
                "point() cannot specify both 'z' and 'height' — they are aliases".to_string(),
            );
        }
        (Some(v), None) | (None, Some(v)) => Some(v),
        (None, None) => None,
    };

    let is_3d = third.is_some();
    let final_srid = resolve_srid(crs.as_deref(), srid, family, is_3d)?;

    // Construct — we use the raw struct to avoid re-deriving 2D-vs-3D via
    // the convenience constructors once we already have the resolved SRID.
    let point = LoraPoint {
        x: first,
        y: second,
        z: if srid_is_3d(final_srid) { third } else { None },
        srid: final_srid,
    };

    Ok(Some(point))
}

/// Fetch a numeric slot from a `point()` map.
///
/// Returns:
/// - `Ok(None)`              → key absent.
/// - `Ok(Some(None))`        → key present with `null` (null-propagate).
/// - `Ok(Some(Some(n)))`     → numeric value; `Int`s are coerced to `f64`.
/// - `Err(msg)`              → present but not numeric / not null.
fn take_numeric(
    map: &BTreeMap<String, LoraValue>,
    key: &str,
) -> Result<Option<Option<f64>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(LoraValue::Null) => Ok(Some(None)),
        Some(LoraValue::Int(v)) => Ok(Some(Some(*v as f64))),
        Some(LoraValue::Float(v)) => Ok(Some(Some(*v))),
        Some(other) => Err(format!(
            "point() field '{key}' must be numeric, got {}",
            crate::errors::value_kind(other)
        )),
    }
}

fn take_string(
    map: &BTreeMap<String, LoraValue>,
    key: &str,
) -> Result<Option<Option<String>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(LoraValue::Null) => Ok(Some(None)),
        Some(LoraValue::String(s)) => Ok(Some(Some(s.clone()))),
        Some(other) => Err(format!(
            "point() field '{key}' must be a string, got {}",
            crate::errors::value_kind(other)
        )),
    }
}

fn take_integer(
    map: &BTreeMap<String, LoraValue>,
    key: &str,
) -> Result<Option<Option<i64>>, String> {
    match map.get(key) {
        None => Ok(None),
        Some(LoraValue::Null) => Ok(Some(None)),
        Some(LoraValue::Int(v)) => Ok(Some(Some(*v))),
        Some(other) => Err(format!(
            "point() field '{key}' must be an integer, got {}",
            crate::errors::value_kind(other)
        )),
    }
}

/// Simple named timezone to offset mapping for common zones.
pub(super) fn timezone_name_to_offset(name: &str) -> i32 {
    // This is a simplified mapping; a full implementation would use a timezone database.
    match name {
        "UTC" | "GMT" | "Z" => 0,
        "Europe/London" => 0, // Ignoring DST for simplicity
        "Europe/Amsterdam" | "Europe/Berlin" | "Europe/Paris" | "CET" => 3600, // +01:00 (ignoring DST)
        "Europe/Moscow" => 10800,                                              // +03:00
        "US/Eastern" | "America/New_York" | "EST" => -18000,                   // -05:00
        "US/Central" | "America/Chicago" | "CST" => -21600,                    // -06:00
        "US/Mountain" | "America/Denver" | "MST" => -25200,                    // -07:00
        "US/Pacific" | "America/Los_Angeles" | "PST" => -28800,                // -08:00
        "Asia/Tokyo" | "JST" => 32400,                                         // +09:00
        "Asia/Shanghai" | "Asia/Hong_Kong" => 28800,                           // +08:00
        _ => 0, // Default to UTC for unknown timezones
    }
}
