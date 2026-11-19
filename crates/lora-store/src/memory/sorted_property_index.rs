//! Sorted property index — a `BTreeMap`-backed companion to the
//! hash-bucket [`super::property_index::PropertyIndex`].
//!
//! The hash index answers point equality in O(1); the sorted index
//! lets the optimizer answer range predicates (`>`, `<`, `BETWEEN`,
//! prefix `STARTS WITH` over strings) without scanning. Both
//! structures are populated for `RANGE` catalog entries; the
//! optimizer chooses between them based on the predicate shape.
//!
//! ## Why a separate structure
//!
//! `PropertyIndex` already stores `(value → ids)`; making it sorted
//! by switching `HashMap<PropertyIndexKey, …>` to `BTreeMap` would
//! reorder semantics for every existing caller. Keeping the structures
//! parallel means we pay for sort order only when a `RANGE` catalog
//! entry exists and a range predicate is on the hot path.
//!
//! ## Comparison key
//!
//! Reuses [`super::property_index::PropertyIndexKey`] so the same
//! `PropertyValue → indexable key` projection rules apply. `Ord` follows
//! the variant ordering in `PropertyIndexKey` (integers and floats share
//! one run, by numeric value) plus lexicographic ordering for the inner
//! data (strings, lists, maps).
//!
//! ## List elements
//!
//! A list value is indexed twice: whole, in `by_value` (for `n.l = [..]`
//! and ranges over lists), and by each distinct element, in `by_element`
//! (for `$x IN n.l`). Element keys put every number at its `f64` image,
//! as Cypher compares an integer with a float (`1 = 1.0`): a seek finds
//! every element equal to the value sought, plus, for integers beyond
//! 2^53, the neighbours that share its image, which the caller refilters.
//! Null elements are not indexed (`x IN [null]` is never true), nor are
//! elements without an index image (durations, points, vectors, NaN).

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use crate::types::PropertyValue;

use super::cow::{CowMap, CowOrdMap};
use super::entity_index_store::{ScopeKey, ScopedPropertyKey};
use super::id_set::IdSet;
use super::property_index::PropertyIndexKey;
use super::str_postings::{StrKey, StrPostings, StrPostingsBuilder};
use crate::scratch::ScratchVec;

/// Sorted bucket: every value seen for an indexed property mapped to
/// the ids carrying that value. The outer map is keyed by
/// `(label-or-type, property)` so range queries don't have to scan
/// across labels.
#[derive(Debug, Default, Clone)]
pub(super) struct SortedPropertyIndex {
    pub(super) by_scope: BTreeMap<ScopedPropertyKey, SortedScope>,
    /// Composite scopes in `by_scope`: writes skip composite maintenance
    /// while there are none.
    composites: usize,
}

#[derive(Debug, Default, Clone)]
pub(super) struct SortedScope {
    /// Keyed by sortable property key, every key but strings (those are
    /// in `strings`). Values are ids in that bucket. Copy-on-write
    /// partitions (see [`CowOrdMap`]): a write's staged graph copy shares
    /// them and copies only the path (root, group, partition) to the
    /// entry it changes.
    pub(super) by_value: CowOrdMap<PropertyIndexKey, IdSet>,
    /// String keys, packed: their own `Arc<str>` each cost a 58-byte key
    /// about 110 bytes (G-64). Strings sort after numbers and before
    /// byte strings, so reads splice them in between (see
    /// [`SortedScope::entries`]).
    pub(super) strings: StrPostings,
    /// Element → ids of the entities whose list value holds it (see
    /// "List elements" above). Empty unless the property holds lists.
    pub(super) by_element: CowMap<PropertyIndexKey, IdSet>,
    /// Ids whose value has no key (a duration, point, vector, NaN, or a
    /// list holding one). They are in no range, but they are not null:
    /// an unbounded probe (`n.p IS NOT NULL`) lists them too. Few by
    /// construction: a RANGE index on such a property serves nothing.
    pub(super) unkeyed: BTreeSet<u64>,
    /// Set on a composite scope (see [`Composite`]).
    pub(super) composite: Option<Box<Composite>>,
    /// Sum of the `by_element` bucket sizes: with the bucket count, the
    /// average number of entities one element seek finds.
    element_postings: usize,
    /// Refcount of catalog entries pointing at this scope.
    refcount: u32,
}

/// A composite RANGE index `(n.a, n.b, ...)` as a sorted scope: each entity
/// holding every component is filed under the tuple of their keys, so an
/// equality on the leading components and a range or order on the last is
/// one contiguous, ordered run (taste N-06: a person's timeline by
/// `(person, at)`). Entities missing a component, or with a component
/// that has no key, are not in it.
///
/// The scope is filed under [`composite_property`] of its components.
#[derive(Debug, Clone)]
pub(super) struct Composite {
    pub(super) components: std::sync::Arc<[String]>,
    /// Each entity's current tuple. Maintenance re-derives an entity's
    /// tuple from its record and moves it, so several properties changed in
    /// one write cannot leave a stale entry behind.
    by_id: super::id_map::CowIdMap<PropertyIndexKey>,
}

const COMPOSITE_PREFIX: &str = "\0composite";

/// The scope name of a composite index on `components`, never a property
/// name a query can write (a NUL cannot be in an identifier).
pub(crate) fn composite_property(components: &[String]) -> String {
    let mut name = String::from(COMPOSITE_PREFIX);
    for c in components {
        name.push('\0');
        name.push_str(c);
    }
    name
}

/// The key a composite component is filed under: [`PropertyIndexKey`],
/// with a whole float as the integer it equals, so `{at: 5}` and
/// `{at: 5.0}` share a tuple prefix (`5 = 5.0`). `None` for a value with
/// no key.
pub(super) fn component_key(value: &PropertyValue) -> Option<PropertyIndexKey> {
    match value {
        PropertyValue::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => {
            Some(PropertyIndexKey::Int(*f as i64))
        }
        PropertyValue::Null => None,
        other => PropertyIndexKey::from_value(other),
    }
}

/// The tuple key of an entity from its property values, `None` unless it
/// holds every component with a key.
pub(super) fn composite_key(
    components: &[String],
    mut get: impl FnMut(&str) -> Option<PropertyValue>,
) -> Option<PropertyIndexKey> {
    components
        .iter()
        .map(|c| component_key(&get(c)?))
        .collect::<Option<Box<[_]>>>()
        .map(PropertyIndexKey::List)
}

/// A key above every component key: the end of a tuple prefix's run.
fn component_ceiling() -> PropertyIndexKey {
    PropertyIndexKey::Temporal {
        kind: super::property_index::TemporalKind::DateTime,
        nanos: i128::MAX.into(),
        offset: i32::MAX,
        zone: Some(crate::ZoneId::MAX),
    }
}

/// A key read from a scope: one of `by_value`'s, or a packed string.
#[derive(Clone, Copy)]
pub(super) enum KeyRef<'a> {
    Key(&'a PropertyIndexKey),
    Str(StrKey<'a>),
}

impl KeyRef<'_> {
    /// Whether this is `key`.
    fn is(&self, key: &PropertyIndexKey) -> bool {
        match (self, key) {
            (KeyRef::Key(k), key) => *k == key,
            (KeyRef::Str(s), PropertyIndexKey::String(t)) => s.eq_str(t),
            (KeyRef::Str(_), _) => false,
        }
    }

    /// The key as `by_value` would hold it.
    #[cfg(test)]
    pub(super) fn to_key(self) -> PropertyIndexKey {
        match self {
            KeyRef::Key(k) => k.clone(),
            KeyRef::Str(s) => PropertyIndexKey::String(s.to_arc()),
        }
    }
}

/// The smallest string key: every key `by_value` holds below it sorts
/// before every string.
fn first_string() -> &'static PropertyIndexKey {
    static KEY: std::sync::OnceLock<PropertyIndexKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| PropertyIndexKey::String(std::sync::Arc::from("")))
}

/// The smallest key after every string: the empty byte string.
fn after_strings() -> &'static PropertyIndexKey {
    static KEY: std::sync::OnceLock<PropertyIndexKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        PropertyIndexKey::Binary(Box::new(crate::LoraBinary::from_bytes(Vec::new())))
    })
}

/// Where `key` sorts against the strings: before them, among them or
/// after them.
fn against_strings(key: &PropertyIndexKey) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match key {
        PropertyIndexKey::String(_) => Ordering::Equal,
        key if key < first_string() => Ordering::Less,
        _ => Ordering::Greater,
    }
}

/// `bound` on the strings, if it lets any through: a bound below every
/// string is no bound, one above them all admits none.
fn string_bound(bound: &Bound<PropertyIndexKey>, lower: bool) -> Option<Bound<&str>> {
    use std::cmp::Ordering;
    let key = match bound {
        Bound::Unbounded => return Some(Bound::Unbounded),
        Bound::Included(k) | Bound::Excluded(k) => k,
    };
    match (key, against_strings(key)) {
        (PropertyIndexKey::String(s), _) => Some(match bound {
            Bound::Included(_) => Bound::Included(&**s),
            _ => Bound::Excluded(&**s),
        }),
        (_, Ordering::Less) => lower.then_some(Bound::Unbounded),
        _ => (!lower).then_some(Bound::Unbounded),
    }
}

impl SortedScope {
    /// Entries with keys in `(lower, upper)`, in key order (reversible):
    /// `by_value`'s keys below the strings, the strings, then the rest.
    pub(super) fn entries<'a>(
        &'a self,
        lower: &'a Bound<PropertyIndexKey>,
        upper: &'a Bound<PropertyIndexKey>,
    ) -> impl DoubleEndedIterator<Item = (KeyRef<'a>, &'a IdSet)> + 'a {
        use std::cmp::Ordering;
        // An inverted range is empty in every part: `range_by` and
        // `StrPostings::range` check their own bounds.
        let below_upper = match upper {
            Bound::Included(u) | Bound::Excluded(u) if against_strings(u) == Ordering::Less => {
                upper.as_ref()
            }
            _ => Bound::Excluded(first_string()),
        };
        let above_lower = match lower {
            Bound::Included(l) | Bound::Excluded(l) if against_strings(l) == Ordering::Greater => {
                lower.as_ref()
            }
            _ => Bound::Included(after_strings()),
        };
        let below = self.by_value.range_by(lower.as_ref(), below_upper);
        let above = self.by_value.range_by(above_lower, upper.as_ref());
        let strings = match (string_bound(lower, true), string_bound(upper, false)) {
            (Some(l), Some(u)) => Some(self.strings.range(l, u)),
            _ => None,
        };
        below
            .map(|(k, ids)| (KeyRef::Key(k), ids))
            .chain(
                strings
                    .into_iter()
                    .flatten()
                    .map(|(k, ids)| (KeyRef::Str(k), ids)),
            )
            .chain(above.map(|(k, ids)| (KeyRef::Key(k), ids)))
    }
}

impl SortedPropertyIndex {
    /// Whether any of `scopes` has an index on `property`. Checked through
    /// a read guard before maintenance takes a (copy-on-write) write guard.
    pub(super) fn covers_any<'a>(
        &self,
        scopes: impl IntoIterator<Item = &'a str>,
        property: &str,
    ) -> bool {
        !self.by_scope.is_empty()
            && scopes.into_iter().any(|scope| {
                self.by_scope
                    .contains_key(&(scope, property) as &dyn ScopeKey)
            })
    }

    pub(super) fn add_scope(&mut self, label: &str, property: &str) -> bool {
        let entry = self
            .by_scope
            .entry(ScopedPropertyKey::new(label, property))
            .or_default();
        let was_empty = entry.refcount == 0;
        entry.refcount = entry.refcount.saturating_add(1);
        was_empty
    }

    pub(super) fn has_scope(&self, label: &str, property: &str) -> bool {
        self.by_scope
            .contains_key(&(label, property) as &dyn ScopeKey)
    }

    /// Add `label`'s `property` scope as [`Self::add_scope`] does, holding
    /// `built`'s entries ([`build_scope`]) if it is new. Returns whether
    /// it was.
    pub(super) fn add_built_scope(
        &mut self,
        label: &str,
        property: &str,
        built: SortedScope,
    ) -> bool {
        let entry = self
            .by_scope
            .entry(ScopedPropertyKey::new(label, property))
            .or_default();
        let new = entry.refcount == 0;
        if new {
            entry.by_value = built.by_value;
            entry.strings = built.strings;
            entry.by_element = built.by_element;
            entry.unkeyed = built.unkeyed;
            entry.element_postings = built.element_postings;
        }
        entry.refcount = entry.refcount.saturating_add(1);
        new
    }

    /// Add a composite scope on `components` for `label`, refcounted like
    /// [`Self::add_scope`]. Returns whether it is new (and must be filled).
    pub(super) fn add_composite_scope(&mut self, label: &str, components: &[String]) -> bool {
        let entry = self
            .by_scope
            .entry(ScopedPropertyKey::new(
                label,
                &composite_property(components),
            ))
            .or_default();
        let new = entry.refcount == 0;
        if new {
            self.composites += 1;
            entry.composite = Some(Box::new(Composite {
                components: components.into(),
                by_id: Default::default(),
            }));
        }
        entry.refcount = entry.refcount.saturating_add(1);
        new
    }

    /// Whether `label` has a composite scope with `property` among its
    /// components (`None` for any property). Checked through a read guard
    /// before maintenance takes a write guard.
    pub(super) fn has_composites(&self) -> bool {
        self.composites != 0
    }

    /// The components of every composite scope of `label` that has
    /// `property` among them. Composite scope names start with a NUL, so
    /// they lead the label's run of scopes.
    pub(super) fn composites_with(
        &self,
        label: &str,
        property: &str,
    ) -> Vec<std::sync::Arc<[String]>> {
        if self.composites == 0 {
            return Vec::new();
        }
        let start: &dyn ScopeKey = &(label, COMPOSITE_PREFIX);
        self.by_scope
            .range::<dyn ScopeKey, _>((Bound::Included(start), Bound::Unbounded))
            .take_while(|(key, _)| key.label == label && key.property.starts_with(COMPOSITE_PREFIX))
            .filter_map(|(_, scope)| scope.composite.as_deref())
            .filter(|c| c.components.iter().any(|component| component == property))
            .map(|c| c.components.clone())
            .collect()
    }

    /// File `id` under `key` in `label`'s composite scope on `components`,
    /// moving it from the tuple it had; `None` takes it out.
    pub(super) fn set_composite(
        &mut self,
        label: &str,
        components: &[String],
        id: u64,
        key: Option<PropertyIndexKey>,
    ) {
        let property = composite_property(components);
        let Some(scope) = self
            .by_scope
            .get_mut(&(label, property.as_str()) as &dyn ScopeKey)
        else {
            return;
        };
        let Some(composite) = scope.composite.as_deref_mut() else {
            return;
        };
        if composite.by_id.get(&id) == key.as_ref() {
            return;
        }
        let old = match &key {
            Some(key) => composite.by_id.insert(id, key.clone()),
            None => composite.by_id.remove(&id),
        };
        if let Some(old) = old {
            Self::remove_from_scope(scope, id, &old);
        }
        if let Some(key) = key {
            insert_id(scope, key, id);
        }
    }

    /// Ids in `label`'s composite scope on `components` whose tuple starts
    /// with `prefix` and whose last component is in `[lo, hi]` (inclusive;
    /// the caller refilters), in tuple order then id order, strictly after
    /// the `(last component, id)` cursor, at most `max`. `None` when no
    /// such scope exists or a bound has no key.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn composite_chunk(
        &self,
        label: &str,
        components: &[String],
        prefix: &[PropertyValue],
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
        descending: bool,
        after: Option<(&PropertyValue, u64)>,
        max: usize,
    ) -> Option<Vec<u64>> {
        if prefix.len() + 1 != components.len() {
            return None;
        }
        let property = composite_property(components);
        let scope = self
            .by_scope
            .get(&(label, property.as_str()) as &dyn ScopeKey)?;
        scope.composite.as_ref()?;
        let prefix: Vec<PropertyIndexKey> =
            prefix.iter().map(component_key).collect::<Option<_>>()?;
        let tuple = |last: Option<PropertyIndexKey>, tail: Option<PropertyIndexKey>| {
            let mut keys = prefix.clone();
            keys.extend(last);
            keys.extend(tail);
            PropertyIndexKey::List(keys.into_boxed_slice())
        };
        // The run of the prefix, narrowed by the bounds on the last
        // component. `[p]` sorts before every `[p, x]`, and `[p, x, MAX]`
        // after `[p, x]` and before `[p, y]` for `y > x`.
        let mut lower_last = match lo {
            Some(v) => Some(component_bound(v, true)?),
            None => None,
        };
        let mut upper_last = match hi {
            Some(v) => Some(component_bound(v, false)?),
            None => None,
        };
        // A one-sided range stops at the end of its kind; with no bound at
        // all, the run is only in Cypher order when it holds one kind.
        match (&lower_last, &upper_last) {
            (Some(l), None) => upper_last = l.kind_ceiling(),
            (None, Some(h)) => lower_last = h.kind_floor(),
            (None, None) => {
                let (start, end) = (
                    tuple(None, None),
                    tuple(Some(component_ceiling()), Some(component_ceiling())),
                );
                let mut run = scope
                    .by_value
                    .range_by(Bound::Included(&start), Bound::Included(&end));
                let last = |key: Option<(&PropertyIndexKey, _)>| match key {
                    Some((PropertyIndexKey::List(parts), _)) => parts.last().cloned(),
                    _ => None,
                };
                let first = last(run.next());
                let end = last(run.next_back()).or_else(|| first.clone());
                match (first, end) {
                    (None, _) => return Some(Vec::new()),
                    (Some(a), Some(b)) if same_order_kind(&a, &b) => {}
                    _ => return None,
                }
            }
            _ => {}
        }
        let mut lower = Bound::Included(tuple(lower_last, None));
        let mut upper = Bound::Included(match upper_last {
            Some(h) => tuple(Some(h), Some(component_ceiling())),
            None => tuple(Some(component_ceiling()), Some(component_ceiling())),
        });
        let after = match after {
            Some((v, id)) => Some((tuple(Some(component_key(v)?), None), id)),
            None => None,
        };
        if let Some((key, _)) = &after {
            if descending {
                upper = Bound::Included(key.clone());
            } else {
                lower = Bound::Included(key.clone());
            }
        }
        if let (Bound::Included(l), Bound::Included(u)) = (&lower, &upper) {
            if l > u {
                return Some(Vec::new());
            }
        }
        let mut out = Vec::with_capacity(max.min(1024));
        let range = scope.by_value.range_by(lower.as_ref(), upper.as_ref());
        let mut push = |key: &PropertyIndexKey, ids: &IdSet| -> bool {
            let cursor_id = after.as_ref().filter(|(k, _)| k == key).map(|(_, id)| *id);
            for id in ids.iter_after(cursor_id, descending) {
                if out.len() >= max {
                    return true;
                }
                out.push(id);
            }
            out.len() >= max
        };
        if descending {
            for (key, ids) in range.rev() {
                if push(key, ids) {
                    break;
                }
            }
        } else {
            for (key, ids) in range {
                if push(key, ids) {
                    break;
                }
            }
        }
        Some(out)
    }

    pub(super) fn remove_scope(&mut self, label: &str, property: &str) {
        let key = ScopedPropertyKey::new(label, property);
        if let Some(scope) = self.by_scope.get_mut(&key) {
            scope.refcount = scope.refcount.saturating_sub(1);
            if scope.refcount == 0 {
                if scope.composite.is_some() {
                    self.composites -= 1;
                }
                self.by_scope.remove(&key);
            }
        }
    }

    /// Index `id` under a whole value: its key and, for a list, each
    /// of its elements.
    pub(super) fn insert(&mut self, label: &str, property: &str, id: u64, value: &PropertyValue) {
        if let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) {
            if let PropertyValue::String(s) = value {
                scope.strings.insert(s, id);
                return;
            }
            // A list holding a duration has no key as a whole, but its
            // other elements do.
            match PropertyIndexKey::from_value(value) {
                Some(key) => insert_id(scope, key, id),
                None => {
                    scope.unkeyed.insert(id);
                }
            }
            insert_elements(scope, value, id);
        }
    }

    /// Record `id` as holding a value with no key (see
    /// [`SortedScope::unkeyed`]).
    pub(super) fn insert_unkeyed(&mut self, label: &str, property: &str, id: u64) {
        if let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) {
            scope.unkeyed.insert(id);
        }
    }

    /// Index `id` under the string `value`, read from a record without
    /// making it a key.
    pub(super) fn insert_str(&mut self, label: &str, property: &str, id: u64, value: &str) {
        if let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) {
            scope.strings.insert(value, id);
        }
    }

    /// Index `id` under `key`, a value already made a key (from a record
    /// view, without decoding the value first). Only for values with no
    /// list elements: a list goes through [`Self::insert`].
    pub(super) fn insert_key(
        &mut self,
        label: &str,
        property: &str,
        id: u64,
        key: PropertyIndexKey,
    ) {
        if let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) {
            insert_id(scope, key, id);
        }
    }

    pub(super) fn update(
        &mut self,
        label: &str,
        property: &str,
        id: u64,
        old: Option<&PropertyValue>,
        new: Option<&PropertyValue>,
    ) {
        let Some(scope) = self.by_scope.get_mut(&(label, property) as &dyn ScopeKey) else {
            return;
        };
        // Strings go to `strings` as they are: making them keys first
        // allocated a copy per write, freed right after.
        match old {
            Some(PropertyValue::String(s)) => {
                scope.strings.remove(s, id);
            }
            Some(old) => {
                match PropertyIndexKey::from_value(old) {
                    Some(key) => Self::remove_from_scope(scope, id, &key),
                    None => {
                        scope.unkeyed.remove(&id);
                    }
                }
                remove_elements(scope, old, id);
            }
            None => {}
        }
        match new {
            Some(PropertyValue::String(s)) => scope.strings.insert(s, id),
            Some(new) => {
                match PropertyIndexKey::from_value(new) {
                    Some(key) => insert_id(scope, key, id),
                    None => {
                        scope.unkeyed.insert(id);
                    }
                }
                insert_elements(scope, new, id);
            }
            None => {}
        }
    }

    /// Ids whose list value holds an element equal to `element`, in
    /// ascending order; see "List elements" above for the integers this
    /// may add. `None` when the scope is not indexed or `element` has no
    /// index image (the caller scans); empty for a null `element`.
    pub(super) fn element_candidates(
        &self,
        label: &str,
        property: &str,
        element: &PropertyValue,
    ) -> Option<Vec<u64>> {
        let scope = self.by_scope.get(&(label, property) as &dyn ScopeKey)?;
        if matches!(element, PropertyValue::Null) {
            return Some(Vec::new());
        }
        let key = element_key(element)?;
        Some(
            scope
                .by_element
                .get(&key)
                .map(IdSet::to_vec)
                .unwrap_or_default(),
        )
    }

    /// `(label, property, distinct elements, element postings)` for every
    /// scope holding list elements, in scope order.
    pub(super) fn element_stats(&self) -> impl Iterator<Item = (&str, &str, usize, usize)> + '_ {
        self.by_scope
            .iter()
            .filter(|(_, scope)| !scope.by_element.is_empty())
            .map(|(key, scope)| {
                (
                    key.label.as_str(),
                    key.property.as_str(),
                    scope.by_element.len(),
                    scope.element_postings,
                )
            })
    }

    /// Range probe: every id whose value falls in `[lo, hi]`. Both
    /// bounds are inclusive at this layer — the caller refilters with
    /// the precise predicate inclusivity. `lo == None` means `-∞`;
    /// `hi == None` means `+∞`. Returns `None` when no scope exists,
    /// signalling "fall back to scan."
    pub(super) fn range_candidates(
        &self,
        label: &str,
        property: &str,
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
    ) -> Option<Vec<u64>> {
        let scope = self.by_scope.get(&(label, property) as &dyn ScopeKey)?;
        let (lower, upper) = probe_bounds(lo, hi)?;
        let mut out = Vec::new();
        if let (Bound::Included(l), Bound::Included(u)) = (&lower, &upper) {
            if l > u {
                return Some(out);
            }
        }
        extend_ids(&mut out, scope.entries(&lower, &upper));
        // No bounds: every id holding a value, keyed or not.
        if lo.is_none() && hi.is_none() {
            out.extend(scope.unkeyed.iter().copied());
        }
        // Gathered flat and sorted once: a set built id by id cost more
        // than the probe (an `IS NOT NULL` over a whole label ran a third
        // slower than scanning the label).
        out.sort_unstable();
        out.dedup();
        Some(out)
    }

    /// Whether `label`'s scope on `property` holds a value for at least `n`
    /// ids, counted bucket by bucket and stopped at `n`.
    pub(super) fn holds_at_least(&self, label: &str, property: &str, n: usize) -> bool {
        let Some(scope) = self.by_scope.get(&(label, property) as &dyn ScopeKey) else {
            return false;
        };
        let mut held = scope.unkeyed.len();
        for (_, ids) in scope.entries(&Bound::Unbounded, &Bound::Unbounded) {
            if held >= n {
                break;
            }
            held += ids.len();
        }
        held >= n
    }

    /// Ids whose value is a temporal of another kind than `like` (a DATE
    /// when `like` is a DATETIME, ...). Empty when `like` is not temporal;
    /// `None` when the scope is not indexed. Each temporal kind is one
    /// contiguous run of keys, so this reads only the other kinds' runs:
    /// two range probes plus the ids found.
    pub(super) fn other_temporal_kind_ids(
        &self,
        label: &str,
        property: &str,
        like: &PropertyValue,
    ) -> Option<Vec<u64>> {
        let scope = self.by_scope.get(&(label, property) as &dyn ScopeKey)?;
        let Some(key) = PropertyIndexKey::from_value(like).filter(PropertyIndexKey::is_temporal)
        else {
            return Some(Vec::new());
        };
        let (Some(kind_floor), Some(kind_ceiling)) = (key.kind_floor(), key.kind_ceiling()) else {
            return Some(Vec::new());
        };
        let (floor, ceiling) = PropertyIndexKey::all_temporals();
        let mut out = BTreeSet::new();
        // Temporals are never strings: `by_value` holds them all.
        for (_, ids) in scope
            .by_value
            .range(Bound::Included(floor), Bound::Excluded(kind_floor))
            .chain(
                scope
                    .by_value
                    .range(Bound::Excluded(kind_ceiling), Bound::Included(ceiling)),
            )
        {
            out.extend(ids.iter());
        }
        Some(out.into_iter().collect())
    }

    /// Ids in `[lo, hi]` (inclusive; the caller refilters exact bounds)
    /// in value order, then id order within a value, starting strictly
    /// after `after` and returning at most `max`. `descending` walks the
    /// same order backwards. `None` when the scope is not indexed.
    ///
    /// Paging by `(value, id)` cursor rather than holding an iterator lets
    /// a streaming scan pull one chunk at a time without keeping the index
    /// lock between pulls; a `LIMIT` above it stops after a few chunks.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ordered_chunk(
        &self,
        label: &str,
        property: &str,
        lo: Option<&PropertyValue>,
        hi: Option<&PropertyValue>,
        descending: bool,
        after: Option<(&PropertyValue, u64)>,
        max: usize,
    ) -> Option<Vec<u64>> {
        let scope = self.by_scope.get(&(label, property) as &dyn ScopeKey)?;
        let after = after.and_then(|(v, id)| PropertyIndexKey::from_value(v).map(|k| (k, id)));

        // Narrow the value range to start at the cursor's value.
        let (mut lower, mut upper) = probe_bounds(lo, hi)?;
        if let Some((key, _)) = &after {
            if descending {
                upper = Bound::Included(key.clone());
            } else {
                lower = Bound::Included(key.clone());
            }
        }
        if let (Bound::Included(l), Bound::Included(u)) = (&lower, &upper) {
            if l > u {
                return Some(Vec::new());
            }
        }

        let mut out = Vec::with_capacity(max.min(1024));
        // Returns true once the chunk is full.
        let mut push_bucket = |key: KeyRef<'_>, ids: &IdSet| -> bool {
            let cursor_id = after.as_ref().filter(|(k, _)| key.is(k)).map(|(_, id)| *id);
            for id in ids.iter_after(cursor_id, descending) {
                if out.len() >= max {
                    return true;
                }
                out.push(id);
            }
            out.len() >= max
        };
        let range = scope.entries(&lower, &upper);
        if descending {
            for (key, ids) in range.rev() {
                if push_bucket(key, ids) {
                    break;
                }
            }
        } else {
            for (key, ids) in range {
                if push_bucket(key, ids) {
                    break;
                }
            }
        }
        Some(out)
    }

    fn remove_from_scope(scope: &mut SortedScope, id: u64, key: &PropertyIndexKey) {
        if let PropertyIndexKey::String(s) = key {
            scope.strings.remove(s, id);
            return;
        }
        let emptied = scope
            .by_value
            .get_mut(key)
            .is_some_and(|bucket| bucket.remove(id));
        if emptied {
            scope.by_value.remove(key);
        }
    }
}

/// Whether every key from `a` to `b` is of one kind, ordered in the index
/// as Cypher orders it: numbers, strings, booleans, or one temporal kind.
/// Kinds are contiguous in key order, so the ends decide.
fn same_order_kind(a: &PropertyIndexKey, b: &PropertyIndexKey) -> bool {
    use PropertyIndexKey as K;
    match (a, b) {
        (K::Int(_) | K::Float(_), K::Int(_) | K::Float(_)) => true,
        (K::String(_), K::String(_)) | (K::Bool(_), K::Bool(_)) => true,
        (K::Temporal { kind: x, .. }, K::Temporal { kind: y, .. }) => x == y,
        _ => false,
    }
}

/// A composite's bound on its last component: [`component_key`], widened
/// for temporals the way a single-property range bound is.
fn component_bound(value: &PropertyValue, lower: bool) -> Option<PropertyIndexKey> {
    match value {
        PropertyValue::Int(_) | PropertyValue::Float(_) => component_key(value),
        _ if lower => PropertyIndexKey::range_lower(value),
        _ => PropertyIndexKey::range_upper(value),
    }
}

/// Index bounds for an inclusive `[lo, hi]` probe. `None` when a bound has
/// no index image (a duration, say): values of that type are not indexed,
/// so the caller must scan. Temporal bounds widen across UTC offsets, and a
/// one-sided temporal or numeric range is capped at the end of its kind.
fn probe_bounds(
    lo: Option<&PropertyValue>,
    hi: Option<&PropertyValue>,
) -> Option<(Bound<PropertyIndexKey>, Bound<PropertyIndexKey>)> {
    let lo_key = match lo {
        Some(v) => Some(PropertyIndexKey::range_lower(v)?),
        None => None,
    };
    let hi_key = match hi {
        Some(v) => Some(PropertyIndexKey::range_upper(v)?),
        None => None,
    };
    let (lo_key, hi_key) = match (lo_key, hi_key) {
        (Some(l), None) => {
            let h = l.kind_ceiling();
            (Some(l), h)
        }
        (None, Some(h)) => (h.kind_floor(), Some(h)),
        other => other,
    };
    Some((
        lo_key.map_or(Bound::Unbounded, Bound::Included),
        hi_key.map_or(Bound::Unbounded, Bound::Included),
    ))
}

fn insert_id(scope: &mut SortedScope, key: PropertyIndexKey, id: u64) {
    if let PropertyIndexKey::String(s) = &key {
        scope.strings.insert(s, id);
        return;
    }
    scope.by_value.upsert(
        key,
        || IdSet::new(id),
        |bucket| {
            bucket.insert(id);
        },
    );
}

/// The key a list element is indexed under: numbers at their `f64`
/// image, everything else as [`PropertyIndexKey::from_value`]. `None`
/// for null and for values without an image.
fn element_key(value: &PropertyValue) -> Option<PropertyIndexKey> {
    match value {
        PropertyValue::Null => None,
        PropertyValue::Int(i) => PropertyIndexKey::from_value(&PropertyValue::Float(*i as f64)),
        other => PropertyIndexKey::from_value(other),
    }
}

/// A scope's entries from every entity's `(value key, id)` and
/// `(list element key, id)`, in any order: sorted (on every core when the
/// build allows) and filed in key order, as many inserts would have.
pub(super) fn build_scope(
    mut values: Vec<(PropertyIndexKey, u64)>,
    mut strings: ScratchVec<(&str, u64)>,
    mut elements: Vec<(PropertyIndexKey, u64)>,
    unkeyed: BTreeSet<u64>,
) -> SortedScope {
    #[cfg(all(feature = "parallel", not(target_arch = "wasm32")))]
    {
        use rayon::slice::ParallelSliceMut;
        values.par_sort_unstable();
        strings.as_mut_slice().par_sort_unstable();
        elements.par_sort_unstable();
    }
    #[cfg(not(all(feature = "parallel", not(target_arch = "wasm32"))))]
    {
        values.sort_unstable();
        strings.as_mut_slice().sort_unstable();
        elements.sort_unstable();
    }
    elements.dedup();
    fn grouped(
        pairs: impl IntoIterator<Item = (PropertyIndexKey, u64)>,
    ) -> impl Iterator<Item = (PropertyIndexKey, IdSet)> {
        let mut pairs = pairs.into_iter().peekable();
        std::iter::from_fn(move || {
            let (key, first) = pairs.next()?;
            let mut ids = IdSet::new(first);
            while let Some((_, id)) = pairs.next_if(|(next, _)| *next == key) {
                ids.insert(id);
            }
            Some((key, ids))
        })
    }
    let element_postings = elements.len();
    let mut by_element = CowMap::default();
    for (key, ids) in grouped(elements) {
        by_element.insert(key, ids);
    }
    // Strings come borrowed from the records, so the build allocates
    // nothing per key: a key per string, freed once packed, left the
    // open's heap full of holes.
    debug_assert!(values
        .iter()
        .all(|(k, _)| !matches!(k, PropertyIndexKey::String(_))));
    let mut packed = StrPostingsBuilder::default();
    let mut pairs = strings.as_slice().iter().peekable();
    while let Some(&(key, first)) = pairs.next() {
        let mut ids = IdSet::new(first);
        while let Some(&(_, id)) = pairs.next_if(|(next, _)| *next == key) {
            ids.insert(id);
        }
        packed.push(key, ids);
    }
    SortedScope {
        by_value: CowOrdMap::from_sorted(grouped(values)),
        strings: packed.finish(),
        by_element,
        unkeyed,
        composite: None,
        element_postings,
        refcount: 0,
    }
}

/// The distinct element keys of `value` when it is a list.
pub(super) fn element_keys(value: &PropertyValue) -> Vec<PropertyIndexKey> {
    let PropertyValue::List(items) = value else {
        return Vec::new();
    };
    let mut keys: Vec<PropertyIndexKey> = items.iter().filter_map(element_key).collect();
    if keys.len() > 1 {
        keys.sort_unstable();
        keys.dedup();
    }
    keys
}

fn insert_elements(scope: &mut SortedScope, value: &PropertyValue, id: u64) {
    for key in element_keys(value) {
        let mut added = true;
        scope
            .by_element
            .upsert(key, || IdSet::new(id), |bucket| added = bucket.insert(id));
        if added {
            scope.element_postings += 1;
        }
    }
}

fn remove_elements(scope: &mut SortedScope, value: &PropertyValue, id: u64) {
    for key in element_keys(value) {
        let Some(bucket) = scope.by_element.get_mut(&key) else {
            continue;
        };
        let held = bucket.len();
        let emptied = bucket.remove(id);
        if emptied || bucket.len() < held {
            scope.element_postings = scope.element_postings.saturating_sub(1);
        }
        if emptied {
            scope.by_element.remove(&key);
        }
    }
}

fn extend_ids<'a>(out: &mut impl Extend<u64>, iter: impl Iterator<Item = (KeyRef<'a>, &'a IdSet)>) {
    for (_, ids) in iter {
        out.extend(ids.iter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PropertyValue;

    #[test]
    fn range_excludes_outside_bucket() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Person", "age");
        idx.insert("Person", "age", 1, &PropertyValue::Int(20));
        idx.insert("Person", "age", 2, &PropertyValue::Int(30));
        idx.insert("Person", "age", 3, &PropertyValue::Int(40));

        let lo = PropertyValue::Int(25);
        let hi = PropertyValue::Int(35);
        let got = idx
            .range_candidates("Person", "age", Some(&lo), Some(&hi))
            .unwrap();
        assert!(got.contains(&2));
        assert!(!got.contains(&1));
        assert!(!got.contains(&3));
    }

    #[test]
    fn range_includes_inclusive_boundaries() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Person", "age");
        idx.insert("Person", "age", 1, &PropertyValue::Int(20));
        idx.insert("Person", "age", 2, &PropertyValue::Int(30));

        let lo = PropertyValue::Int(20);
        let hi = PropertyValue::Int(30);
        let got = idx
            .range_candidates("Person", "age", Some(&lo), Some(&hi))
            .unwrap();
        assert!(got.contains(&1));
        assert!(got.contains(&2));
    }

    #[test]
    fn open_lower_bound_includes_all_below() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Person", "age");
        idx.insert("Person", "age", 1, &PropertyValue::Int(20));
        idx.insert("Person", "age", 2, &PropertyValue::Int(30));

        let hi = PropertyValue::Int(25);
        let got = idx
            .range_candidates("Person", "age", None, Some(&hi))
            .unwrap();
        assert!(got.contains(&1));
        assert!(!got.contains(&2));
    }

    #[test]
    fn float_ranges_keep_numeric_order_across_zero() {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("Reading", "temperature");
        idx.insert("Reading", "temperature", 1, &PropertyValue::Float(-10.0));
        idx.insert("Reading", "temperature", 2, &PropertyValue::Float(-1.5));
        idx.insert("Reading", "temperature", 3, &PropertyValue::Float(2.0));

        let lo = PropertyValue::Float(-2.0);
        let hi = PropertyValue::Float(1.0);
        let got = idx
            .range_candidates("Reading", "temperature", Some(&lo), Some(&hi))
            .unwrap();
        assert!(!got.contains(&1));
        assert!(got.contains(&2));
        assert!(!got.contains(&3));
    }

    fn mixed_numbers() -> SortedPropertyIndex {
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("P", "age");
        idx.insert("P", "age", 1, &PropertyValue::Int(30));
        idx.insert("P", "age", 2, &PropertyValue::Float(1.5));
        idx.insert("P", "age", 3, &PropertyValue::Int(10));
        idx.insert("P", "age", 4, &PropertyValue::Float(10.0));
        idx.insert("P", "age", 5, &PropertyValue::Null);
        idx.insert("P", "age", 6, &PropertyValue::Bool(true));
        idx.insert("P", "age", 7, &PropertyValue::String("x".into()));
        idx
    }

    fn ids(
        idx: &SortedPropertyIndex,
        lo: Option<PropertyValue>,
        hi: Option<PropertyValue>,
    ) -> Vec<u64> {
        idx.range_candidates("P", "age", lo.as_ref(), hi.as_ref())
            .unwrap()
            .into_iter()
            .collect()
    }

    /// Integers and floats share one numeric order: a float bound finds
    /// the integers past it and an integer bound the floats, and a
    /// one-sided range stays inside the numbers.
    #[test]
    fn mixed_int_and_float_ranges_compare_by_value() {
        use PropertyValue::{Float, Int};
        let idx = mixed_numbers();
        assert_eq!(ids(&idx, Some(Float(25.5)), None), vec![1]);
        assert_eq!(ids(&idx, None, Some(Int(9))), vec![2]);
        // Equal values of the other kind sit on the inclusive bound.
        assert_eq!(ids(&idx, Some(Float(10.0)), Some(Int(10))), vec![3, 4]);
        assert_eq!(ids(&idx, Some(Int(10)), Some(Float(10.0))), vec![3, 4]);
        assert_eq!(ids(&idx, None, Some(Int(10))), vec![2, 3, 4]);
        assert_eq!(ids(&idx, Some(Float(10.0)), None), vec![1, 3, 4]);
        assert_eq!(ids(&idx, Some(Int(2)), Some(Float(29.9))), vec![3, 4]);
    }

    #[test]
    fn ordered_walk_interleaves_ints_and_floats() {
        let idx = mixed_numbers();
        let lo = PropertyValue::Int(i64::MIN);
        let asc = idx
            .ordered_chunk("P", "age", Some(&lo), None, false, None, 100)
            .unwrap();
        assert_eq!(asc, vec![2, 3, 4, 1]);
        let desc = idx
            .ordered_chunk("P", "age", Some(&lo), None, true, None, 100)
            .unwrap();
        assert_eq!(desc, vec![1, 4, 3, 2]);
        // Resuming from a cursor on an integer continues with the float of
        // the same value.
        let after = PropertyValue::Int(10);
        let rest = idx
            .ordered_chunk("P", "age", Some(&lo), None, false, Some((&after, 3)), 100)
            .unwrap();
        assert_eq!(rest, vec![4, 1]);
    }

    /// Extremes: integers beyond 2^53 are not rounded through `f64`, and
    /// infinities bound every integer.
    #[test]
    fn numeric_order_is_exact_at_the_extremes() {
        use PropertyValue::{Float, Int};
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("P", "x");
        let big = 1i64 << 53;
        idx.insert("P", "x", 1, &Int(big + 1));
        idx.insert("P", "x", 2, &Float(big as f64));
        idx.insert("P", "x", 3, &Int(i64::MAX));
        idx.insert("P", "x", 4, &Float(9_223_372_036_854_775_808.0));
        idx.insert("P", "x", 5, &Int(i64::MIN));
        idx.insert("P", "x", 6, &Float(f64::NEG_INFINITY));
        idx.insert("P", "x", 7, &Float(-0.0));
        idx.insert("P", "x", 8, &Int(0));
        let lo = Float(f64::NEG_INFINITY);
        let order = idx
            .ordered_chunk("P", "x", Some(&lo), None, false, None, 100)
            .unwrap();
        assert_eq!(order, vec![6, 5, 8, 7, 2, 1, 3, 4]);
        let got: Vec<u64> = idx
            .range_candidates("P", "x", Some(&Float(big as f64)), Some(&Int(big)))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(got, vec![2]);
        let got: Vec<u64> = idx
            .range_candidates("P", "x", Some(&Int(0)), Some(&Float(0.0)))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(got, vec![7, 8]);
    }

    fn list(items: Vec<PropertyValue>) -> PropertyValue {
        PropertyValue::List(items)
    }

    fn elements(idx: &SortedPropertyIndex, element: PropertyValue) -> Option<Vec<u64>> {
        idx.element_candidates("E", "l", &element)
    }

    /// Each distinct element of a list lists the entity once; numbers
    /// meet across integer and float; nulls and scalars add nothing; an
    /// update moves the entity between elements.
    #[test]
    fn list_elements_are_indexed_and_maintained() {
        use PropertyValue::{Float, Int, Null, String as Str};
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("E", "l");
        let a = list(vec![Str("x".into()), Str("x".into()), Null, Int(1)]);
        idx.insert("E", "l", 1, &a);
        idx.insert("E", "l", 2, &list(vec![Float(1.0), Str("y".into())]));
        idx.insert("E", "l", 3, &Str("x".into()));
        idx.insert("E", "l", 4, &list(vec![Null]));

        assert_eq!(elements(&idx, Str("x".into())), Some(vec![1]));
        assert_eq!(elements(&idx, Int(1)), Some(vec![1, 2]));
        assert_eq!(elements(&idx, Float(1.0)), Some(vec![1, 2]));
        assert_eq!(elements(&idx, Null), Some(vec![]));
        assert_eq!(elements(&idx, Str("z".into())), Some(vec![]));
        assert_eq!(idx.element_candidates("E", "other", &Int(1)), None);
        let stats: Vec<_> = idx.element_stats().collect();
        assert_eq!(stats, vec![("E", "l", 3, 4)]);
        // The whole list is still there for `=` and ranges.
        assert_eq!(
            idx.range_candidates("E", "l", Some(&a), Some(&a))
                .map(|ids| ids.into_iter().collect::<Vec<_>>()),
            Some(vec![1])
        );

        let b = list(vec![Str("y".into())]);
        idx.update("E", "l", 1, Some(&a), Some(&b));
        assert_eq!(elements(&idx, Str("x".into())), Some(vec![]));
        assert_eq!(elements(&idx, Int(1)), Some(vec![2]));
        assert_eq!(elements(&idx, Str("y".into())), Some(vec![1, 2]));
        idx.update(
            "E",
            "l",
            2,
            Some(&list(vec![Float(1.0), Str("y".into())])),
            None,
        );
        idx.update("E", "l", 1, Some(&b), None);
        assert_eq!(elements(&idx, Str("y".into())), Some(vec![]));
        assert_eq!(idx.element_stats().count(), 0);
    }

    /// Integers past 2^53 share an element key with their `f64` image:
    /// a seek of one finds the others too, and the caller refilters.
    #[test]
    fn big_integers_share_their_float_image() {
        use PropertyValue::{Float, Int};
        let big = 1i64 << 53;
        let mut idx = SortedPropertyIndex::default();
        idx.add_scope("E", "l");
        idx.insert("E", "l", 1, &list(vec![Int(big)]));
        idx.insert("E", "l", 2, &list(vec![Int(big + 1)]));
        idx.insert("E", "l", 3, &list(vec![Float(big as f64)]));
        idx.insert("E", "l", 4, &list(vec![Float(-0.0)]));
        assert_eq!(elements(&idx, Int(big + 1)), Some(vec![1, 2, 3]));
        assert_eq!(elements(&idx, Int(0)), Some(vec![4]));
        assert_eq!(elements(&idx, Float(f64::NAN)), None);
    }

    #[test]
    fn numeric_bounds_report_no_other_temporal_kind() {
        let idx = mixed_numbers();
        assert_eq!(
            idx.other_temporal_kind_ids("P", "age", &PropertyValue::Int(3)),
            Some(Vec::new())
        );
    }
}
