//! Hash-bucket property indexes used by [`InMemoryGraph`] for
//! `find_*_by_property` lookups.
//!
//! Two registries live on the graph (one for nodes, one for
//! relationships); each registry owns a flat property→value→ids map
//! plus a parallel scope-keyed map for label/type filtered lookups.
//!
//! Activation is lazy: a property is *activated* the first time a
//! lookup asks for it. Subsequent lookups read from the index;
//! mutations to active keys are mirrored into it. Inactive keys still
//! work via the scan fallback in `super::scan_*`. A declared RANGE index
//! (or the backing index of a uniqueness / key constraint) activates its
//! keys when it is created.
//!
//! Restart reproduces that state, not the writer's: snapshot load and WAL
//! replay activate only the declared keys (by re-registering the catalog
//! or replaying the DDL events). Keys an equality lookup activated in the
//! writer are activated again by the next such lookup. `MemoryReport`
//! lists the active keys and marks the implicit ones.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::cow::{route, CowMap, FastHashMap};
use super::id_set::IdSet;
use crate::types::PropertyValue;
use crate::{LoraBinary, ZoneId};

/// Value → ids for one property. A copy-on-write hash trie (see
/// [`CowMap`]) so cloning the graph (the staged copy a write works on)
/// shares it and a write copies only the path to the value it changes.
pub(super) type PropertyValueBuckets = CowMap<PropertyIndexKey, IdSet>;
/// Property key → its buckets. Keys are behind `Arc` and cloning the
/// buckets is one refcount bump on their root, so copying this map (the
/// first write to it after a graph clone) is two refcount bumps per key;
/// the buckets of a key are copied only along the path a write takes.
/// Held directly, not behind another `Arc`: a lookup is one pointer hop
/// shorter.
pub(super) type PropertyIndex = FastHashMap<Arc<str>, PropertyValueBuckets>;
/// Scope (label / relationship type) → its per-key index, each behind
/// `Arc` so a write copies only the scopes it touches.
pub(super) type ScopedPropertyIndex = FastHashMap<Arc<str>, Arc<PropertyIndex>>;

/// Pair of [`PropertyIndexState`] registries: one for node properties,
/// one for relationship properties. Lives behind an `RwLock` on the
/// graph so cold lookups can take a read guard while activate-on-write
/// paths take a write guard briefly. Cloning it is six refcount bumps
/// (see [`PropertyIndexState`]).
#[derive(Default)]
pub(super) struct PropertyIndexRegistry {
    pub(super) node_properties: PropertyIndexState,
    pub(super) relationship_properties: PropertyIndexState,
}

impl std::fmt::Debug for PropertyIndexRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PropertyIndexRegistry")
            .field("node_properties", &self.node_properties)
            .field("relationship_properties", &self.relationship_properties)
            .finish()
    }
}

impl Clone for PropertyIndexRegistry {
    fn clone(&self) -> Self {
        Self {
            node_properties: self.node_properties.clone(),
            relationship_properties: self.relationship_properties.clone(),
        }
    }
}

/// Per-namespace property index — flat values plus a scope-keyed
/// (label / rel-type) variant for filtered lookups.
///
/// Every level sits behind `Arc` and is copied on write, so cloning the
/// state (part of every graph clone) is three refcount bumps, and the
/// first write after a clone copies only the path it touches: the key
/// and scope tables (a refcount bump or two per entry) and the path to
/// the touched value in the touched keys' buckets, never the buckets of
/// untouched keys or scopes.
#[derive(Debug, Default, Clone)]
pub(super) struct PropertyIndexState {
    pub(super) active_keys: Arc<BTreeSet<String>>,
    pub(super) values: Arc<PropertyIndex>,
    pub(super) scoped_values: Arc<ScopedPropertyIndex>,
}

impl PropertyIndexState {
    pub(super) fn is_active(&self, key: &str) -> bool {
        self.active_keys.contains(key)
    }

    pub(super) fn activate(&mut self, key: &str) -> bool {
        if self.active_keys.contains(key) {
            return false;
        }
        Arc::make_mut(&mut self.active_keys).insert(key.to_string())
    }

    /// The (copied-on-write) per-key index of `scope`, created if absent.
    fn scope_mut<'s>(
        scoped: &'s mut Arc<ScopedPropertyIndex>,
        scope: &str,
    ) -> &'s mut PropertyIndex {
        let scoped = Arc::make_mut(scoped);
        if !scoped.contains_key(scope) {
            scoped.insert(Arc::from(scope), Arc::default());
        }
        Arc::make_mut(scoped.get_mut(scope).expect("scope inserted above"))
    }

    fn insert_value(
        values: &mut PropertyIndex,
        entity_id: u64,
        key: &str,
        value: PropertyIndexKey,
    ) {
        let buckets = match values.get_mut(key) {
            Some(buckets) => buckets,
            None => values.entry(Arc::from(key)).or_default(),
        };
        buckets.upsert(
            value,
            || IdSet::new(entity_id),
            |ids| {
                ids.insert(entity_id);
            },
        );
    }

    /// Index `key` for every `(id, scopes, value)` that `entries` yields,
    /// as [`Self::insert_with_scopes`] for each would. `entries` is
    /// walked twice: first for the keys' routes, to shape each bucket map
    /// `key` does not have yet for exactly the keys it will hold (see
    /// [`CowMap::with_shape`]), then to insert. Activating an index over
    /// existing data this way allocates every leaf table once.
    pub(super) fn insert_bulk<'a, I, S>(&mut self, key: &str, entries: impl Fn() -> I)
    where
        I: Iterator<Item = (u64, S, &'a PropertyValue)>,
        S: IntoIterator<Item = &'a str>,
    {
        let mut flat = Vec::new();
        let mut scoped: FastHashMap<&'a str, Vec<u64>> = FastHashMap::default();
        for (_, scopes, value) in entries() {
            let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
                continue;
            };
            let route = route(&indexed_value);
            for scope in scopes {
                scoped.entry(scope).or_default().push(route);
            }
            flat.push(route);
        }
        let values = Arc::make_mut(&mut self.values);
        if !flat.is_empty() && !values.contains_key(key) {
            values.insert(Arc::from(key), CowMap::with_shape(flat));
        }
        for (scope, routes) in scoped {
            let values = Self::scope_mut(&mut self.scoped_values, scope);
            if !values.contains_key(key) {
                values.insert(Arc::from(key), CowMap::with_shape(routes));
            }
        }
        for (entity_id, scopes, value) in entries() {
            self.insert_with_scopes(entity_id, scopes, key, value);
        }
    }

    /// Whether `values` has a bucket for `value` under `key`: a remove
    /// that finds none changes nothing, so it must not copy shared tables.
    fn holds(values: &PropertyIndex, key: &str, value: &PropertyIndexKey) -> bool {
        values
            .get(key)
            .is_some_and(|buckets| buckets.contains_key(value))
    }

    fn remove_value(
        values: &mut PropertyIndex,
        entity_id: u64,
        key: &str,
        value: &PropertyIndexKey,
    ) {
        let mut remove_key = false;
        if let Some(buckets) = values.get_mut(key) {
            let emptied = buckets
                .get_mut(value)
                .is_some_and(|ids| ids.remove(entity_id));
            if emptied {
                buckets.remove(value);
            }
            remove_key = buckets.is_empty();
        }
        if remove_key {
            values.remove(key);
        }
    }

    pub(super) fn insert_scoped(
        &mut self,
        entity_id: u64,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        let scoped = Self::scope_mut(&mut self.scoped_values, scope);
        Self::insert_value(scoped, entity_id, key, indexed_value);
    }

    pub(super) fn insert_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        Self::insert_value(
            Arc::make_mut(&mut self.values),
            entity_id,
            key,
            indexed_value.clone(),
        );
        for scope in scopes {
            let scoped = Self::scope_mut(&mut self.scoped_values, scope);
            Self::insert_value(scoped, entity_id, key, indexed_value.clone());
        }
    }

    /// Remove `entity_id` from `scope`'s bucket for `key` = `value`,
    /// dropping the scope once it is empty.
    fn remove_from_scope(
        &mut self,
        entity_id: u64,
        scope: &str,
        key: &str,
        value: &PropertyIndexKey,
    ) {
        if !self
            .scoped_values
            .get(scope)
            .is_some_and(|values| Self::holds(values, key, value))
        {
            return;
        }
        let scoped = Arc::make_mut(&mut self.scoped_values);
        let mut remove_scope = false;
        if let Some(values) = scoped.get_mut(scope) {
            let values = Arc::make_mut(values);
            Self::remove_value(values, entity_id, key, value);
            remove_scope = values.is_empty();
        }
        if remove_scope {
            scoped.remove(scope);
        }
    }

    pub(super) fn remove_scoped(
        &mut self,
        entity_id: u64,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };
        self.remove_from_scope(entity_id, scope, key, &indexed_value);
    }

    pub(super) fn remove_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(indexed_value) = PropertyIndexKey::from_value(value) else {
            return;
        };

        if Self::holds(&self.values, key, &indexed_value) {
            Self::remove_value(
                Arc::make_mut(&mut self.values),
                entity_id,
                key,
                &indexed_value,
            );
        }
        for scope in scopes {
            self.remove_from_scope(entity_id, scope, key, &indexed_value);
        }
    }

    pub(super) fn ids_for(&self, key: &str, value: &PropertyValue) -> Option<&IdSet> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.values
            .get(key)
            .and_then(|values| values.get(&indexed_value))
    }

    pub(super) fn scoped_ids_for(
        &self,
        scope: &str,
        key: &str,
        value: &PropertyValue,
    ) -> Option<&IdSet> {
        let indexed_value = PropertyIndexKey::from_value(value)?;
        self.scoped_values
            .get(scope)
            .and_then(|values| values.get(key))
            .and_then(|values| values.get(&indexed_value))
    }
}

/// Hashable & sortable image of a [`PropertyValue`]. `None` from
/// [`PropertyIndexKey::from_value`] means "no stable image": durations
/// (no total order), spatial, and vector values fall through to the scan
/// fallback.
///
/// Temporal values (`Date`, `DateTime`, `LocalDateTime`, `Time`,
/// `LocalTime`) are keyed by kind, then by the instant they denote
/// (`order_nanos`, the order Cypher comparisons use), then by UTC offset.
/// The offset only separates values that denote the same instant in
/// different zones, so equality lookups stay exact while range probes
/// widen a bound across every offset (see [`PropertyIndexKey::range_lower`]).
///
/// `Ord` is hand-rolled (not derived) because [`crate::LoraBinary`]
/// doesn't expose `Ord` on its segmented byte representation. The
/// custom impl walks variants in their declaration order and falls
/// through to lexicographic ordering of inner data.
///
/// Floats use a sortable IEEE-754 bit projection, so range indexes
/// preserve numeric ordering across negative and positive values.
/// `f64::NAN` is rejected upstream by `from_value`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum PropertyIndexKey {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64),
    /// Shared so copying an index leaf or partition (a write's staged
    /// graph copy) bumps a refcount per key instead of reallocating it.
    String(std::sync::Arc<str>),
    Binary(LoraBinary),
    List(Vec<PropertyIndexKey>),
    Map(BTreeMap<String, PropertyIndexKey>),
    Temporal {
        kind: TemporalKind,
        nanos: i128,
        offset: i32,
        /// A DATETIME's named zone: the same instant and offset in another
        /// zone is another value. Only breaks ties; range bounds use
        /// extreme offsets, so it never decides a range.
        zone: Option<ZoneId>,
    },
}

/// Temporal key families. Values of different kinds never compare in
/// Cypher, so each kind occupies its own contiguous run of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum TemporalKind {
    Date,
    LocalTime,
    Time,
    LocalDateTime,
    DateTime,
}

impl PartialOrd for PropertyIndexKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PropertyIndexKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let tag = |k: &PropertyIndexKey| match k {
            PropertyIndexKey::Null => 0,
            PropertyIndexKey::Bool(_) => 1,
            PropertyIndexKey::Int(_) => 2,
            PropertyIndexKey::Float(_) => 3,
            PropertyIndexKey::String(_) => 4,
            PropertyIndexKey::Binary(_) => 5,
            PropertyIndexKey::List(_) => 6,
            PropertyIndexKey::Map(_) => 7,
            PropertyIndexKey::Temporal { .. } => 8,
        };
        match tag(self).cmp(&tag(other)) {
            Ordering::Equal => match (self, other) {
                (PropertyIndexKey::Null, PropertyIndexKey::Null) => Ordering::Equal,
                (PropertyIndexKey::Bool(a), PropertyIndexKey::Bool(b)) => a.cmp(b),
                (PropertyIndexKey::Int(a), PropertyIndexKey::Int(b)) => a.cmp(b),
                (PropertyIndexKey::Float(a), PropertyIndexKey::Float(b)) => a.cmp(b),
                (PropertyIndexKey::String(a), PropertyIndexKey::String(b)) => a.cmp(b),
                (PropertyIndexKey::Binary(a), PropertyIndexKey::Binary(b)) => {
                    // Lexicographic byte comparison across segments.
                    // Allocates only when LoraBinary doesn't expose a
                    // contiguous view; for sorted-index inserts this
                    // happens once per insertion and is bounded by the
                    // value size.
                    let aa: Vec<u8> = a.segments().iter().flatten().copied().collect();
                    let bb: Vec<u8> = b.segments().iter().flatten().copied().collect();
                    aa.cmp(&bb)
                }
                (PropertyIndexKey::List(a), PropertyIndexKey::List(b)) => a.cmp(b),
                (PropertyIndexKey::Map(a), PropertyIndexKey::Map(b)) => a.cmp(b),
                (
                    PropertyIndexKey::Temporal {
                        kind: ak,
                        nanos: an,
                        offset: ao,
                        zone: az,
                    },
                    PropertyIndexKey::Temporal {
                        kind: bk,
                        nanos: bn,
                        offset: bo,
                        zone: bz,
                    },
                ) => ak
                    .cmp(bk)
                    .then(an.cmp(bn))
                    .then(ao.cmp(bo))
                    .then(az.cmp(bz)),
                _ => Ordering::Equal, // unreachable given equal tags
            },
            ord => ord,
        }
    }
}

impl PropertyIndexKey {
    pub(super) fn from_value(value: &PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::Null => Some(Self::Null),
            PropertyValue::Bool(v) => Some(Self::Bool(*v)),
            PropertyValue::Int(v) => Some(Self::Int(*v)),
            PropertyValue::Float(v) => {
                if v.is_nan() {
                    None
                } else {
                    Some(Self::Float(sortable_f64_bits(*v)))
                }
            }
            PropertyValue::String(v) => Some(Self::String(std::sync::Arc::from(v.as_str()))),
            PropertyValue::Binary(v) => Some(Self::Binary(v.clone())),
            PropertyValue::List(values) => values
                .iter()
                .map(Self::from_value)
                .collect::<Option<Vec<_>>>()
                .map(Self::List),
            PropertyValue::Map(values) => values
                .iter()
                .map(|(k, v)| Self::from_value(v).map(|indexed| (k.clone(), indexed)))
                .collect::<Option<BTreeMap<_, _>>>()
                .map(Self::Map),
            PropertyValue::Date(v) => Some(Self::temporal(TemporalKind::Date, v.order_nanos(), 0)),
            PropertyValue::LocalTime(v) => {
                Some(Self::temporal(TemporalKind::LocalTime, v.order_nanos(), 0))
            }
            PropertyValue::Time(v) => Some(Self::temporal(
                TemporalKind::Time,
                v.order_nanos(),
                v.offset_seconds,
            )),
            PropertyValue::LocalDateTime(v) => Some(Self::temporal(
                TemporalKind::LocalDateTime,
                v.order_nanos(),
                0,
            )),
            PropertyValue::DateTime(v) => Some(Self::Temporal {
                kind: TemporalKind::DateTime,
                nanos: v.order_nanos(),
                offset: v.offset_seconds,
                zone: v.zone,
            }),
            // Durations have no total order in Cypher (a month has no fixed
            // length), and spatial and vector values have no stable ordered
            // image. Those use the scan fallback, and a range bound of one of
            // these types makes the range probe fall back to a scan too.
            PropertyValue::Duration(_) | PropertyValue::Point(_) | PropertyValue::Vector(_) => None,
        }
    }

    fn temporal(kind: TemporalKind, nanos: i128, offset: i32) -> Self {
        Self::Temporal {
            kind,
            nanos,
            offset,
            zone: None,
        }
    }

    /// The smallest key a value `>= value` can have. Equal to
    /// [`Self::from_value`] except for temporals, which widen to the lowest
    /// offset so a bound matches every value denoting the same instant.
    pub(super) fn range_lower(value: &PropertyValue) -> Option<Self> {
        match Self::from_value(value)? {
            Self::Temporal { kind, nanos, .. } => Some(Self::temporal(kind, nanos, i32::MIN)),
            key => Some(key),
        }
    }

    /// The largest key a value `<= value` can have; see [`Self::range_lower`].
    pub(super) fn range_upper(value: &PropertyValue) -> Option<Self> {
        match Self::from_value(value)? {
            Self::Temporal { kind, nanos, .. } => Some(Self::temporal(kind, nanos, i32::MAX)),
            key => Some(key),
        }
    }

    /// For a one-sided temporal range, the other end of that temporal kind:
    /// values of other types never satisfy a temporal comparison, so the
    /// probe (and an ordered walk) stays inside the kind.
    pub(super) fn kind_floor(&self) -> Option<Self> {
        match self {
            Self::Temporal { kind, .. } => Some(Self::temporal(*kind, i128::MIN, i32::MIN)),
            _ => None,
        }
    }

    /// Upper counterpart of [`Self::kind_floor`].
    pub(super) fn kind_ceiling(&self) -> Option<Self> {
        match self {
            Self::Temporal { kind, .. } => Some(Self::temporal(*kind, i128::MAX, i32::MAX)),
            _ => None,
        }
    }

    /// The first and last possible temporal keys: the temporal kinds sit
    /// next to each other, so these bound all of them at once.
    pub(super) fn all_temporals() -> (Self, Self) {
        (
            Self::temporal(TemporalKind::Date, i128::MIN, i32::MIN),
            Self::temporal(TemporalKind::DateTime, i128::MAX, i32::MAX),
        )
    }
}

fn sortable_f64_bits(value: f64) -> u64 {
    let bits = if value == 0.0 {
        0.0f64.to_bits()
    } else {
        value.to_bits()
    };
    if bits & (1 << 63) == 0 {
        bits | (1 << 63)
    } else {
        !bits
    }
}
