//! Hash-bucket property indexes used by [`InMemoryGraph`] for
//! `find_*_by_property` lookups.
//!
//! Two registries live on the graph (one for nodes, one for
//! relationships); each registry owns one property→value→ids map per
//! scope (label or relationship type), for lookups that name one. A
//! node without labels is listed under [`UNLABELLED`]. A key looked up
//! without naming a scope also gets a map across all scopes, built by
//! the first such lookup.
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

use std::collections::BTreeSet;
use std::sync::Arc;

use super::cow::{route, CowMap, FastHashMap};
use super::id_set::IdSet;
use crate::types::PropertyValue;
use crate::{LoraBinary, ZoneId};

/// Value → ids for one property, in two copy-on-write hash tries (see
/// [`CowMap`]) so cloning the graph (the staged copy a write works on)
/// shares them and a write copies only the path to the value it changes.
///
/// A value with a heap image (a string, byte string, list or map) that
/// one entity holds is kept as a 64-bit digest of the value and the
/// entity's id: 16 bytes, where the value itself cost a 24-byte key plus
/// its copy on the heap (a 58-byte key string took 104 bytes, beside the
/// 58 the record already holds). A lookup confirms the entity against its
/// record. That is the common case for the keys a uniqueness constraint
/// or a lookup by key indexes: one entity per value.
///
/// `exact` keeps the value itself: for values two or more entities hold
/// (a lookup then reads no record), for values with no heap image (null,
/// booleans, numbers, temporals: no smaller than a digest), and for the
/// rare value whose digest another value's single entry already uses.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct PropertyValueBuckets {
    exact: CowMap<PropertyIndexKey, IdSet>,
    single: CowMap<u64, u64>,
}

/// Ids an index lookup found: a bucket, or the one entity a digest named
/// and its record confirmed.
#[derive(Clone, Copy)]
pub(super) enum Hits<'a> {
    Set(&'a IdSet),
    One(u64),
}

impl<'a> Hits<'a> {
    pub(super) fn is_empty(self) -> bool {
        match self {
            Hits::Set(ids) => ids.is_empty(),
            Hits::One(_) => false,
        }
    }

    pub(super) fn to_vec(self) -> Vec<u64> {
        match self {
            Hits::Set(ids) => ids.to_vec(),
            Hits::One(id) => vec![id],
        }
    }

    /// Ids in ascending order.
    pub(super) fn iter(self) -> impl Iterator<Item = u64> + 'a {
        let (set, one) = match self {
            Hits::Set(ids) => (Some(ids.iter()), None),
            Hits::One(id) => (None, Some(id)),
        };
        set.into_iter().flatten().chain(one)
    }
}

/// The digest a value with a heap image is kept under in
/// [`PropertyValueBuckets::single`]. Seeded per process like every index
/// hash, so values chosen to collide cannot be precomputed; a collision
/// costs memory (both values keep an exact entry), never a wrong answer.
fn digest(value: &PropertyIndexKey) -> u64 {
    use std::hash::BuildHasher;
    match value {
        PropertyIndexKey::String(s) => digest_str(s),
        other => std::hash::BuildHasherDefault::<super::cow::KeyHasher>::default().hash_one(other),
    }
}

/// [`digest`] of a string key, from the string.
fn digest_str(value: &str) -> u64 {
    use std::hash::BuildHasher;
    std::hash::BuildHasherDefault::<super::cow::KeyHasher>::default().hash_one(value)
}

/// A value looked up in a hash property index: a string as given (most
/// lookups by key are, and a digest entry answers them without building a
/// key), or any other value's key.
pub(super) enum Probe<'a> {
    Str(&'a str),
    Key(PropertyIndexKey),
}

impl<'a> Probe<'a> {
    /// `None` when `value` has no index image (see
    /// [`PropertyIndexKey::from_value`]).
    pub(super) fn of(value: &'a PropertyValue) -> Option<Self> {
        match value {
            PropertyValue::String(s) => Some(Probe::Str(s)),
            other => PropertyIndexKey::from_value(other).map(Probe::Key),
        }
    }

    /// As [`Self::of`], for a value read through a view.
    pub(super) fn of_ref(value: crate::ValueRef<'a>) -> Option<Self> {
        match value {
            crate::ValueRef::String(s) => Some(Probe::Str(s)),
            other => PropertyIndexKey::from_value_ref(other).map(Probe::Key),
        }
    }

    fn key(&self) -> std::borrow::Cow<'_, PropertyIndexKey> {
        match self {
            Probe::Str(s) => std::borrow::Cow::Owned(PropertyIndexKey::String(Arc::from(*s))),
            Probe::Key(key) => std::borrow::Cow::Borrowed(key),
        }
    }

    /// The digest entry the value would have; `None` when it has no heap
    /// image and is always kept exact.
    fn digest(&self) -> Option<u64> {
        match self {
            Probe::Str(s) => Some(digest_str(s)),
            Probe::Key(key) => key.has_heap_image().then(|| digest(key)),
        }
    }

    /// Whether a stored value, read through a view, is this one.
    fn matches(&self, value: crate::ValueRef<'_>) -> bool {
        match self {
            Probe::Str(s) => matches!(value, crate::ValueRef::String(v) if v == *s),
            Probe::Key(key) => key.matches(value),
        }
    }
}

impl PropertyValueBuckets {
    pub(super) fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.single.is_empty()
    }

    /// Empty buckets shaped (see [`CowMap::with_shape`]) for the values
    /// whose keys are given: `exact` routes for values kept as themselves,
    /// `single` routes (of the digests) for values with a heap image.
    fn with_shape(exact: Vec<u64>, single: Vec<u64>) -> Self {
        Self {
            exact: CowMap::with_shape(exact),
            single: CowMap::with_shape(single),
        }
    }

    /// The trie route `value` takes in its tier, for [`Self::with_shape`]:
    /// `(true, route)` for the digest tier.
    fn shape_route(value: &Probe<'_>) -> (bool, u64) {
        match value.digest() {
            Some(d) => (true, route(&d)),
            None => (false, route(&*value.key())),
        }
    }

    /// Add `id` under `value`. `holds` tells whether another entity holds
    /// `value` when `value`'s digest already names one.
    pub(super) fn insert(&mut self, id: u64, value: &Probe<'_>, holds: &dyn Fn(u64) -> bool) {
        let Some(d) = value.digest() else {
            self.exact.upsert(
                value.key().into_owned(),
                || IdSet::new(id),
                |ids| {
                    ids.insert(id);
                },
            );
            return;
        };
        if !self.exact.is_empty() {
            if let Some(ids) = self.exact.get_mut(&*value.key()) {
                ids.insert(id);
                return;
            }
        }
        match self.single.get(&d).copied() {
            None => {
                self.single.insert(d, id);
            }
            Some(other) if other == id => {}
            Some(other) => {
                let mut ids = IdSet::new(id);
                if holds(other) {
                    // A second holder: the value moves to `exact`.
                    self.single.remove(&d);
                    ids.insert(other);
                }
                self.exact.insert(value.key().into_owned(), ids);
            }
        }
    }

    /// Remove `id` from under `value`.
    pub(super) fn remove(&mut self, id: u64, value: &Probe<'_>) {
        let digest = value.digest();
        if !self.exact.is_empty() {
            let key = value.key();
            if let Some(ids) = self.exact.get_mut(&*key) {
                if ids.remove(id) {
                    self.exact.remove(&*key);
                } else if let (1, Some(d)) = (ids.len(), digest) {
                    // Back to one holder: its digest entry, unless another
                    // value's single entry has that digest.
                    let last = ids.iter().next().expect("one id left");
                    if !self.single.contains_key(&d) {
                        self.exact.remove(&*key);
                        self.single.insert(d, last);
                    }
                }
            }
        }
        if let Some(d) = digest {
            if self.single.get(&d) == Some(&id) {
                self.single.remove(&d);
            }
        }
    }

    /// Whether `value` has a bucket or `id` is its digest entry: a remove
    /// that finds neither changes nothing, so it must not copy shared
    /// tables.
    fn lists(&self, id: u64, value: &Probe<'_>) -> bool {
        (!self.exact.is_empty() && self.exact.contains_key(&*value.key()))
            || value
                .digest()
                .is_some_and(|d| self.single.get(&d) == Some(&id))
    }

    /// The ids listed under `value`. An entity found by digest is checked
    /// with `holds` (does `id` hold `value`?) and dropped when it does
    /// not.
    pub(super) fn get(&self, value: &Probe<'_>, holds: impl Fn(u64) -> bool) -> Option<Hits<'_>> {
        if !self.exact.is_empty() {
            if let Some(ids) = self.exact.get(&*value.key()) {
                return Some(Hits::Set(ids));
            }
        }
        let id = *self.single.get(&value.digest()?)?;
        holds(id).then_some(Hits::One(id))
    }

    /// Every entity listed, with the value kept for it when the value is
    /// kept (`None` for a digest entry).
    pub(super) fn entries(&self) -> impl Iterator<Item = (Option<&PropertyIndexKey>, u64)> + '_ {
        self.exact
            .iter()
            .flat_map(|(value, ids)| ids.iter().map(move |id| (Some(value), id)))
            .chain(self.single.iter().map(|(_, id)| (None, *id)))
    }

    /// The exact tier, for the memory report.
    pub(super) fn exact(&self) -> &CowMap<PropertyIndexKey, IdSet> {
        &self.exact
    }

    /// The digest tier, for the memory report.
    pub(super) fn single(&self) -> &CowMap<u64, u64> {
        &self.single
    }
}
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
    pub(super) scoped_values: Arc<ScopedPropertyIndex>,
    /// Active keys that also keep a map across every scope, in
    /// `any_scope`. Built by the first lookup of the key that names no
    /// scope, and not restored by a restart: most keys are only ever
    /// looked up under a label, and the second map would double their
    /// index.
    pub(super) any_scope_keys: Arc<BTreeSet<String>>,
    pub(super) any_scope: Arc<PropertyIndex>,
}

/// The scope a node without labels is listed under. No node carries an
/// empty label (labels are trimmed and empty ones dropped), and a lookup
/// that asks for one is answered with nothing instead of this scope
/// (see [`PropertyIndexState::scoped_ids_for`]). Every entity is therefore
/// listed under at least one scope, which is what the across-scopes map
/// is built from.
pub(super) const UNLABELLED: &str = "";

/// `scopes`, or [`UNLABELLED`] when there are none.
fn or_unlabelled<'a>(scopes: impl IntoIterator<Item = &'a str>) -> impl Iterator<Item = &'a str> {
    let mut scopes = scopes.into_iter().peekable();
    let unlabelled = scopes.peek().is_none().then_some(UNLABELLED);
    scopes.chain(unlabelled)
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

    /// Stop maintaining `key` and release its buckets in every scope. The
    /// next equality lookup of the key activates it again from the data.
    pub(super) fn deactivate(&mut self, key: &str) -> bool {
        if !self.active_keys.contains(key) {
            return false;
        }
        Arc::make_mut(&mut self.active_keys).remove(key);
        if self.any_scope_keys.contains(key) {
            Arc::make_mut(&mut self.any_scope_keys).remove(key);
            Arc::make_mut(&mut self.any_scope).remove(key);
        }
        let holds_key: Vec<Arc<str>> = self
            .scoped_values
            .iter()
            .filter(|(_, values)| values.contains_key(key))
            .map(|(scope, _)| scope.clone())
            .collect();
        for scope in holds_key {
            let scoped = Arc::make_mut(&mut self.scoped_values);
            let values = Arc::make_mut(scoped.get_mut(&*scope).expect("scope listed above"));
            values.remove(key);
            if values.is_empty() {
                scoped.remove(&*scope);
            }
        }
        true
    }

    pub(super) fn any_scope_is_active(&self, key: &str) -> bool {
        self.any_scope_keys.contains(key)
    }

    /// Build the across-scopes map for `key` from the per-scope ones, and
    /// keep it current from now on.
    pub(super) fn activate_any_scope(&mut self, key: &str, records: &dyn EntityValues) {
        if self.any_scope_keys.contains(key) {
            return;
        }
        let mut merged = PropertyValueBuckets::default();
        for values in self.scoped_values.values() {
            let Some(buckets) = values.get(key) else {
                continue;
            };
            for (value, id) in buckets.entries() {
                // A digest entry's value is read back from its record.
                let value = match value {
                    Some(value) => value.clone(),
                    None => match records.value(id, key) {
                        Some(value) => value,
                        None => continue,
                    },
                };
                let probe = Probe::Key(value);
                merged.insert(id, &probe, &|other| {
                    records.get(other, key).is_some_and(|v| probe.matches(v))
                });
            }
        }
        if !merged.is_empty() {
            Arc::make_mut(&mut self.any_scope).insert(Arc::from(key), merged);
        }
        Arc::make_mut(&mut self.any_scope_keys).insert(key.to_string());
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
        value: &Probe<'_>,
        records: &dyn EntityValues,
    ) {
        let buckets = match values.get_mut(key) {
            Some(buckets) => buckets,
            None => values.entry(Arc::from(key)).or_default(),
        };
        buckets.insert(entity_id, value, &|other| {
            records.get(other, key).is_some_and(|v| value.matches(v))
        });
    }

    /// Index `key` for every `(id, scopes, value)` that `entries` yields,
    /// as [`Self::insert_with_scopes`] for each would. `entries` is
    /// walked twice: first for the values' routes, to shape each bucket
    /// map `key` does not have yet for exactly the values it will hold
    /// (see [`CowMap::with_shape`]), then to insert. Activating an index
    /// over existing data this way allocates every leaf table once.
    pub(super) fn insert_bulk<'a, I, S>(
        &mut self,
        key: &str,
        entries: impl Fn() -> I,
        records: &dyn EntityValues,
    ) where
        I: Iterator<Item = (u64, S, crate::ValueRef<'a>)>,
        S: IntoIterator<Item = &'a str>,
    {
        type Routes = (Vec<u64>, Vec<u64>);
        let mut scoped: FastHashMap<&'a str, Routes> = FastHashMap::default();
        for (_, scopes, value) in entries() {
            let Some(probe) = Probe::of_ref(value) else {
                continue;
            };
            let (single, route) = PropertyValueBuckets::shape_route(&probe);
            for scope in or_unlabelled(scopes) {
                let routes = scoped.entry(scope).or_default();
                if single {
                    routes.1.push(route);
                } else {
                    routes.0.push(route);
                }
            }
        }
        for (scope, (exact, single)) in scoped {
            let values = Self::scope_mut(&mut self.scoped_values, scope);
            if !values.contains_key(key) {
                values.insert(
                    Arc::from(key),
                    PropertyValueBuckets::with_shape(exact, single),
                );
            }
        }
        for (entity_id, scopes, value) in entries() {
            let Some(probe) = Probe::of_ref(value) else {
                continue;
            };
            self.insert_probe_with_scopes(entity_id, scopes, key, &probe, records);
        }
    }

    /// Whether `values` lists anything `entity_id` could be listed under
    /// for `key` = `value`: a remove that finds nothing changes nothing,
    /// so it must not copy shared tables.
    fn lists(values: &PropertyIndex, entity_id: u64, key: &str, value: &Probe<'_>) -> bool {
        values
            .get(key)
            .is_some_and(|buckets| buckets.lists(entity_id, value))
    }

    fn remove_value(values: &mut PropertyIndex, entity_id: u64, key: &str, value: &Probe<'_>) {
        let mut remove_key = false;
        if let Some(buckets) = values.get_mut(key) {
            buckets.remove(entity_id, value);
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
        records: &dyn EntityValues,
    ) {
        let Some(probe) = Probe::of(value) else {
            return;
        };

        let scoped = Self::scope_mut(&mut self.scoped_values, scope);
        Self::insert_value(scoped, entity_id, key, &probe, records);
    }

    pub(super) fn insert_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
        records: &dyn EntityValues,
    ) {
        let Some(probe) = Probe::of(value) else {
            return;
        };
        self.insert_probe_with_scopes(entity_id, scopes, key, &probe, records);
    }

    fn insert_probe_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        probe: &Probe<'_>,
        records: &dyn EntityValues,
    ) {
        if self.any_scope_keys.contains(key) {
            Self::insert_value(
                Arc::make_mut(&mut self.any_scope),
                entity_id,
                key,
                probe,
                records,
            );
        }
        for scope in or_unlabelled(scopes) {
            let scoped = Self::scope_mut(&mut self.scoped_values, scope);
            Self::insert_value(scoped, entity_id, key, probe, records);
        }
    }

    /// Remove `entity_id` from `scope`'s bucket for `key` = `value`,
    /// dropping the scope once it is empty.
    fn remove_from_scope(&mut self, entity_id: u64, scope: &str, key: &str, value: &Probe<'_>) {
        if !self
            .scoped_values
            .get(scope)
            .is_some_and(|values| Self::lists(values, entity_id, key, value))
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
        let Some(probe) = Probe::of(value) else {
            return;
        };
        self.remove_from_scope(entity_id, scope, key, &probe);
    }

    pub(super) fn remove_with_scopes<'a>(
        &mut self,
        entity_id: u64,
        scopes: impl IntoIterator<Item = &'a str>,
        key: &str,
        value: &PropertyValue,
    ) {
        let Some(probe) = Probe::of(value) else {
            return;
        };

        if Self::lists(&self.any_scope, entity_id, key, &probe) {
            Self::remove_value(Arc::make_mut(&mut self.any_scope), entity_id, key, &probe);
        }
        for scope in or_unlabelled(scopes) {
            self.remove_from_scope(entity_id, scope, key, &probe);
        }
    }

    /// Ids of every entity whose `key` equals `value`, whatever its scope,
    /// in ascending order. Reads the across-scopes map, which the caller
    /// must have built (see [`Self::activate_any_scope`]).
    pub(super) fn ids_for(
        &self,
        key: &str,
        value: &PropertyValue,
        records: &dyn EntityValues,
    ) -> Option<Hits<'_>> {
        debug_assert!(
            self.any_scope_keys.contains(key),
            "lookup of `{key}` across scopes before its map was built"
        );
        let probe = Probe::of(value)?;
        self.any_scope.get(key)?.get(&probe, |id| {
            records.get(id, key).is_some_and(|v| probe.matches(v))
        })
    }

    pub(super) fn scoped_ids_for(
        &self,
        scope: &str,
        key: &str,
        value: &PropertyValue,
        records: &dyn EntityValues,
    ) -> Option<Hits<'_>> {
        // No entity has an empty label; the scope by that name holds the
        // nodes that have none (see `UNLABELLED`).
        if scope == UNLABELLED {
            return None;
        }
        let probe = Probe::of(value)?;
        self.scoped_values.get(scope)?.get(key)?.get(&probe, |id| {
            records.get(id, key).is_some_and(|v| probe.matches(v))
        })
    }
}

/// Reads an entity's current value of a property from its record, for the
/// digest entries of [`PropertyValueBuckets`], which keep no value.
pub(super) trait EntityValues {
    /// Entity `id`'s value under `key`, if it has one.
    fn get(&self, id: u64, key: &str) -> Option<crate::ValueRef<'_>>;

    /// Entity `id`'s value under `key`, as an index key.
    fn value(&self, id: u64, key: &str) -> Option<PropertyIndexKey> {
        PropertyIndexKey::from_value_ref(self.get(id, key)?)
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
/// `f64::NAN` is rejected upstream by `from_value`. Integers and floats
/// share one run ordered by exact numeric value (as Cypher compares them),
/// an integer before a float of the same value so `Ord` agrees with the
/// derived `Eq`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum PropertyIndexKey {
    Null,
    Bool(bool),
    Int(i64),
    Float(u64),
    /// Shared so copying an index leaf or partition (a write's staged
    /// graph copy) bumps a refcount per key instead of reallocating it.
    String(std::sync::Arc<str>),
    /// Boxed: a `LoraBinary` is four words, and the key should stay at
    /// three with its tag.
    Binary(Box<LoraBinary>),
    /// Boxed slices, not a `Vec` and a `BTreeMap`: those are three words
    /// and made every key four, in every entry of every index.
    List(Box<[PropertyIndexKey]>),
    /// Entries in key order, so ordering, equality and hashing match
    /// the `BTreeMap` they were collected from.
    Map(Box<[(String, PropertyIndexKey)]>),
    Temporal {
        kind: TemporalKind,
        nanos: Nanos,
        offset: i32,
        /// A DATETIME's named zone: the same instant and offset in another
        /// zone is another value. Only breaks ties; range bounds use
        /// extreme offsets, so it never decides a range.
        zone: Option<ZoneId>,
    },
}

/// A temporal's position on its kind's timeline, in nanoseconds: an `i128`
/// kept as two words. An `i128` field would align the whole key to 16
/// bytes and pad it from 24 to 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Nanos {
    high: i64,
    low: u64,
}

impl From<i128> for Nanos {
    fn from(nanos: i128) -> Self {
        Self {
            high: (nanos >> 64) as i64,
            low: nanos as u64,
        }
    }
}

// On 64-bit targets; a 32-bit one (wasm32) has smaller pointers.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<PropertyIndexKey>() == 24);

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
            PropertyIndexKey::Int(_) | PropertyIndexKey::Float(_) => 2,
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
                (PropertyIndexKey::Int(a), PropertyIndexKey::Float(b)) => {
                    int_float_cmp(*a, float_from_sortable_bits(*b)).then(Ordering::Less)
                }
                (PropertyIndexKey::Float(a), PropertyIndexKey::Int(b)) => {
                    int_float_cmp(*b, float_from_sortable_bits(*a))
                        .reverse()
                        .then(Ordering::Greater)
                }
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
    /// Whether the key holds a copy of its value on the heap (a string,
    /// byte string, list or map), which a digest is smaller than.
    #[inline]
    pub(super) fn has_heap_image(&self) -> bool {
        matches!(
            self,
            Self::String(_) | Self::Binary(_) | Self::List(_) | Self::Map(_)
        )
    }

    /// Whether `value`, read through a view, has this key: as
    /// `from_value_ref(value) == Some(self)`, without building the key for
    /// a string.
    #[inline]
    pub(super) fn matches(&self, value: crate::ValueRef<'_>) -> bool {
        match (self, value) {
            (Self::String(key), crate::ValueRef::String(value)) => **key == *value,
            (Self::String(_), _) => false,
            (key, value) => Self::from_value_ref(value).as_ref() == Some(key),
        }
    }

    /// As [`Self::from_value`], for a value read through a view.
    pub(super) fn from_value_ref(value: crate::ValueRef<'_>) -> Option<Self> {
        use crate::ValueRef;
        match value {
            ValueRef::Null => Some(Self::Null),
            ValueRef::Bool(v) => Some(Self::Bool(v)),
            ValueRef::Int(v) => Some(Self::Int(v)),
            ValueRef::Float(v) => Self::from_value(&PropertyValue::Float(v)),
            ValueRef::String(v) => Some(Self::String(std::sync::Arc::from(v))),
            ValueRef::Other(v) => Self::from_value(&v.get()),
        }
    }

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
            PropertyValue::Binary(v) => Some(Self::Binary(Box::new(v.clone()))),
            PropertyValue::List(values) => values
                .iter()
                .map(Self::from_value)
                .collect::<Option<Box<[_]>>>()
                .map(Self::List),
            // A `BTreeMap` iterates in key order.
            PropertyValue::Map(values) => values
                .iter()
                .map(|(k, v)| Self::from_value(v).map(|indexed| (k.clone(), indexed)))
                .collect::<Option<Box<[_]>>>()
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
                nanos: v.order_nanos().into(),
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

    fn temporal(kind: TemporalKind, nanos: impl Into<Nanos>, offset: i32) -> Self {
        Self::Temporal {
            kind,
            nanos: nanos.into(),
            offset,
            zone: None,
        }
    }

    /// The smallest key a value `>= value` can have. Equal to
    /// [`Self::from_value`] except for temporals, which widen to the lowest
    /// offset so a bound matches every value denoting the same instant.
    /// A whole float widens to the integer of the same value, which sorts
    /// just before it.
    pub(super) fn range_lower(value: &PropertyValue) -> Option<Self> {
        match Self::from_value(value)? {
            Self::Temporal { kind, nanos, .. } => Some(Self::temporal(kind, nanos, i32::MIN)),
            Self::Float(bits) => {
                let f = float_from_sortable_bits(bits);
                let whole = f as i64;
                Some(if int_float_cmp(whole, f) == std::cmp::Ordering::Equal {
                    Self::Int(whole)
                } else {
                    Self::Float(bits)
                })
            }
            key => Some(key),
        }
    }

    /// The largest key a value `<= value` can have; see [`Self::range_lower`].
    /// An integer widens to the float of the same value, when one exists,
    /// which sorts just after it.
    pub(super) fn range_upper(value: &PropertyValue) -> Option<Self> {
        match Self::from_value(value)? {
            Self::Temporal { kind, nanos, .. } => Some(Self::temporal(kind, nanos, i32::MAX)),
            Self::Int(i) => {
                let f = i as f64;
                Some(if int_float_cmp(i, f) == std::cmp::Ordering::Equal {
                    Self::Float(sortable_f64_bits(f))
                } else {
                    Self::Int(i)
                })
            }
            key => Some(key),
        }
    }

    /// For a one-sided temporal or numeric range, the other end of that
    /// kind: values of other types never satisfy the comparison, so the
    /// probe (and an ordered walk) stays inside the kind. Integers and
    /// floats are one kind, bounded by the infinities.
    pub(super) fn kind_floor(&self) -> Option<Self> {
        match self {
            Self::Temporal { kind, .. } => Some(Self::temporal(*kind, i128::MIN, i32::MIN)),
            Self::Int(_) | Self::Float(_) => {
                Some(Self::Float(sortable_f64_bits(f64::NEG_INFINITY)))
            }
            _ => None,
        }
    }

    /// Upper counterpart of [`Self::kind_floor`].
    pub(super) fn kind_ceiling(&self) -> Option<Self> {
        match self {
            Self::Temporal { kind, .. } => Some(Self::temporal(*kind, i128::MAX, i32::MAX)),
            Self::Int(_) | Self::Float(_) => Some(Self::Float(sortable_f64_bits(f64::INFINITY))),
            _ => None,
        }
    }

    pub(super) fn is_temporal(&self) -> bool {
        matches!(self, Self::Temporal { .. })
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

/// Inverse of [`sortable_f64_bits`] (a negative zero comes back positive).
fn float_from_sortable_bits(bits: u64) -> f64 {
    f64::from_bits(if bits & (1 << 63) != 0 {
        bits & !(1 << 63)
    } else {
        !bits
    })
}

/// Exact comparison of an integer with a (non-NaN) float, without the
/// rounding of `i as f64`: every `i64` lies in `[-2^63, 2^63)`, and inside
/// that range a float's whole part converts to `i64` exactly.
fn int_float_cmp(i: i64, f: f64) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    if f >= TWO_63 {
        return Ordering::Less;
    }
    if f < -TWO_63 {
        return Ordering::Greater;
    }
    let whole = f.trunc();
    i.cmp(&(whole as i64))
        .then_with(|| 0.0f64.partial_cmp(&(f - whole)).unwrap_or(Ordering::Equal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::hash::{BuildHasher, BuildHasherDefault};

    use super::super::cow::KeyHasher;

    fn key(value: PropertyValue) -> PropertyIndexKey {
        PropertyIndexKey::from_value(&value).expect("indexable")
    }

    /// List and map keys order, compare and hash as the `Vec` and
    /// `BTreeMap` they were stored as before.
    #[test]
    fn list_and_map_keys_order_like_their_values() {
        let lists: Vec<Vec<i64>> = vec![vec![], vec![0], vec![1, 2], vec![1, 3], vec![2]];
        let maps: Vec<BTreeMap<String, i64>> = [
            &[][..],
            &[("a", 1)],
            &[("a", 1), ("b", 0)],
            &[("a", 1), ("b", 3)],
            &[("a", 2)],
            &[("b", 1)],
        ]
        .iter()
        .map(|entries| entries.iter().map(|&(k, v)| (k.to_string(), v)).collect())
        .collect();
        let list_value =
            |l: &Vec<i64>| PropertyValue::List(l.iter().map(|&v| PropertyValue::Int(v)).collect());
        let map_value = |m: &BTreeMap<String, i64>| {
            PropertyValue::Map(
                m.iter()
                    .map(|(k, &v)| (k.clone(), PropertyValue::Int(v)))
                    .collect(),
            )
        };
        let hash = |k: &PropertyIndexKey| BuildHasherDefault::<KeyHasher>::default().hash_one(k);
        for a in &lists {
            for b in &lists {
                let (ka, kb) = (key(list_value(a)), key(list_value(b)));
                assert_eq!(ka.cmp(&kb), a.cmp(b), "{a:?} vs {b:?}");
                assert_eq!(ka == kb, hash(&ka) == hash(&kb));
            }
        }
        for a in &maps {
            for b in &maps {
                let (ka, kb) = (key(map_value(a)), key(map_value(b)));
                assert_eq!(ka.cmp(&kb), a.cmp(b), "{a:?} vs {b:?}");
                assert_eq!(ka == kb, hash(&ka) == hash(&kb));
            }
        }
    }

    fn ids(hits: Option<Hits<'_>>) -> Vec<u64> {
        hits.map(Hits::to_vec).unwrap_or_default()
    }

    fn string(s: &str) -> Probe<'_> {
        Probe::Str(s)
    }

    /// A value one entity holds is a digest and an id; a second holder
    /// moves it to an exact bucket, and losing that holder moves it back.
    #[test]
    fn a_shared_value_moves_between_the_digest_and_exact_tiers() {
        let mut b = PropertyValueBuckets::default();
        let same = |_: u64| true;
        b.insert(1, &string("a"), &same);
        b.insert(2, &string("b"), &same);
        assert_eq!((b.exact.len(), b.single.len()), (0, 2));
        assert_eq!(ids(b.get(&string("a"), same)), vec![1]);

        b.insert(3, &string("a"), &same);
        assert_eq!((b.exact.len(), b.single.len()), (1, 1));
        assert_eq!(ids(b.get(&string("a"), |_| false)), vec![1, 3]);

        b.remove(1, &string("a"));
        assert_eq!((b.exact.len(), b.single.len()), (0, 2));
        assert_eq!(ids(b.get(&string("a"), same)), vec![3]);

        b.remove(3, &string("a"));
        b.remove(2, &string("b"));
        assert!(b.is_empty());
    }

    /// A digest entry is only an answer once the entity's record confirms
    /// it: an entity whose value has changed, or another value with the
    /// same digest, is not returned.
    #[test]
    fn a_digest_entry_is_confirmed_by_the_record() {
        let mut b = PropertyValueBuckets::default();
        b.insert(7, &string("k"), &|_| true);
        assert_eq!(ids(b.get(&string("k"), |id| id == 7)), vec![7]);
        assert!(b.get(&string("k"), |_| false).is_none());
    }

    /// When a value's digest names an entity that does not hold it (a
    /// digest collision), the new value keeps an exact bucket of its own
    /// and both stay answerable.
    #[test]
    fn a_digest_collision_keeps_both_values() {
        let mut b = PropertyValueBuckets::default();
        b.insert(1, &string("x"), &|_| true);
        // Entity 1 does not hold "x" for this insert: as if "x" and the
        // value entity 1 holds had the same digest.
        b.insert(2, &string("x"), &|_| false);
        assert_eq!((b.exact.len(), b.single.len()), (1, 1));
        assert_eq!(ids(b.get(&string("x"), |_| true)), vec![2]);
        b.remove(2, &string("x"));
        assert_eq!((b.exact.len(), b.single.len()), (0, 1));
        b.remove(1, &string("x"));
        assert!(b.is_empty());
    }

    /// Values with no heap image are always exact: a digest saves nothing
    /// on them, and their lookups read no record.
    #[test]
    fn inline_values_stay_exact() {
        let mut b = PropertyValueBuckets::default();
        let int = Probe::Key(PropertyIndexKey::Int(5));
        b.insert(1, &int, &|_| unreachable!("no digest entry to confirm"));
        assert_eq!((b.exact.len(), b.single.len()), (1, 0));
        assert_eq!(ids(b.get(&int, |_| unreachable!())), vec![1]);
    }

    /// A string probe and a string key reach the same digest entry.
    #[test]
    fn string_probes_and_keys_share_a_digest() {
        let key = PropertyIndexKey::String(Arc::from("same"));
        assert_eq!(Probe::Key(key.clone()).digest(), string("same").digest());
        assert_eq!(digest(&key), digest_str("same"));
    }

    /// The memory a unique string key's index entry costs: a digest and
    /// an id, no copy of the string (G-64). Guards the per-entry layout.
    #[test]
    fn a_unique_string_key_costs_a_digest_entry() {
        let mut b = PropertyValueBuckets::default();
        let keys: Vec<String> = (0..1000)
            .map(|i| format!("musicbrainz:release-group:{i}"))
            .collect();
        for (i, k) in keys.iter().enumerate() {
            b.insert(i as u64, &string(k), &|_| true);
        }
        assert_eq!((b.exact.len(), b.single.len()), (0, 1000));
        assert_eq!(
            std::mem::size_of::<(u64, u64)>(),
            16,
            "a digest entry is a digest and an id"
        );
    }
}
