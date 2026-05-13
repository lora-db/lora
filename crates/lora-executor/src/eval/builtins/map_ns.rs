//! `map.*` — operations on MAP.
//!
//! Provides common map helper operations.

use std::collections::BTreeMap;

use crate::value::LoraValue;

pub(super) fn dispatch(op: &str, args: &[LoraValue]) -> Option<LoraValue> {
    Some(match op {
        "from" => from(args),
        "set" => set(args),
        "remove" => remove(args),
        "merge" => merge(args),
        "deep_merge" => deep_merge(args),
        "compact" => compact(args),
        "group_by" => group_by(args),
        "flatten" => flatten(args),
        "unflatten" => unflatten(args),
        "get_path" => get_path(args),
        "set_path" => set_path(args),
        "remove_path" => remove_path(args),
        "entries" => entries(args),
        "values" => values(args),
        "keys" => keys(args),
        "has_key" => has_key(args),
        "pick" => pick(args),
        "rename" => rename(args),
        "invert" => invert(args),
        "get" => get(args),
        "size" => size(args),
        "index_by" => index_by(args),
        _ => return None,
    })
}

fn as_map(v: Option<&LoraValue>) -> Option<&BTreeMap<String, LoraValue>> {
    match v? {
        LoraValue::Map(m) => Some(m),
        _ => None,
    }
}

fn as_list(v: Option<&LoraValue>) -> Option<&[LoraValue]> {
    match v? {
        LoraValue::List(xs) => Some(xs.as_slice()),
        _ => None,
    }
}

fn from(args: &[LoraValue]) -> LoraValue {
    match args.first() {
        Some(LoraValue::List(pairs)) => {
            if let Some(LoraValue::List(vals)) = args.get(1) {
                let mut out = BTreeMap::new();
                for (k, v) in pairs.iter().zip(vals.iter()) {
                    if let LoraValue::String(k) = k {
                        out.insert(k.clone(), v.clone());
                    } else {
                        return LoraValue::Null;
                    }
                }
                return LoraValue::Map(out);
            }
            if !pairs.is_empty() && pairs.iter().all(|p| matches!(p, LoraValue::List(_))) {
                let mut out = BTreeMap::new();
                for p in pairs {
                    if let LoraValue::List(pair) = p {
                        if pair.len() != 2 {
                            return LoraValue::Null;
                        }
                        if let LoraValue::String(k) = &pair[0] {
                            out.insert(k.clone(), pair[1].clone());
                        } else {
                            return LoraValue::Null;
                        }
                    }
                }
                LoraValue::Map(out)
            } else {
                let mut out = BTreeMap::new();
                let mut it = pairs.iter();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    if let LoraValue::String(k) = k {
                        out.insert(k.clone(), v.clone());
                    } else {
                        return LoraValue::Null;
                    }
                }
                LoraValue::Map(out)
            }
        }
        _ => LoraValue::Null,
    }
}

fn set(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(LoraValue::String(k)), Some(v)) =
        (as_map(args.first()), args.get(1), args.get(2))
    else {
        return LoraValue::Null;
    };
    let mut out = m.clone();
    out.insert(k.clone(), v.clone());
    LoraValue::Map(out)
}

fn remove(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    let mut out = m.clone();
    match args.get(1) {
        Some(LoraValue::String(k)) => {
            out.remove(k);
        }
        Some(LoraValue::List(keys)) => {
            for k in keys {
                if let LoraValue::String(k) = k {
                    out.remove(k);
                }
            }
        }
        _ => return LoraValue::Null,
    }
    LoraValue::Map(out)
}

fn merge(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b)) = (as_map(args.first()), as_map(args.get(1))) else {
        return LoraValue::Null;
    };
    let strategy = match args.get(2) {
        Some(LoraValue::String(s)) => s.to_ascii_lowercase(),
        _ => "right".to_string(),
    };
    let mut out = a.clone();
    match strategy.as_str() {
        "right" => {
            for (k, v) in b {
                out.insert(k.clone(), v.clone());
            }
        }
        "left" => {
            for (k, v) in b {
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        "error" => {
            for (k, v) in b {
                if out.contains_key(k) {
                    return LoraValue::Null;
                }
                out.insert(k.clone(), v.clone());
            }
        }
        _ => return LoraValue::Null,
    }
    LoraValue::Map(out)
}

fn deep_merge(args: &[LoraValue]) -> LoraValue {
    let (Some(a), Some(b)) = (as_map(args.first()), as_map(args.get(1))) else {
        return LoraValue::Null;
    };
    let strategy = match merge_strategy(args.get(2)) {
        Some(strategy) => strategy,
        None => return LoraValue::Null,
    };
    match deep_merge_maps(a, b, strategy) {
        Some(out) => LoraValue::Map(out),
        None => LoraValue::Null,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeStrategy {
    Right,
    Left,
    Error,
}

fn merge_strategy(value: Option<&LoraValue>) -> Option<MergeStrategy> {
    match value {
        None => Some(MergeStrategy::Right),
        Some(LoraValue::String(s)) => match s.to_ascii_lowercase().as_str() {
            "right" => Some(MergeStrategy::Right),
            "left" => Some(MergeStrategy::Left),
            "error" => Some(MergeStrategy::Error),
            _ => None,
        },
        _ => Some(MergeStrategy::Right),
    }
}

fn deep_merge_maps(
    a: &BTreeMap<String, LoraValue>,
    b: &BTreeMap<String, LoraValue>,
    strategy: MergeStrategy,
) -> Option<BTreeMap<String, LoraValue>> {
    let mut out = a.clone();
    for (key, right) in b {
        match (out.get(key), right) {
            (Some(LoraValue::Map(left_map)), LoraValue::Map(right_map)) => {
                let merged = deep_merge_maps(left_map, right_map, strategy)?;
                out.insert(key.clone(), LoraValue::Map(merged));
            }
            (Some(_), _) => match strategy {
                MergeStrategy::Right => {
                    out.insert(key.clone(), right.clone());
                }
                MergeStrategy::Left => {}
                MergeStrategy::Error => return None,
            },
            (None, _) => {
                out.insert(key.clone(), right.clone());
            }
        }
    }
    Some(out)
}

fn compact(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::Map(
        m.iter()
            .filter(|(_, v)| !matches!(v, LoraValue::Null))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

fn group_by(args: &[LoraValue]) -> LoraValue {
    let (Some(items), Some(LoraValue::String(key))) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let mut out: BTreeMap<String, Vec<LoraValue>> = BTreeMap::new();
    for item in items {
        if let LoraValue::Map(m) = item {
            if let Some(k_val) = m.get(key) {
                let k = stringify(k_val);
                out.entry(k).or_default().push(item.clone());
            }
        }
    }
    LoraValue::Map(
        out.into_iter()
            .map(|(k, v)| (k, LoraValue::List(v)))
            .collect(),
    )
}

fn index_by(args: &[LoraValue]) -> LoraValue {
    let (Some(items), Some(LoraValue::String(key))) = (as_list(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    let mut out: BTreeMap<String, LoraValue> = BTreeMap::new();
    for item in items {
        if let LoraValue::Map(m) = item {
            if let Some(k_val) = m.get(key) {
                out.insert(stringify(k_val), item.clone());
            }
        }
    }
    LoraValue::Map(out)
}

fn flatten(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    let sep = match args.get(1) {
        Some(LoraValue::String(s)) => s.clone(),
        _ => ".".to_string(),
    };
    let mut out = BTreeMap::new();
    flatten_into(m, "", &sep, &mut out);
    LoraValue::Map(out)
}

fn flatten_into(
    m: &BTreeMap<String, LoraValue>,
    prefix: &str,
    sep: &str,
    out: &mut BTreeMap<String, LoraValue>,
) {
    for (k, v) in m {
        let key = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}{sep}{k}")
        };
        match v {
            LoraValue::Map(inner) => flatten_into(inner, &key, sep, out),
            other => {
                out.insert(key, other.clone());
            }
        }
    }
}

fn unflatten(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    let sep = match args.get(1) {
        Some(LoraValue::String(s)) => s.clone(),
        _ => ".".to_string(),
    };
    let mut out: BTreeMap<String, LoraValue> = BTreeMap::new();
    for (k, v) in m {
        let parts: Vec<&str> = k.split(&sep[..]).collect();
        insert_path(&mut out, &parts, v.clone());
    }
    LoraValue::Map(out)
}

fn get_path(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    let Some(path) = path_parts(args.get(1)) else {
        return LoraValue::Null;
    };
    match get_path_value(m, &path) {
        Some(value) => value.clone(),
        None => args.get(2).cloned().unwrap_or(LoraValue::Null),
    }
}

fn set_path(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(path), Some(value)) =
        (as_map(args.first()), path_parts(args.get(1)), args.get(2))
    else {
        return LoraValue::Null;
    };
    let mut out = m.clone();
    set_path_value(&mut out, &path, value.clone());
    LoraValue::Map(out)
}

fn remove_path(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(path)) = (as_map(args.first()), path_parts(args.get(1))) else {
        return LoraValue::Null;
    };
    let mut out = m.clone();
    remove_path_value(&mut out, &path);
    LoraValue::Map(out)
}

fn path_parts(value: Option<&LoraValue>) -> Option<Vec<String>> {
    let parts: Vec<String> = match value? {
        LoraValue::String(path) => path
            .split('.')
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        LoraValue::List(parts) => parts
            .iter()
            .map(|part| match part {
                LoraValue::String(part) => Some(part.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

fn get_path_value<'a>(
    map: &'a BTreeMap<String, LoraValue>,
    path: &[String],
) -> Option<&'a LoraValue> {
    let (first, rest) = path.split_first()?;
    let value = map.get(first)?;
    if rest.is_empty() {
        return Some(value);
    }
    match value {
        LoraValue::Map(inner) => get_path_value(inner, rest),
        _ => None,
    }
}

fn set_path_value(map: &mut BTreeMap<String, LoraValue>, path: &[String], value: LoraValue) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        map.insert(first.clone(), value);
        return;
    }
    let entry = map
        .entry(first.clone())
        .or_insert_with(|| LoraValue::Map(BTreeMap::new()));
    if !matches!(entry, LoraValue::Map(_)) {
        *entry = LoraValue::Map(BTreeMap::new());
    }
    if let LoraValue::Map(inner) = entry {
        set_path_value(inner, rest, value);
    }
}

fn remove_path_value(map: &mut BTreeMap<String, LoraValue>, path: &[String]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        map.remove(first);
        return;
    }
    if let Some(LoraValue::Map(inner)) = map.get_mut(first) {
        remove_path_value(inner, rest);
    }
}

fn insert_path(target: &mut BTreeMap<String, LoraValue>, parts: &[&str], value: LoraValue) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        target.insert(parts[0].to_string(), value);
        return;
    }
    let key = parts[0].to_string();
    let entry = target
        .entry(key)
        .or_insert_with(|| LoraValue::Map(BTreeMap::new()));
    if let LoraValue::Map(inner) = entry {
        insert_path(inner, &parts[1..], value);
    }
}

fn entries(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    let sorted = matches!(args.get(1), Some(LoraValue::Bool(true)));
    let mut pairs: Vec<(String, LoraValue)> =
        m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if sorted {
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
    }
    LoraValue::List(
        pairs
            .into_iter()
            .map(|(k, v)| LoraValue::List(vec![LoraValue::String(k), v]))
            .collect(),
    )
}

fn values(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    let out: Vec<LoraValue> = match args.get(1) {
        Some(LoraValue::List(keys)) => keys
            .iter()
            .filter_map(|k| match k {
                LoraValue::String(k) => m.get(k).cloned(),
                _ => None,
            })
            .collect(),
        _ => m.values().cloned().collect(),
    };
    LoraValue::List(out)
}

fn keys(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::List(m.keys().cloned().map(LoraValue::String).collect())
}

fn has_key(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(LoraValue::String(k))) = (as_map(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    LoraValue::Bool(m.contains_key(k))
}

fn pick(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(keys)) = (as_map(args.first()), as_list(args.get(1))) else {
        return LoraValue::Null;
    };
    let mut out = BTreeMap::new();
    for key in keys {
        let LoraValue::String(key) = key else {
            return LoraValue::Null;
        };
        if let Some(value) = m.get(key) {
            out.insert(key.clone(), value.clone());
        }
    }
    LoraValue::Map(out)
}

fn rename(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(LoraValue::String(from)), Some(LoraValue::String(to))) =
        (as_map(args.first()), args.get(1), args.get(2))
    else {
        return LoraValue::Null;
    };
    let mut out = m.clone();
    if let Some(value) = out.remove(from) {
        out.insert(to.clone(), value);
    }
    LoraValue::Map(out)
}

fn invert(args: &[LoraValue]) -> LoraValue {
    let Some(m) = as_map(args.first()) else {
        return LoraValue::Null;
    };
    LoraValue::Map(
        m.iter()
            .map(|(k, v)| (stringify(v), LoraValue::String(k.clone())))
            .collect(),
    )
}

fn get(args: &[LoraValue]) -> LoraValue {
    let (Some(m), Some(LoraValue::String(k))) = (as_map(args.first()), args.get(1)) else {
        return LoraValue::Null;
    };
    match m.get(k).cloned() {
        Some(v) => v,
        None => args.get(2).cloned().unwrap_or(LoraValue::Null),
    }
}

fn size(args: &[LoraValue]) -> LoraValue {
    match as_map(args.first()) {
        Some(m) => LoraValue::Int(m.len() as i64),
        None => LoraValue::Null,
    }
}

fn stringify(v: &LoraValue) -> String {
    match v {
        LoraValue::String(s) => s.clone(),
        LoraValue::Int(i) => i.to_string(),
        LoraValue::Float(f) => f.to_string(),
        LoraValue::Bool(b) => b.to_string(),
        LoraValue::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}
