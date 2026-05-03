//! ISO-8601 fragment parsers shared by [`super::time::LoraTime`],
//! [`super::time::LoraLocalTime`], [`super::datetime::LoraDateTime`],
//! and [`super::datetime::LoraLocalDateTime`].

/// Parse a time string returning (hour, minute, second, nanosecond,
/// optional offset_seconds).
pub(super) fn parse_time_string(s: &str) -> Result<(u32, u32, u32, u32, Option<i32>), String> {
    // Find offset suffix: Z, +HH:MM, -HH:MM
    let (time_str, offset) = if let Some(stripped) = s.strip_suffix('Z') {
        (stripped, Some(0i32))
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

    let hour = parts[0]
        .parse::<u32>()
        .map_err(|_| format!("Invalid time: {s}"))?;
    let minute = parts[1]
        .parse::<u32>()
        .map_err(|_| format!("Invalid time: {s}"))?;

    let (second, nanosecond) = if parts.len() == 3 {
        parse_seconds_and_fraction(parts[2])?
    } else {
        (0, 0)
    };

    Ok((hour, minute, second, nanosecond, offset))
}

fn parse_seconds_and_fraction(s: &str) -> Result<(u32, u32), String> {
    if let Some(dot_pos) = s.find('.') {
        let sec = s[..dot_pos]
            .parse::<u32>()
            .map_err(|_| format!("Invalid seconds: {s}"))?;
        let frac = &s[dot_pos + 1..];
        // Pad/truncate to 9 digits for nanoseconds
        let padded = format!("{:0<9}", frac);
        let ns = padded[..9].parse::<u32>().unwrap_or(0);
        Ok((sec, ns))
    } else {
        let sec = s
            .parse::<u32>()
            .map_err(|_| format!("Invalid seconds: {s}"))?;
        Ok((sec, 0))
    }
}

fn parse_offset(s: &str) -> Result<i32, String> {
    let sign = if s.starts_with('+') {
        1
    } else if s.starts_with('-') {
        -1
    } else {
        return Err(format!("Invalid offset: {s}"));
    };
    let rest = &s[1..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid offset: {s}"));
    }
    let h = parts[0]
        .parse::<i32>()
        .map_err(|_| format!("Invalid offset: {s}"))?;
    let m = parts[1]
        .parse::<i32>()
        .map_err(|_| format!("Invalid offset: {s}"))?;
    Ok(sign * (h * 3600 + m * 60))
}
