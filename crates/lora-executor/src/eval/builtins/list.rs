//! `list.*` — operations on `LIST<T>`.
//!
//! Provides common list helper operations, plus median/product computed forms
//! (the row-aggregating versions live in
//! `lora-analyzer::expressions::AGGREGATE_FUNCTIONS`).

use std::collections::BTreeMap;

use crate::value::LoraValue;

use super::super::binops::value_eq;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "sum" => sum(args),
        "avg" => avg(args),
        "min" => min(args),
        "max" => max(args),
        "product" => product(args),
        "stdev" => stdev(args),
        "median" => median(args),
        "sort" => sort(args),
        "reverse" => reverse(args),
        "unique" => unique(args),
        "first" => first(args),
        "rest" => rest(args),
        "init" => init(args),
        "last" => last(args),
        "at" => at(args),
        "slice" => slice(args),
        "size" => list_size(args),
        "range" => range(args),
        "contains" => contains(args),
        "contains_all" => contains_all(args),
        "has_duplicates" => has_duplicates(args),
        "all_distinct" => all_distinct(args),
        "equal_unordered" => equal_unordered(args),
        "is_empty" => is_empty(args),
        "index_of" | "find_index" => find_index(args),
        "indexes_of" | "find_indexes" => find_indexes(args),
        "find_duplicates" => find_duplicates(args),
        "count_by" => count_by(args),
        "union" => union(args),
        "intersect" => intersect(args),
        "diff" => diff(args),
        "symmetric_diff" => symmetric_diff(args),
        "zip" => zip(args),
        "chunks" => chunks(args),
        "split_by" => split_by(args),
        "windows" => windows(args),
        "scan" => scan(args),
        "repeat" => repeat(args),
        "flatten" => flatten(args),
        "sample" => sample(args),
        "shuffle" => shuffle(args),
        "combinations" => combinations(args),
        "concat" => concat(args),
        "append" => append(args),
        "prepend" => prepend(args),
        "take" => take(args),
        "drop" => drop_n(args),
        "take_last" => take_last(args),
        "drop_last" => drop_last(args),
        "insert" => insert(args),
        "remove" => remove(args),
        "compact" => compact(args),
        _ => return None,
    })
}

fn as_list(v: Option<&LoraValue>) -> Option<&[LoraValue]> {
    match v? {
        LoraValue::List(xs) => Some(xs.as_slice()),
        _ => None,
    }
}

fn as_i64(v: Option<&LoraValue>) -> Option<i64> {
    v.and_then(LoraValue::as_i64)
}

fn sum(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut int_acc: i64 = 0;
    let mut float_acc: f64 = 0.0;
    let mut any_float = false;
    let mut any_value = false;
    for v in xs {
        match v {
            LoraValue::Null => continue,
            LoraValue::Int(i) => {
                any_value = true;
                if any_float {
                    float_acc += *i as f64;
                } else {
                    int_acc = int_acc.wrapping_add(*i);
                }
            }
            LoraValue::Float(f) => {
                any_value = true;
                if !any_float {
                    any_float = true;
                    float_acc = int_acc as f64;
                }
                float_acc += *f;
            }
            _ => return LoraValue::Null,
        }
    }
    if !any_value {
        LoraValue::Int(0)
    } else if any_float {
        LoraValue::Float(float_acc)
    } else {
        LoraValue::Int(int_acc)
    }
}

fn avg(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut acc = 0.0_f64;
    let mut count = 0_u64;
    for v in xs {
        match v.as_f64() {
            Some(f) => {
                acc += f;
                count += 1;
            }
            None if matches!(v, LoraValue::Null) => continue,
            None => return LoraValue::Null,
        }
    }
    if count == 0 {
        LoraValue::Null
    } else {
        LoraValue::Float(acc / count as f64)
    }
}

fn min(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    xs.iter()
        .filter(|v| !matches!(v, LoraValue::Null))
        .fold(None, |best, v| match best {
            None => Some(v.clone()),
            Some(b) => match compare_values(&b, v) {
                Some(std::cmp::Ordering::Greater) => Some(v.clone()),
                _ => Some(b),
            },
        })
        .unwrap_or(LoraValue::Null)
}

fn max(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    xs.iter()
        .filter(|v| !matches!(v, LoraValue::Null))
        .fold(None, |best, v| match best {
            None => Some(v.clone()),
            Some(b) => match compare_values(&b, v) {
                Some(std::cmp::Ordering::Less) => Some(v.clone()),
                _ => Some(b),
            },
        })
        .unwrap_or(LoraValue::Null)
}

fn product(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut int_acc: i64 = 1;
    let mut float_acc: f64 = 1.0;
    let mut any_float = false;
    let mut any_value = false;
    for v in xs {
        match v {
            LoraValue::Null => continue,
            LoraValue::Int(i) => {
                any_value = true;
                if any_float {
                    float_acc *= *i as f64;
                } else {
                    int_acc = int_acc.wrapping_mul(*i);
                }
            }
            LoraValue::Float(f) => {
                any_value = true;
                if !any_float {
                    any_float = true;
                    float_acc = int_acc as f64;
                }
                float_acc *= *f;
            }
            _ => return LoraValue::Null,
        }
    }
    if !any_value {
        LoraValue::Int(1)
    } else if any_float {
        LoraValue::Float(float_acc)
    } else {
        LoraValue::Int(int_acc)
    }
}

fn stdev(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut values: Vec<f64> = Vec::with_capacity(xs.len());
    for v in xs {
        match v {
            LoraValue::Null => continue,
            other => match other.as_f64() {
                Some(f) => values.push(f),
                None => return LoraValue::Null,
            },
        }
    }
    if values.len() < 2 {
        return LoraValue::Null;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() as f64 - 1.0);
    LoraValue::Float(var.sqrt())
}

fn median(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut values: Vec<f64> = Vec::with_capacity(xs.len());
    for v in xs {
        match v {
            LoraValue::Null => continue,
            other => match other.as_f64() {
                Some(f) => values.push(f),
                None => return LoraValue::Null,
            },
        }
    }
    if values.is_empty() {
        return LoraValue::Null;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        LoraValue::Float((values[mid - 1] + values[mid]) / 2.0)
    } else {
        LoraValue::Float(values[mid])
    }
}

fn sort(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut out: Vec<LoraValue> = xs.to_vec();
    out.sort_by(|a, b| compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal));
    if matches!(
        args.get(1),
        Some(LoraValue::String(s)) if s.eq_ignore_ascii_case("desc")
    ) {
        out.reverse();
    }
    LoraValue::List(out)
}

fn reverse(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::List(xs.iter().rev().cloned().collect())
}

fn unique(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut out: Vec<LoraValue> = Vec::with_capacity(xs.len());
    for v in xs {
        if !out.iter().any(|existing| value_eq(existing, v)) {
            out.push(v.clone());
        }
    }
    LoraValue::List(out)
}

fn contains(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(needle)) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    LoraValue::Bool(xs.iter().any(|v| value_eq(v, needle)))
}

fn contains_all(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(needles)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    LoraValue::Bool(needles.iter().all(|n| xs.iter().any(|v| value_eq(v, n))))
}

fn has_duplicates(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    for (i, a) in xs.iter().enumerate() {
        for b in &xs[i + 1..] {
            if value_eq(a, b) {
                return LoraValue::Bool(true);
            }
        }
    }
    LoraValue::Bool(false)
}

fn all_distinct(args: &[LoraValue]) -> LoraValue {
    match has_duplicates(args) {
        LoraValue::Bool(b) => LoraValue::Bool(!b),
        other => other,
    }
}

fn equal_unordered(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(ys)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    if xs.len() != ys.len() {
        return LoraValue::Bool(false);
    }
    let mut consumed = vec![false; ys.len()];
    for x in xs {
        let pos = ys
            .iter()
            .enumerate()
            .find(|(i, y)| !consumed[*i] && value_eq(x, y));
        match pos {
            Some((i, _)) => consumed[i] = true,
            None => return LoraValue::Bool(false),
        }
    }
    LoraValue::Bool(true)
}

fn is_empty(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::List(xs)) => LoraValue::Bool(xs.is_empty()),
        Some(LoraValue::String(s)) => LoraValue::Bool(s.is_empty()),
        Some(LoraValue::Map(m)) => LoraValue::Bool(m.is_empty()),
        Some(LoraValue::Null) | None => LoraValue::Null,
        _ => LoraValue::Bool(false),
    }
}

fn find_index(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(needle)) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    match xs.iter().position(|v| value_eq(v, needle)) {
        Some(i) => LoraValue::Int(i as i64),
        None => LoraValue::Int(-1),
    }
}

fn find_indexes(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(needle)) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    LoraValue::List(
        xs.iter()
            .enumerate()
            .filter_map(|(i, v)| value_eq(v, needle).then_some(LoraValue::Int(i as i64)))
            .collect(),
    )
}

fn find_duplicates(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut seen: Vec<LoraValue> = Vec::new();
    let mut dupes: Vec<LoraValue> = Vec::new();
    for v in xs {
        if seen.iter().any(|s| value_eq(s, v)) {
            if !dupes.iter().any(|d| value_eq(d, v)) {
                dupes.push(v.clone());
            }
        } else {
            seen.push(v.clone());
        }
    }
    LoraValue::List(dupes)
}

fn count_by(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for v in xs {
        let key = display_key(v);
        *counts.entry(key).or_insert(0) += 1;
    }
    LoraValue::Map(
        counts
            .into_iter()
            .map(|(k, n)| (k, LoraValue::Int(n)))
            .collect(),
    )
}

fn union(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(ys)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    let mut out: Vec<LoraValue> = Vec::with_capacity(xs.len() + ys.len());
    for v in xs.iter().chain(ys.iter()) {
        if !out.iter().any(|e| value_eq(e, v)) {
            out.push(v.clone());
        }
    }
    LoraValue::List(out)
}

fn intersect(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(ys)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    let mut out = Vec::new();
    for v in xs {
        if ys.iter().any(|y| value_eq(y, v)) && !out.iter().any(|e| value_eq(e, v)) {
            out.push(v.clone());
        }
    }
    LoraValue::List(out)
}

fn diff(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(ys)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    LoraValue::List(
        xs.iter()
            .filter(|v| !ys.iter().any(|y| value_eq(y, v)))
            .cloned()
            .collect(),
    )
}

fn symmetric_diff(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(ys)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    let mut out = Vec::new();
    for v in xs {
        if !ys.iter().any(|y| value_eq(y, v)) && !out.iter().any(|e| value_eq(e, v)) {
            out.push(v.clone());
        }
    }
    for v in ys {
        if !xs.iter().any(|x| value_eq(x, v)) && !out.iter().any(|e| value_eq(e, v)) {
            out.push(v.clone());
        }
    }
    LoraValue::List(out)
}

fn zip(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(ys)) = (as_list(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    let len = xs.len().min(ys.len());
    LoraValue::List(
        (0..len)
            .map(|i| LoraValue::List(vec![xs[i].clone(), ys[i].clone()]))
            .collect(),
    )
}

fn chunks(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(size)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if size <= 0 {
        return LoraValue::Null;
    }
    let size = size as usize;
    LoraValue::List(
        xs.chunks(size)
            .map(|c| LoraValue::List(c.to_vec()))
            .collect(),
    )
}

fn split_by(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(sep)) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let mut out: Vec<Vec<LoraValue>> = vec![Vec::new()];
    for v in xs {
        if value_eq(v, sep) {
            out.push(Vec::new());
        } else {
            out.last_mut().unwrap().push(v.clone());
        }
    }
    LoraValue::List(out.into_iter().map(LoraValue::List).collect())
}

fn windows(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(size)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if size <= 0 {
        return LoraValue::Null;
    }
    let size = size as usize;
    let step = as_i64(args.get(2)).unwrap_or(1).max(1) as usize;
    let mut out: Vec<LoraValue> = Vec::new();
    let mut i = 0;
    while i + size <= xs.len() {
        out.push(LoraValue::List(xs[i..i + size].to_vec()));
        i += step;
    }
    LoraValue::List(out)
}

fn scan(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(LoraValue::String(op))) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let mut out = Vec::with_capacity(xs.len());
    let mut int_acc: i64 = 0;
    let mut float_acc: f64 = 0.0;
    let mut any_float = false;
    let mut started = false;
    for v in xs {
        let value = match v {
            LoraValue::Null => continue,
            LoraValue::Int(i) => *i as f64,
            LoraValue::Float(f) => {
                any_float = true;
                *f
            }
            _ => return LoraValue::Null,
        };
        if !started {
            if matches!(v, LoraValue::Int(_)) {
                int_acc = v.as_i64().unwrap_or(0);
            } else {
                float_acc = value;
                any_float = true;
            }
            started = true;
        } else {
            match op.as_str() {
                "sum" => {
                    if any_float {
                        float_acc += value;
                    } else if let Some(i) = v.as_i64() {
                        int_acc = int_acc.wrapping_add(i);
                    } else {
                        any_float = true;
                        float_acc = int_acc as f64 + value;
                    }
                }
                "product" => {
                    if any_float {
                        float_acc *= value;
                    } else if let Some(i) = v.as_i64() {
                        int_acc = int_acc.wrapping_mul(i);
                    } else {
                        any_float = true;
                        float_acc = int_acc as f64 * value;
                    }
                }
                "min" => {
                    if any_float {
                        float_acc = float_acc.min(value);
                    } else if let Some(i) = v.as_i64() {
                        int_acc = int_acc.min(i);
                    }
                }
                "max" => {
                    if any_float {
                        float_acc = float_acc.max(value);
                    } else if let Some(i) = v.as_i64() {
                        int_acc = int_acc.max(i);
                    }
                }
                _ => return LoraValue::Null,
            }
        }
        out.push(if any_float {
            LoraValue::Float(float_acc)
        } else {
            LoraValue::Int(int_acc)
        });
    }
    LoraValue::List(out)
}

fn repeat(args: &[LoraValue]) -> LoraValue {
    let Some(item) = args.first() else {
        return LoraValue::Null;
    };
    let count = match as_i64(args.get(1)) {
        Some(n) if n >= 0 => n as usize,
        _ => return LoraValue::Null,
    };
    LoraValue::List(vec![item.clone(); count])
}

fn flatten(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let depth = as_i64(args.get(1)).unwrap_or(1).max(0) as usize;
    fn go(items: &[LoraValue], depth: usize, out: &mut Vec<LoraValue>) {
        for v in items {
            match v {
                LoraValue::List(inner) if depth > 0 => go(inner, depth - 1, out),
                _ => out.push(v.clone()),
            }
        }
    }
    let mut out = Vec::new();
    go(xs, depth, &mut out);
    LoraValue::List(out)
}

fn sample(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let n = as_i64(args.get(1)).unwrap_or(1).max(0) as usize;
    if xs.is_empty() || n == 0 {
        return LoraValue::List(Vec::new());
    }
    let mut indices: Vec<usize> = (0..xs.len()).collect();
    let mut rng = SimpleRng::new();
    for i in (1..indices.len()).rev() {
        let j = rng.range(i + 1);
        indices.swap(i, j);
    }
    let take = n.min(indices.len());
    LoraValue::List(indices[..take].iter().map(|i| xs[*i].clone()).collect())
}

fn shuffle(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    let mut out = xs.to_vec();
    let mut rng = SimpleRng::new();
    for i in (1..out.len()).rev() {
        let j = rng.range(i + 1);
        out.swap(i, j);
    }
    LoraValue::List(out)
}

fn combinations(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(k)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if k < 0 || (k as usize) > xs.len() {
        return LoraValue::List(Vec::new());
    }
    let k = k as usize;
    let mut out: Vec<Vec<LoraValue>> = Vec::new();
    let mut current: Vec<LoraValue> = Vec::with_capacity(k);
    fn go(
        xs: &[LoraValue],
        start: usize,
        k: usize,
        current: &mut Vec<LoraValue>,
        out: &mut Vec<Vec<LoraValue>>,
    ) {
        if current.len() == k {
            out.push(current.clone());
            return;
        }
        for i in start..xs.len() {
            current.push(xs[i].clone());
            go(xs, i + 1, k, current, out);
            current.pop();
        }
    }
    go(xs, 0, k, &mut current, &mut out);
    LoraValue::List(out.into_iter().map(LoraValue::List).collect())
}

fn concat(args: &[LoraValue]) -> LoraValue {
    let mut out = Vec::new();
    for arg in args {
        let LoraValue::List(xs) = arg else {
            return LoraValue::Null;
        };
        out.extend(xs.iter().cloned());
    }
    LoraValue::List(out)
}

fn append(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(item)) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let mut out = xs.to_vec();
    out.push(item.clone());
    LoraValue::List(out)
}

fn prepend(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(item)) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let mut out = Vec::with_capacity(xs.len() + 1);
    out.push(item.clone());
    out.extend(xs.iter().cloned());
    LoraValue::List(out)
}

fn take(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(n)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if n <= 0 {
        return LoraValue::List(Vec::new());
    }
    let n = (n as usize).min(xs.len());
    LoraValue::List(xs[..n].to_vec())
}

fn drop_n(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(n)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if n <= 0 {
        return LoraValue::List(xs.to_vec());
    }
    let n = (n as usize).min(xs.len());
    LoraValue::List(xs[n..].to_vec())
}

fn take_last(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(n)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if n <= 0 {
        return LoraValue::List(Vec::new());
    }
    let n = (n as usize).min(xs.len());
    LoraValue::List(xs[xs.len() - n..].to_vec())
}

fn drop_last(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(n)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    if n <= 0 {
        return LoraValue::List(xs.to_vec());
    }
    let keep = xs.len().saturating_sub(n as usize);
    LoraValue::List(xs[..keep].to_vec())
}

fn insert(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(idx), Some(item)) =
        (as_list(args.first()), as_i64(args.get(1)), args.get(2))
    else {
        return LoraValue::Null;
    };
    let mut out = xs.to_vec();
    let idx = if idx < 0 {
        0
    } else if idx as usize > out.len() {
        out.len()
    } else {
        idx as usize
    };
    out.insert(idx, item.clone());
    LoraValue::List(out)
}

fn remove(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(idx)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    let mut out = xs.to_vec();
    let len = out.len() as i64;
    let real = if idx < 0 { idx + len } else { idx };
    if real < 0 || real >= len {
        return LoraValue::List(out);
    }
    out.remove(real as usize);
    LoraValue::List(out)
}

fn compact(args: &[LoraValue]) -> LoraValue {
    let Some(xs) = as_list(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::List(
        xs.iter()
            .filter(|v| !matches!(v, LoraValue::Null))
            .cloned()
            .collect(),
    )
}

fn first(args: &[LoraValue]) -> LoraValue {
    match as_list(args.first()) {
        Some(xs) => xs.first().cloned().unwrap_or(LoraValue::Null),
        None => LoraValue::Null,
    }
}

fn rest(args: &[LoraValue]) -> LoraValue {
    match as_list(args.first()) {
        Some([]) => LoraValue::Null,
        Some(xs) => LoraValue::List(xs[1..].to_vec()),
        None => LoraValue::Null,
    }
}

fn init(args: &[LoraValue]) -> LoraValue {
    match as_list(args.first()) {
        Some([]) => LoraValue::Null,
        Some(xs) => LoraValue::List(xs[..xs.len() - 1].to_vec()),
        None => LoraValue::Null,
    }
}

fn last(args: &[LoraValue]) -> LoraValue {
    match as_list(args.first()) {
        Some(xs) => xs.last().cloned().unwrap_or(LoraValue::Null),
        None => LoraValue::Null,
    }
}

fn at(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(idx)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    let len = xs.len() as i64;
    let real = if idx < 0 { idx + len } else { idx };
    if real < 0 || real >= len {
        LoraValue::Null
    } else {
        xs[real as usize].clone()
    }
}

fn slice(args: &[LoraValue]) -> LoraValue {
    let (Some(xs), Some(start)) = (as_list(args.first()), as_i64(args.get(1))) else {
        return LoraValue::Null;
    };
    let len = xs.len() as i64;
    let end = as_i64(args.get(2)).unwrap_or(len);
    let start = normalize_slice_bound(start, len);
    let end = normalize_slice_bound(end, len);
    if end <= start {
        LoraValue::List(Vec::new())
    } else {
        LoraValue::List(xs[start as usize..end as usize].to_vec())
    }
}

fn normalize_slice_bound(bound: i64, len: i64) -> i64 {
    let real = if bound < 0 { bound + len } else { bound };
    real.clamp(0, len)
}

fn list_size(args: &[LoraValue]) -> LoraValue {
    match as_list(args.first()) {
        Some(xs) => LoraValue::Int(xs.len() as i64),
        None => LoraValue::Null,
    }
}

fn range(args: &[LoraValue]) -> LoraValue {
    let start = args.first().and_then(LoraValue::as_i64).unwrap_or(0);
    let end = args.get(1).and_then(LoraValue::as_i64).unwrap_or(0);
    let step = args.get(2).and_then(LoraValue::as_i64).unwrap_or(1);
    if step == 0 {
        return LoraValue::Null;
    }
    let mut result = Vec::new();
    let mut i = start;
    if step > 0 {
        while i <= end {
            result.push(LoraValue::Int(i));
            i += step;
        }
    } else {
        while i >= end {
            result.push(LoraValue::Int(i));
            i += step;
        }
    }
    LoraValue::List(result)
}

fn compare_values(a: &LoraValue, b: &LoraValue) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (a, b) {
        (LoraValue::Int(x), LoraValue::Int(y)) => Some(x.cmp(y)),
        (LoraValue::Float(x), LoraValue::Float(y)) => x.partial_cmp(y),
        (LoraValue::Int(x), LoraValue::Float(y)) => (*x as f64).partial_cmp(y),
        (LoraValue::Float(x), LoraValue::Int(y)) => x.partial_cmp(&(*y as f64)),
        (LoraValue::String(x), LoraValue::String(y)) => Some(x.cmp(y)),
        (LoraValue::Bool(x), LoraValue::Bool(y)) => Some(x.cmp(y)),
        (LoraValue::Null, LoraValue::Null) => Some(Ordering::Equal),
        (LoraValue::Null, _) => Some(Ordering::Less),
        (_, LoraValue::Null) => Some(Ordering::Greater),
        _ => None,
    }
}

fn display_key(v: &LoraValue) -> String {
    match v {
        LoraValue::String(s) => s.clone(),
        LoraValue::Int(i) => i.to_string(),
        LoraValue::Float(f) => f.to_string(),
        LoraValue::Bool(b) => b.to_string(),
        LoraValue::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

/// Tiny LCG seeded from the system clock. Deterministic within a single
/// `dispatch` call, non-cryptographic, no external dep. We don't need
/// cryptographic randomness for `list.sample` / `list.shuffle`.
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15)
            .max(1);
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn range(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}
