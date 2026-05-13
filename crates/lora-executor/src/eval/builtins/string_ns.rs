//! `string.*` — transforming and querying operations on STRING.
//!
//! Includes transforming string helpers. Analytic helpers (distances,
//! phonetics) live in [`super::text`].

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "upper" => unary_str(args, |s| s.to_uppercase()),
        "lower" => unary_str(args, |s| s.to_lowercase()),
        "capitalize" => capitalize(args),
        "case" => case(args),
        "replace" => replace(args),
        "find" => find(args),
        "count" => count(args),
        "before" => before(args),
        "after" => after(args),
        "split" => split(args),
        "join" => join(args),
        "pad" => pad(args),
        "pad_left" => pad_side(args, "left"),
        "pad_right" => pad_side(args, "right"),
        "repeat" => repeat(args),
        "slugify" => slugify(args),
        "escape" => escape(args),
        "hex" => hex(args),
        "char_at" => char_at(args),
        "code_at" => code_at(args),
        "regex_groups" => regex_groups(args),
        "matches" => matches_re(args),
        "starts_with" => starts_with(args),
        "ends_with" => ends_with(args),
        "contains" => contains(args),
        "words" => words(args),
        "is_blank" => is_blank(args),
        "length" => length(args),
        "url_encode" => url_encode(args),
        "url_decode" => url_decode(args),
        "swap_case" => swap_case(args),
        "trim" => trim(args),
        "trim_left" => trim_left(args),
        "trim_right" => trim_right(args),
        "slice" => slice(args),
        "prefix" => prefix(args),
        "suffix" => suffix(args),
        "reverse" => reverse(args),
        "normalize" => normalize(args),
        _ => return None,
    })
}

fn as_str(v: Option<&LoraValue>) -> Option<&str> {
    match v? {
        LoraValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn unary_str(args: &[LoraValue], f: impl Fn(&str) -> String) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::String(f(s)),
        None => LoraValue::Null,
    }
}

fn capitalize(args: &[LoraValue]) -> LoraValue {
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let all = matches!(args.get(1), Some(LoraValue::Bool(true)));
    if all {
        let mut out = String::with_capacity(s.len());
        let mut new_word = true;
        for ch in s.chars() {
            if ch.is_whitespace() {
                new_word = true;
                out.push(ch);
            } else if new_word {
                out.extend(ch.to_uppercase());
                new_word = false;
            } else {
                out.push(ch);
            }
        }
        LoraValue::String(out)
    } else {
        let mut chars = s.chars();
        match chars.next() {
            None => LoraValue::String(String::new()),
            Some(first) => {
                let rest: String = chars.collect();
                LoraValue::String(format!("{}{}", first.to_uppercase(), rest))
            }
        }
    }
}

fn case(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(style))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let words = tokenize_words(s);
    let out = match style.to_ascii_lowercase().as_str() {
        "camel" => {
            let mut out = String::new();
            for (i, w) in words.iter().enumerate() {
                if i == 0 {
                    out.push_str(&w.to_lowercase());
                } else {
                    out.push_str(&capitalize_word(w));
                }
            }
            out
        }
        "pascal" => words.iter().map(|w| capitalize_word(w)).collect(),
        "snake" => words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("_"),
        "kebab" => words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("-"),
        "screaming_snake" | "constant" => words
            .iter()
            .map(|w| w.to_uppercase())
            .collect::<Vec<_>>()
            .join("_"),
        "title" => words
            .iter()
            .map(|w| capitalize_word(w))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return LoraValue::Null,
    };
    LoraValue::String(out)
}

fn capitalize_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let rest: String = chars.collect::<String>().to_lowercase();
            format!("{}{}", first.to_uppercase(), rest)
        }
    }
}

fn tokenize_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
        prev_lower = ch.is_lowercase() || ch.is_numeric();
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn replace(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(pat)), Some(LoraValue::String(with))) =
        (as_str(args.first()), args.get(1), args.get(2))
    else {
        return LoraValue::Null;
    };
    let limit = args.get(3).and_then(LoraValue::as_i64).filter(|n| *n >= 0);
    let result = if is_regex(pat) {
        let pattern = strip_regex(pat);
        match Regex::new(pattern) {
            Ok(re) => match limit {
                Some(n) => re.replacen(s, n as usize, with.as_str()).into_owned(),
                None => re.replace_all(s, with.as_str()).into_owned(),
            },
            Err(_) => return LoraValue::Null,
        }
    } else {
        match limit {
            Some(n) => s.replacen(pat.as_str(), with, n as usize),
            None => s.replace(pat.as_str(), with),
        }
    };
    LoraValue::String(result)
}

fn find(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(needle))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let all = matches!(args.get(2), Some(LoraValue::Bool(true)));
    if all {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(pos) = s[from..].find(needle.as_str()) {
            out.push(LoraValue::Int((from + pos) as i64));
            from += pos + needle.len().max(1);
            if needle.is_empty() {
                break;
            }
        }
        LoraValue::List(out)
    } else {
        match s.find(needle.as_str()) {
            Some(i) => LoraValue::Int(i as i64),
            None => LoraValue::Int(-1),
        }
    }
}

fn count(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(needle))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let needle = needle.as_str();
    if is_regex(needle) {
        let pattern = strip_regex(needle);
        if pattern.is_empty() {
            return LoraValue::Null;
        }
        return match Regex::new(pattern) {
            Ok(re) => LoraValue::Int(re.find_iter(s).count() as i64),
            Err(_) => LoraValue::Null,
        };
    }
    if needle.is_empty() {
        return LoraValue::Null;
    }
    LoraValue::Int(s.matches(needle).count() as i64)
}

fn before(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(needle))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    if needle.is_empty() {
        return LoraValue::Null;
    }
    match s.find(needle.as_str()) {
        Some(idx) => LoraValue::String(s[..idx].to_string()),
        None => LoraValue::Null,
    }
}

fn after(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(needle))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    if needle.is_empty() {
        return LoraValue::Null;
    }
    match s.find(needle.as_str()) {
        Some(idx) => LoraValue::String(s[idx + needle.len()..].to_string()),
        None => LoraValue::Null,
    }
}

fn split(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(sep))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    if is_regex(sep) {
        let pattern = strip_regex(sep);
        match Regex::new(pattern) {
            Ok(re) => LoraValue::List(
                re.split(s)
                    .map(|p| LoraValue::String(p.to_string()))
                    .collect(),
            ),
            Err(_) => LoraValue::Null,
        }
    } else {
        LoraValue::List(
            s.split(sep.as_str())
                .map(|p| LoraValue::String(p.to_string()))
                .collect(),
        )
    }
}

fn join(args: &[LoraValue]) -> LoraValue {
    let (Some(LoraValue::List(items)), Some(LoraValue::String(sep))) = (args.first(), args.get(1))
    else {
        return LoraValue::Null;
    };
    let parts: Vec<String> = items
        .iter()
        .map(|v| match v {
            LoraValue::String(s) => s.clone(),
            LoraValue::Int(i) => i.to_string(),
            LoraValue::Float(f) => f.to_string(),
            LoraValue::Bool(b) => b.to_string(),
            LoraValue::Null => String::new(),
            other => format!("{other:?}"),
        })
        .collect();
    LoraValue::String(parts.join(sep))
}

fn pad(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(target), Some(LoraValue::String(with))) = (
        as_str(args.first()),
        args.get(1).and_then(LoraValue::as_i64),
        args.get(2),
    ) else {
        return LoraValue::Null;
    };
    if with.is_empty() {
        return LoraValue::Null;
    }
    let side = match args.get(3) {
        Some(LoraValue::String(side)) => side.to_ascii_lowercase(),
        _ => "left".to_string(),
    };
    let current = s.chars().count() as i64;
    if current >= target {
        return LoraValue::String(s.to_string());
    }
    let needed = (target - current) as usize;
    match side.as_str() {
        "left" => {
            let prefix: String = with.chars().cycle().take(needed).collect();
            LoraValue::String(format!("{prefix}{s}"))
        }
        "right" => {
            let suffix: String = with.chars().cycle().take(needed).collect();
            LoraValue::String(format!("{s}{suffix}"))
        }
        "both" => {
            let left_n = needed / 2;
            let right_n = needed - left_n;
            let prefix: String = with.chars().cycle().take(left_n).collect();
            let suffix: String = with.chars().cycle().take(right_n).collect();
            LoraValue::String(format!("{prefix}{s}{suffix}"))
        }
        _ => LoraValue::Null,
    }
}

fn repeat(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(n)) = (
        as_str(args.first()),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    if n < 0 {
        return LoraValue::Null;
    }
    LoraValue::String(s.repeat(n as usize))
}

fn slugify(args: &[LoraValue]) -> LoraValue {
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    LoraValue::String(trimmed)
}

fn escape(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(dialect))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    match dialect.to_ascii_lowercase().as_str() {
        "cypher" | "lora" => {
            let mut out = String::with_capacity(s.len() + 2);
            out.push('"');
            for ch in s.chars() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    other => out.push(other),
                }
            }
            out.push('"');
            LoraValue::String(out)
        }
        "json" => match serde_json::to_string(s) {
            Ok(j) => LoraValue::String(j),
            Err(_) => LoraValue::Null,
        },
        "html" => {
            let mut out = String::with_capacity(s.len());
            for ch in s.chars() {
                match ch {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    '\'' => out.push_str("&#39;"),
                    other => out.push(other),
                }
            }
            LoraValue::String(out)
        }
        _ => LoraValue::Null,
    }
}

fn hex(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::Int(i)) => LoraValue::String(format!("{:x}", *i as u64)),
        _ => LoraValue::Null,
    }
}

fn char_at(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(i)) = (
        as_str(args.first()),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    let chars: Vec<char> = s.chars().collect();
    let idx = resolve_index(i, chars.len());
    match idx {
        Some(i) => LoraValue::String(chars[i].to_string()),
        None => LoraValue::Null,
    }
}

fn code_at(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(i)) = (
        as_str(args.first()),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    let chars: Vec<char> = s.chars().collect();
    let idx = resolve_index(i, chars.len());
    match idx {
        Some(i) => LoraValue::Int(chars[i] as i64),
        None => LoraValue::Null,
    }
}

fn resolve_index(i: i64, len: usize) -> Option<usize> {
    let real = if i < 0 { i + len as i64 } else { i };
    if real < 0 || real >= len as i64 {
        None
    } else {
        Some(real as usize)
    }
}

fn regex_groups(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(pat))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let pattern = strip_regex(pat);
    let by_name = matches!(args.get(2), Some(LoraValue::Bool(true)));
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => return LoraValue::Null,
    };
    let mut out = Vec::new();
    for caps in re.captures_iter(s) {
        if by_name {
            let mut m = std::collections::BTreeMap::new();
            for name in re.capture_names().flatten() {
                if let Some(g) = caps.name(name) {
                    m.insert(name.to_string(), LoraValue::String(g.as_str().to_string()));
                }
            }
            out.push(LoraValue::Map(m));
        } else {
            let groups: Vec<LoraValue> = caps
                .iter()
                .map(|m| match m {
                    Some(g) => LoraValue::String(g.as_str().to_string()),
                    None => LoraValue::Null,
                })
                .collect();
            out.push(LoraValue::List(groups));
        }
    }
    LoraValue::List(out)
}

fn matches_re(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(pat))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let pattern = strip_regex(pat);
    match Regex::new(pattern) {
        Ok(re) => LoraValue::Bool(re.is_match(s)),
        Err(_) => LoraValue::Null,
    }
}

fn starts_with(args: &[LoraValue]) -> LoraValue {
    match (as_str(args.first()), args.get(1)) {
        (Some(s), Some(LoraValue::String(p))) => LoraValue::Bool(s.starts_with(p.as_str())),
        _ => LoraValue::Null,
    }
}

fn ends_with(args: &[LoraValue]) -> LoraValue {
    match (as_str(args.first()), args.get(1)) {
        (Some(s), Some(LoraValue::String(p))) => LoraValue::Bool(s.ends_with(p.as_str())),
        _ => LoraValue::Null,
    }
}

fn contains(args: &[LoraValue]) -> LoraValue {
    match (as_str(args.first()), args.get(1)) {
        (Some(s), Some(LoraValue::String(p))) => LoraValue::Bool(s.contains(p.as_str())),
        _ => LoraValue::Null,
    }
}

fn words(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::List(
            s.split_whitespace()
                .map(|word| LoraValue::String(word.to_string()))
                .collect(),
        ),
        None => LoraValue::Null,
    }
}

fn is_blank(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::Bool(s.trim().is_empty()),
        None => LoraValue::Null,
    }
}

fn length(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::Int(s.chars().count() as i64),
        None => LoraValue::Null,
    }
}

fn url_encode(args: &[LoraValue]) -> LoraValue {
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    LoraValue::String(out)
}

fn url_decode(args: &[LoraValue]) -> LoraValue {
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_digit(bytes[i + 1]);
                let lo = hex_digit(bytes[i + 2]);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h << 4) | l);
                        i += 3;
                    }
                    _ => return LoraValue::Null,
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    match String::from_utf8(out) {
        Ok(s) => LoraValue::String(s),
        Err(_) => LoraValue::Null,
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn swap_case(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => {
            let out: String = s
                .chars()
                .map(|c| {
                    if c.is_uppercase() {
                        c.to_lowercase().next().unwrap_or(c)
                    } else if c.is_lowercase() {
                        c.to_uppercase().next().unwrap_or(c)
                    } else {
                        c
                    }
                })
                .collect();
            LoraValue::String(out)
        }
        None => LoraValue::Null,
    }
}

fn pad_side(args: &[LoraValue], side: &str) -> LoraValue {
    let with = args
        .get(2)
        .cloned()
        .unwrap_or(LoraValue::String(" ".to_string()));
    let new_args = [
        args.first().cloned().unwrap_or(LoraValue::Null),
        args.get(1).cloned().unwrap_or(LoraValue::Null),
        with,
        LoraValue::String(side.to_string()),
    ];
    pad(&new_args)
}

fn trim(args: &[LoraValue]) -> LoraValue {
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let side = match args.get(1) {
        Some(LoraValue::String(s)) => s.to_ascii_lowercase(),
        _ => "both".to_string(),
    };
    LoraValue::String(match side.as_str() {
        "both" => s.trim().to_string(),
        "left" => s.trim_start().to_string(),
        "right" => s.trim_end().to_string(),
        _ => return LoraValue::Null,
    })
}

fn trim_left(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::String(s.trim_start().to_string()),
        None => LoraValue::Null,
    }
}

fn trim_right(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::String(s.trim_end().to_string()),
        None => LoraValue::Null,
    }
}

fn slice(args: &[LoraValue]) -> LoraValue {
    use crate::eval::binops::substring_by_chars;
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let start = args.get(1).and_then(LoraValue::as_i64).unwrap_or(0);
    let length = args.get(2).and_then(LoraValue::as_i64);
    LoraValue::String(substring_by_chars(s, start, length))
}

fn prefix(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(n)) = (
        as_str(args.first()),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    let n = n.max(0) as usize;
    LoraValue::String(s.chars().take(n).collect())
}

fn suffix(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(n)) = (
        as_str(args.first()),
        args.get(1).and_then(LoraValue::as_i64),
    ) else {
        return LoraValue::Null;
    };
    let n = n.max(0) as usize;
    let char_count = s.chars().count();
    let skip = char_count.saturating_sub(n);
    LoraValue::String(s.chars().skip(skip).collect())
}

fn reverse(args: &[LoraValue]) -> LoraValue {
    match as_str(args.first()) {
        Some(s) => LoraValue::String(s.chars().rev().collect()),
        None => LoraValue::Null,
    }
}

fn normalize(args: &[LoraValue]) -> LoraValue {
    let Some(s) = as_str(args.first()) else {
        return LoraValue::Null;
    };
    let form = match args.get(1) {
        Some(LoraValue::String(form)) => form.to_ascii_lowercase(),
        Some(_) => return LoraValue::Null,
        None => "nfc".to_string(),
    };
    let normalized: String = match form.as_str() {
        "nfc" => s.nfc().collect(),
        "nfd" => s.nfd().collect(),
        "nfkc" => s.nfkc().collect(),
        "nfkd" => s.nfkd().collect(),
        _ => return LoraValue::Null,
    };
    LoraValue::String(normalized)
}

fn is_regex(s: &str) -> bool {
    s.len() >= 2 && s.starts_with('/') && s.ends_with('/')
}

fn strip_regex(s: &str) -> &str {
    if is_regex(s) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}
