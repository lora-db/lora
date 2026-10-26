//! Value-size and nesting limits (see [`lora_ast::limits`]).
//!
//! Every recursive walk over a [`LoraValue`] (clone, drop, equality,
//! ordering, hashing, serialisation, the storage codecs) is safe only while
//! values stay shallow, and one expression must not be able to claim
//! unbounded memory. Values enter a query from parameters, which
//! [`check_parameters`] vets, from stored properties, which were vetted when
//! written, and from expressions, which check each container they build
//! with [`check_built`]. A value stored as a property is checked by
//! [`check_storable`].
//!
//! The walks here are iterative, so they are safe on values of any depth.

use std::collections::BTreeMap;

pub use lora_ast::limits::{MAX_VALUE_BYTES, MAX_VALUE_NESTING, MAX_VAR_LENGTH_HOPS};

use crate::value::LoraValue;

/// What a list element, map entry or scalar counts for in
/// [`MAX_VALUE_BYTES`], on top of the string or byte content it holds.
pub const ELEMENT_BYTES: usize = 8;

/// The error a value nested past [`MAX_VALUE_NESTING`] fails with.
pub fn too_deep_message() -> String {
    format!("value nests lists and maps more than {MAX_VALUE_NESTING} levels deep")
}

/// The error a value over [`MAX_VALUE_BYTES`] fails with.
pub fn too_large_message() -> String {
    format!(
        "value is larger than the {} MiB limit for a single value",
        MAX_VALUE_BYTES >> 20
    )
}

/// The nesting depth of `value` (1 for a list or map of scalars, 0 for a
/// scalar) and its size as counted for [`MAX_VALUE_BYTES`]. Stops early,
/// returning a depth or size over the limit, once either is exceeded.
pub fn measure(value: &LoraValue) -> (usize, usize) {
    measure_all(std::iter::once(value))
}

/// [`measure`] over the items of a list: the deepest item and their total
/// size.
pub fn measure_list(items: &[LoraValue]) -> (usize, usize) {
    measure_all(items)
}

/// [`measure`] over several values: the deepest depth and the total size.
fn measure_all<'a>(values: impl IntoIterator<Item = &'a LoraValue>) -> (usize, usize) {
    let mut m = Measure::default();
    for value in values {
        m.value(value);
        if m.over() {
            break;
        }
    }
    (m.max_depth, m.bytes)
}

/// Running totals of a depth-first walk. The walk keeps one iterator per
/// open container, so it holds at most `MAX_VALUE_NESTING + 1` of them and
/// never allocates per element.
#[derive(Default)]
struct Measure<'a> {
    max_depth: usize,
    bytes: usize,
    /// The open containers, innermost last; reused across values.
    open: Vec<Children<'a>>,
}

enum Children<'a> {
    List(std::slice::Iter<'a, LoraValue>),
    Map(std::collections::btree_map::Iter<'a, String, LoraValue>),
}

impl<'a> Measure<'a> {
    fn over(&self) -> bool {
        self.max_depth > MAX_VALUE_NESTING || self.bytes > MAX_VALUE_BYTES
    }

    /// Count `value`; return its children when it is a container.
    #[inline]
    fn scalar_or_children(&mut self, value: &'a LoraValue) -> Option<Children<'a>> {
        self.bytes = self.bytes.saturating_add(ELEMENT_BYTES);
        match value {
            LoraValue::String(s) => self.bytes = self.bytes.saturating_add(s.len()),
            LoraValue::Binary(b) => self.bytes = self.bytes.saturating_add(b.len()),
            LoraValue::Path(p) => {
                self.bytes = self
                    .bytes
                    .saturating_add((p.nodes.len() + p.rels.len()).saturating_mul(ELEMENT_BYTES))
            }
            LoraValue::Vector(v) => {
                self.bytes = self
                    .bytes
                    .saturating_add(v.dimension.saturating_mul(ELEMENT_BYTES))
            }
            LoraValue::List(items) => return Some(Children::List(items.iter())),
            LoraValue::Map(map) => return Some(Children::Map(map.iter())),
            _ => {}
        }
        None
    }

    fn value(&mut self, value: &'a LoraValue) {
        // A list of scalars, the common container, needs no walk state.
        if let LoraValue::List(items) = value {
            if let Some(bytes) = flat_list_bytes(items) {
                self.bytes = self.bytes.saturating_add(bytes);
                self.max_depth = self.max_depth.max(1);
                return;
            }
        }
        let Some(children) = self.scalar_or_children(value) else {
            return;
        };
        let mut open = std::mem::take(&mut self.open);
        open.push(children);
        self.max_depth = self.max_depth.max(1);
        'walk: while let Some(top) = open.last_mut() {
            let found = match top {
                // Scalars, the common case, are counted in a tight loop.
                Children::List(items) => loop {
                    let Some(item) = items.next() else {
                        break None;
                    };
                    match item {
                        LoraValue::List(_)
                        | LoraValue::Map(_)
                        | LoraValue::String(_)
                        | LoraValue::Binary(_)
                        | LoraValue::Path(_)
                        | LoraValue::Vector(_) => break Some(item),
                        _ => self.bytes = self.bytes.saturating_add(ELEMENT_BYTES),
                    }
                },
                Children::Map(entries) => entries.next().map(|(key, item)| {
                    self.bytes = self.bytes.saturating_add(key.len());
                    item
                }),
            };
            let Some(item) = found else {
                open.pop();
                if self.bytes > MAX_VALUE_BYTES {
                    break 'walk;
                }
                continue;
            };
            if let Some(children) = self.scalar_or_children(item) {
                open.push(children);
                self.max_depth = self.max_depth.max(open.len());
            }
            if self.over() {
                break;
            }
        }
        open.clear();
        self.open = open;
    }
}

/// The size of a list holding only scalars and strings, or `None` when it
/// holds a container.
#[inline]
fn flat_list_bytes(items: &[LoraValue]) -> Option<usize> {
    let mut bytes = ELEMENT_BYTES.saturating_mul(items.len() + 1);
    for item in items {
        match item {
            LoraValue::String(s) => bytes = bytes.saturating_add(s.len()),
            LoraValue::List(_)
            | LoraValue::Map(_)
            | LoraValue::Binary(_)
            | LoraValue::Path(_)
            | LoraValue::Vector(_) => return None,
            _ => {}
        }
    }
    Some(bytes)
}

/// Check a list or map an expression is about to build from `items`: it
/// nests one level deeper than its deepest item, and holds them all.
pub fn check_built<'a>(items: impl IntoIterator<Item = &'a LoraValue>) -> Result<(), String> {
    let (depth, bytes) = measure_all(items);
    check_built_measured(depth, bytes)
}

/// [`check_built`] for a new list, `items`.
#[inline]
pub fn check_built_list(items: &[LoraValue]) -> Result<(), String> {
    if let Some(bytes) = flat_list_bytes(items) {
        return if bytes > MAX_VALUE_BYTES {
            Err(too_large_message())
        } else {
            Ok(())
        };
    }
    check_built(items)
}

fn check_built_measured(depth: usize, bytes: usize) -> Result<(), String> {
    if depth >= MAX_VALUE_NESTING {
        Err(too_deep_message())
    } else if bytes > MAX_VALUE_BYTES {
        Err(too_large_message())
    } else {
        Ok(())
    }
}

/// [`check_built`] for nesting only, for lists whose size is bounded by
/// the data rather than by the expression (`collect`).
pub fn check_built_nesting<'a>(
    items: impl IntoIterator<Item = &'a LoraValue>,
) -> Result<(), String> {
    for item in items {
        if matches!(item, LoraValue::List(_) | LoraValue::Map(_))
            && nesting(item) >= MAX_VALUE_NESTING
        {
            return Err(too_deep_message());
        }
    }
    Ok(())
}

/// The nesting depth of `value`, counting no further than one past the
/// limit.
fn nesting(value: &LoraValue) -> usize {
    let mut max_depth = 0usize;
    let mut stack: Vec<(&LoraValue, usize)> = vec![(value, 0)];
    while let Some((value, depth)) = stack.pop() {
        let children: Box<dyn Iterator<Item = &LoraValue>> = match value {
            LoraValue::List(items) => Box::new(items.iter()),
            LoraValue::Map(map) => Box::new(map.values()),
            _ => continue,
        };
        let depth = depth + 1;
        max_depth = max_depth.max(depth);
        if depth > MAX_VALUE_NESTING {
            break;
        }
        stack.extend(
            children
                .filter(|v| matches!(v, LoraValue::List(_) | LoraValue::Map(_)))
                .map(|v| (v, depth)),
        );
    }
    max_depth
}

/// Check `a ++ b`, the concatenation of two lists that are each within the
/// limits, or (with `b` a one-element slice) `a` with an element added.
///
/// Nesting can only grow by the added element, so only `b` is walked for
/// it. The size is the sum of both, but walking `a` on every append would
/// make the common `reduce(acc = [], x IN list | acc + [x])` pay a full walk
/// per element: `a` is walked only when the length crosses a power of two
/// or `b` is large, which amortises to a constant per element. Between
/// those walks a list grows by small appends, each of which copies the
/// whole list anyway, so it cannot run far past the limit.
pub fn check_concat(a: &[LoraValue], b: &[LoraValue], b_is_element: bool) -> Result<(), String> {
    let (depth, b_bytes) = measure_all(b);
    let limit = if b_is_element {
        MAX_VALUE_NESTING
    } else {
        MAX_VALUE_NESTING + 1
    };
    if depth >= limit {
        return Err(too_deep_message());
    }
    let len = a.len() + b.len();
    if len.saturating_mul(ELEMENT_BYTES) > MAX_VALUE_BYTES || b_bytes > MAX_VALUE_BYTES {
        return Err(too_large_message());
    }
    let crosses_power_of_two = (a.len() | 1).leading_zeros() != (len | 1).leading_zeros();
    if crosses_power_of_two || b_bytes >= LARGE_APPEND_BYTES {
        let (_, a_bytes) = measure_all(a);
        if a_bytes.saturating_add(b_bytes) > MAX_VALUE_BYTES {
            return Err(too_large_message());
        }
    }
    Ok(())
}

/// See [`check_concat`].
const LARGE_APPEND_BYTES: usize = 1 << 20;

/// Check a value an expression produced without adding a level (a
/// concatenation, a repeat, ...).
pub fn check_value(value: &LoraValue) -> Result<(), String> {
    if let LoraValue::List(items) = value {
        return check_built_list(items);
    }
    let (depth, bytes) = measure(value);
    if depth > MAX_VALUE_NESTING {
        Err(too_deep_message())
    } else if bytes > MAX_VALUE_BYTES {
        Err(too_large_message())
    } else {
        Ok(())
    }
}

/// Check that `len` elements of `each` bytes (beyond [`ELEMENT_BYTES`])
/// fit in one value, before allocating them.
pub fn check_len(len: usize, each: usize) -> Result<(), String> {
    match len.checked_mul(each.saturating_add(ELEMENT_BYTES)) {
        Some(bytes) if bytes <= MAX_VALUE_BYTES => Ok(()),
        _ => Err(too_large_message()),
    }
}

/// Check that a string or byte array of `len` bytes fits, before building
/// it.
pub fn check_bytes(len: usize) -> Result<(), String> {
    if len <= MAX_VALUE_BYTES {
        Ok(())
    } else {
        Err(too_large_message())
    }
}

/// Check a value about to be stored as a property.
pub fn check_storable(value: &LoraValue) -> Result<(), String> {
    check_value(value)
}

/// Refuse parameters nested deeper than [`MAX_VALUE_NESTING`] or larger
/// than [`MAX_VALUE_BYTES`]. On refusal every parameter is dismantled
/// iteratively, so dropping them afterwards cannot overflow the stack
/// however deep they are.
pub fn check_parameters(params: &mut BTreeMap<String, LoraValue>) -> Result<(), String> {
    let mut failure = None;
    for (name, value) in params.iter() {
        let (depth, bytes) = measure(value);
        if depth > MAX_VALUE_NESTING {
            failure = Some(format!(
                "parameter ${name} nests lists and maps more than {MAX_VALUE_NESTING} levels deep"
            ));
        } else if bytes > MAX_VALUE_BYTES {
            failure = Some(format!(
                "parameter ${name} is larger than the {} MiB limit for a single value",
                MAX_VALUE_BYTES >> 20
            ));
        }
        if failure.is_some() {
            break;
        }
    }
    match failure {
        None => Ok(()),
        Some(message) => {
            for (_, value) in std::mem::take(params) {
                drop_deep(value);
            }
            Err(message)
        }
    }
}

/// Drop `value` without recursion, so a value of any depth can be freed.
pub fn drop_deep(value: LoraValue) {
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            LoraValue::List(items) => stack.extend(items),
            LoraValue::Map(map) => stack.extend(map.into_values()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested(depth: usize) -> LoraValue {
        let mut v = LoraValue::Int(1);
        for _ in 0..depth {
            v = LoraValue::List(vec![v]);
        }
        v
    }

    #[test]
    fn measures_depth_and_size() {
        assert_eq!(measure(&LoraValue::Int(1)), (0, ELEMENT_BYTES));
        assert_eq!(measure(&nested(3)).0, 3);
        let s = LoraValue::String("abc".into());
        assert_eq!(measure(&s).1, ELEMENT_BYTES + 3);
    }

    #[test]
    fn parameters_at_the_limit_pass_and_deeper_ones_are_dismantled() {
        let mut params = BTreeMap::new();
        params.insert("ok".to_string(), nested(MAX_VALUE_NESTING));
        check_parameters(&mut params).unwrap();
        // Deep enough that a recursive drop would overflow the stack.
        params.insert("deep".to_string(), nested(1_000_000));
        let err = check_parameters(&mut params).unwrap_err();
        assert!(err.contains("$deep"), "{err}");
        assert!(params.is_empty());
    }

    #[test]
    fn built_containers_stay_within_the_nesting_limit() {
        check_built([&nested(MAX_VALUE_NESTING - 1)]).unwrap();
        assert!(check_built([&nested(MAX_VALUE_NESTING)]).is_err());
    }

    #[test]
    fn sizes_are_bounded() {
        assert!(check_len(usize::MAX, 0).is_err());
        assert!(check_bytes(MAX_VALUE_BYTES + 1).is_err());
        check_bytes(MAX_VALUE_BYTES).unwrap();
    }
}
