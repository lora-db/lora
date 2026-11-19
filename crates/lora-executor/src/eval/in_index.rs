//! A hash index over a list that `x IN list` tests many times in one query
//! (taste N-10: a check against 1000 strings scanned the list, ~3 µs).
//!
//! The list must stay put while the index is used: either a value the row
//! shares behind an `Arc` (a list collected earlier, or one inside a
//! carried map: `x IN m.l`), whose `Arc` the cache holds so the address is
//! not reused, or a parameter, which lives for the whole query. Entries are
//! dropped when the outermost query scope on the thread ends
//! ([`clear`], called from `cancel::DeadlineScope`), and only made while one
//! is open.
//!
//! Only scalar lists are indexed: strings, booleans, nulls and numbers whose
//! `=` agrees with a hash (integers within ±2^53, floats). Anything else, on
//! either side, falls back to the scan, so the answer is always the scan's.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::value::LoraValue;

use super::binops::value_eq;

/// A list shorter than this is scanned: building an index would cost more.
const MIN_LEN: usize = 32;
/// Lists indexed at once per thread; more clears the cache.
const MAX_ENTRIES: usize = 16;
/// Lookups of one list before it is indexed: a list tested once or twice is
/// scanned.
const BUILD_AFTER: u32 = 2;
/// Integers beyond this compare equal to floats that are not their own
/// value, so they have no hash `=` can agree with.
const EXACT: i64 = 1 << 53;

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
    static DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
struct Cache {
    entries: HashMap<(usize, usize, usize), Entry>,
}

struct Entry {
    /// Keeps the list's memory alive (and unchanged: an `Arc` with two
    /// owners is never mutated in place). `None` for a parameter.
    _owner: Option<Arc<LoraValue>>,
    seen: u32,
    index: Option<Index>,
}

enum Index {
    /// The list holds an element `=` cannot be hashed for.
    Unhashable,
    Built {
        /// Positions of the elements with each hash.
        positions: HashMap<u64, smallvec::SmallVec<u32, 1>>,
        has_null: bool,
    },
}

/// Called by `DeadlineScope` on entry and exit.
pub(crate) fn scope_enter() {
    DEPTH.with(|d| d.set(d.get() + 1));
}

pub(crate) fn scope_exit() {
    let depth = DEPTH.with(|d| {
        let n = d.get().saturating_sub(1);
        d.set(n);
        n
    });
    if depth == 0 {
        clear();
    }
}

fn clear() {
    CACHE.with(|c| {
        if let Ok(mut c) = c.try_borrow_mut() {
            c.entries.clear();
        }
    });
}

/// Where the list came from, for keying and keeping it alive.
pub(super) enum ListOwner<'a> {
    /// A row value shared behind this `Arc`; the list is it or inside it.
    Shared(&'a Arc<LoraValue>),
    /// A query parameter.
    Parameter,
}

/// `lhs IN items` through the cache, or `None` when it cannot answer and
/// the caller scans.
pub(super) fn lookup(
    owner: ListOwner<'_>,
    items: &[LoraValue],
    lhs: &LoraValue,
) -> Option<LoraValue> {
    if items.len() < MIN_LEN || DEPTH.with(|d| d.get()) == 0 {
        return None;
    }
    let lhs_hash = hash_scalar(lhs)?;
    let root = match &owner {
        ListOwner::Shared(arc) => Arc::as_ptr(arc) as usize,
        ListOwner::Parameter => 0,
    };
    let key = (root, items.as_ptr() as usize, items.len());
    CACHE.with(|cache| {
        let mut cache = cache.try_borrow_mut().ok()?;
        if !cache.entries.contains_key(&key) && cache.entries.len() >= MAX_ENTRIES {
            cache.entries.clear();
        }
        let entry = cache.entries.entry(key).or_insert_with(|| Entry {
            _owner: match owner {
                ListOwner::Shared(arc) => Some(Arc::clone(arc)),
                ListOwner::Parameter => None,
            },
            seen: 0,
            index: None,
        });
        if entry.index.is_none() {
            entry.seen += 1;
            if entry.seen <= BUILD_AFTER {
                return None;
            }
            entry.index = Some(build(items));
        }
        match entry.index.as_ref()? {
            Index::Unhashable => None,
            Index::Built {
                positions,
                has_null,
            } => {
                let hit = positions.get(&lhs_hash).is_some_and(|at| {
                    at.iter()
                        .any(|&i| items.get(i as usize).is_some_and(|v| value_eq(lhs, v)))
                });
                Some(if hit {
                    LoraValue::Bool(true)
                } else if *has_null {
                    LoraValue::Null
                } else {
                    LoraValue::Bool(false)
                })
            }
        }
    })
}

fn build(items: &[LoraValue]) -> Index {
    let mut positions: HashMap<u64, smallvec::SmallVec<u32, 1>> =
        HashMap::with_capacity(items.len());
    let mut has_null = false;
    for (i, item) in items.iter().enumerate() {
        match item {
            LoraValue::Null => has_null = true,
            // NaN equals nothing: it can never be found.
            LoraValue::Float(f) if f.is_nan() => {}
            _ => match hash_scalar(item) {
                Some(h) => positions.entry(h).or_default().push(i as u32),
                None => return Index::Unhashable,
            },
        }
    }
    Index::Built {
        positions,
        has_null,
    }
}

/// A hash of `value` that agrees with `=`: equal values hash alike, and a
/// whole float hashes as the integer it equals. `None` for a value outside
/// that set (null, containers, temporals, an integer beyond ±2^53, NaN).
fn hash_scalar(value: &LoraValue) -> Option<u64> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match value {
        LoraValue::Bool(b) => (0u8, b).hash(&mut h),
        LoraValue::Int(i) if (-EXACT..=EXACT).contains(i) => (1u8, i).hash(&mut h),
        LoraValue::Float(f) if f.is_nan() => return None,
        LoraValue::Float(f) => {
            if f.trunc() == *f && f.abs() <= EXACT as f64 {
                // `1.0 = 1`. Adding 0.0 folds -0.0 into 0.
                (1u8, (*f + 0.0) as i64).hash(&mut h);
            } else {
                // A fraction, or beyond 2^53 where integers are unhashable
                // anyway: equal only to the same float.
                (2u8, (*f + 0.0).to_bits()).hash(&mut h);
            }
        }
        LoraValue::String(s) => (3u8, s.as_str()).hash(&mut h),
        _ => return None,
    }
    Some(h.finish())
}
