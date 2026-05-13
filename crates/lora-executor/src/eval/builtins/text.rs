//! `text.*` — string-analysis functions (distances, phonetics).
//!
//! Transforming string operations live in [`super::string_ns`]; this
//! module is the analytic half.

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "distance" => distance(args),
        "similarity" => similarity(args),
        "phonetic" => phonetic(args),
        "phonetic_match" => phonetic_match(args),
        _ => return None,
    })
}

fn as_str(v: Option<&LoraValue>) -> Option<&str> {
    match v? {
        LoraValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn distance(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b), Some(LoraValue::String(metric))) =
        (as_str(args.first()), as_str(args.get(1)), args.get(2))
    else {
        return LoraValue::Null;
    };
    let n = match metric.to_ascii_lowercase().as_str() {
        "levenshtein" => levenshtein(a, b),
        "damerau" => damerau_levenshtein(a, b),
        "hamming" => match hamming(a, b) {
            Some(n) => n,
            None => return LoraValue::Null,
        },
        _ => return LoraValue::Null,
    };
    LoraValue::Int(n as i64)
}

fn similarity(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b), Some(LoraValue::String(metric))) =
        (as_str(args.first()), as_str(args.get(1)), args.get(2))
    else {
        return LoraValue::Null;
    };
    let val = match metric.to_ascii_lowercase().as_str() {
        "levenshtein" => levenshtein_similarity(a, b),
        "jaro" => jaro(a, b),
        "jaro_winkler" => jaro_winkler(a, b),
        "sorensen_dice" | "sorensen" | "dice" => sorensen_dice(a, b),
        _ => return LoraValue::Null,
    };
    LoraValue::Float(val)
}

fn phonetic(args: &[LoraValue]) -> LoraValue {
    let (Some(s), Some(LoraValue::String(algo))) = (as_str(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let code = match algo.to_ascii_lowercase().as_str() {
        "soundex" => soundex(s),
        _ => return LoraValue::Null,
    };
    LoraValue::String(code)
}

fn phonetic_match(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b), Some(LoraValue::String(algo))) =
        (as_str(args.first()), as_str(args.get(1)), args.get(2))
    else {
        return LoraValue::Null;
    };
    let codes = match algo.to_ascii_lowercase().as_str() {
        "soundex" => (soundex(a), soundex(b)),
        _ => return LoraValue::Null,
    };
    LoraValue::Bool(codes.0 == codes.1)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut d = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[m][n]
}

fn hamming(a: &str, b: &str) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len() != b.len() {
        return None;
    }
    Some(a.iter().zip(b.iter()).filter(|(x, y)| x != y).count())
}

fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    let d = levenshtein(a, b);
    1.0 - (d as f64 / max_len as f64)
}

fn jaro(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 && n == 0 {
        return 1.0;
    }
    if m == 0 || n == 0 {
        return 0.0;
    }
    let match_distance = (m.max(n) / 2).saturating_sub(1);
    let mut a_matches = vec![false; m];
    let mut b_matches = vec![false; n];
    let mut matches = 0;
    for i in 0..m {
        let lo = i.saturating_sub(match_distance);
        let hi = (i + match_distance + 1).min(n);
        for j in lo..hi {
            if !b_matches[j] && a[i] == b[j] {
                a_matches[i] = true;
                b_matches[j] = true;
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 {
        return 0.0;
    }
    let mut t = 0;
    let mut k = 0;
    for i in 0..m {
        if a_matches[i] {
            while !b_matches[k] {
                k += 1;
            }
            if a[i] != b[k] {
                t += 1;
            }
            k += 1;
        }
    }
    let t = t as f64 / 2.0;
    let mf = matches as f64;
    (mf / m as f64 + mf / n as f64 + (mf - t) / mf) / 3.0
}

fn jaro_winkler(a: &str, b: &str) -> f64 {
    let j = jaro(a, b);
    if j < 0.7 {
        return j;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let prefix = a_chars
        .iter()
        .zip(b_chars.iter())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count();
    j + prefix as f64 * 0.1 * (1.0 - j)
}

fn sorensen_dice(a: &str, b: &str) -> f64 {
    let a_bigrams = bigrams(a);
    let b_bigrams = bigrams(b);
    if a_bigrams.is_empty() && b_bigrams.is_empty() {
        return 1.0;
    }
    if a_bigrams.is_empty() || b_bigrams.is_empty() {
        return 0.0;
    }
    let mut intersection = 0;
    let mut b_consumed = vec![false; b_bigrams.len()];
    for ab in &a_bigrams {
        for (i, bb) in b_bigrams.iter().enumerate() {
            if !b_consumed[i] && ab == bb {
                b_consumed[i] = true;
                intersection += 1;
                break;
            }
        }
    }
    2.0 * intersection as f64 / (a_bigrams.len() + b_bigrams.len()) as f64
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = s.chars().collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

fn soundex(s: &str) -> String {
    let mut chars = s.chars().filter(|c| c.is_alphabetic());
    let first = match chars.next() {
        Some(c) => c.to_ascii_uppercase(),
        None => return String::new(),
    };
    let mut out = String::with_capacity(4);
    out.push(first);
    let mut last_code = soundex_code(first);
    for ch in chars {
        let code = soundex_code(ch.to_ascii_uppercase());
        if code != '0' && code != last_code {
            out.push(code);
            if out.len() == 4 {
                break;
            }
        }
        if code != '0' {
            last_code = code;
        } else {
            last_code = '0';
        }
    }
    while out.len() < 4 {
        out.push('0');
    }
    out
}

fn soundex_code(c: char) -> char {
    match c {
        'B' | 'F' | 'P' | 'V' => '1',
        'C' | 'G' | 'J' | 'K' | 'Q' | 'S' | 'X' | 'Z' => '2',
        'D' | 'T' => '3',
        'L' => '4',
        'M' | 'N' => '5',
        'R' => '6',
        _ => '0',
    }
}
