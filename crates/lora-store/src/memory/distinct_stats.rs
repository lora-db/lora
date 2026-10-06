//! Per-(label / relationship type, property key) distinct-value
//! sketches: the planner's distinct counts (see [`super::GraphStats`]).
//!
//! # Why a sketch, and why this one
//!
//! The distinct counts used to be the bucket count of the *active* hash
//! property index, so they existed only for keys whose index happened to
//! be active: declared ones, or ones an earlier lookup activated. Plans
//! then depended on lookup history, and a restarted process (which no
//! longer activates every key on load) planned differently from the
//! writer. These sketches exist for every key on every label and type,
//! are maintained on every write, and are a pure function of the live
//! data, so the writer, a restarted process and a fresh process that
//! loaded the same data hold identical sketches and plan identically.
//!
//! A HyperLogLog cannot forget a deleted value, so after deletes the
//! writer's estimate would drift above a restarted process's. This is a
//! *counting multi-resolution bitmap* (Estan, Varghese & Fisk, "Bitmap
//! algorithms for counting active flows", 2003) with counters instead of
//! bits:
//!
//! * a value's 64-bit hash picks a level (the number of trailing zero
//!   bits, so level `l` sees a `2^-(l+1)` sample of the distinct values)
//!   and a bucket within it (the top [`BUCKET_BITS`] bits);
//! * each bucket counts the entities whose value hashes there. Insert
//!   increments, delete decrements, so the state after any sequence of
//!   writes equals the state of building from the surviving data;
//! * the estimate linear-counts the occupied buckets of every level from
//!   the first one that is not saturated up, and scales by the sampling
//!   rate of those levels.
//!
//! Accuracy: exact up to about a dozen distinct values (beyond that,
//! bucket collisions start to blur it), then 6–16% RMS relative error,
//! worst seen ~36%, no systematic bias, from a hundred to 1.5M values
//! (`error_profile` below); the planner only compares orders of
//! magnitude. It is not clamped to the number of entities carrying the
//! key, so a unique key can read above its row count.
//!
//! Memory: one level of 64 `u16` counters (128 bytes) per level up to
//! the highest occupied one (`u32`, 256 bytes, once a bucket passes
//! 65 535 entities), plus ~80 bytes of header. `d` distinct values reach
//! about `log2(d) + 2` levels: a few hundred bytes for a key with a
//! handful of values, ~3 KB for a unique key over millions of entities.
//! Sketches sit behind `Arc`, so cloning the graph (every staged write)
//! shares them all, and the first write to a sketch after a clone copies
//! it: two allocations and a `memcpy`, no refcount bumps.
//!
//! Sketches are not persisted. WAL replay rebuilds them through the same
//! hooks as live writes; snapshot load through [`DistinctBuilder`], which
//! produces the same sketches without the copy-on-write checks.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::stats::DistinctValues;
use crate::PropertyValue;

/// Bucket index bits; buckets per level.
const BUCKET_BITS: u32 = 6;
const BUCKETS: usize = 1 << BUCKET_BITS;
/// Levels beyond what any graph reaches: level `l` is entered by one in
/// `2^(l+1)` distinct values, and the last level also takes every hash
/// with more trailing zeros.
const MAX_LEVELS: usize = 40;
/// A level with more occupied buckets than this is saturated: linear
/// counting over it is too noisy, so the estimate starts above it.
/// About 70% full (expected load ≈1.2 values per bucket).
const FILL_LIMIT: u32 = 44;

/// One level's counters.
type Row<C> = [C; BUCKETS];

/// The levels' counters, in one contiguous allocation so copying a
/// sketch (its first write after a graph clone) is one allocation and a
/// `memcpy`, with no refcounts to bump. `u16` until some bucket counts
/// more than 65 535 entities (a value repeated that often), then `u32`
/// for good.
#[derive(Debug, Clone)]
enum Counters {
    Narrow(Vec<Row<u16>>),
    Wide(Vec<Row<u32>>),
}

impl Counters {
    #[inline]
    fn get(&self, level: usize, bucket: usize) -> u32 {
        match self {
            Counters::Narrow(rows) => u32::from(rows[level][bucket]),
            Counters::Wide(rows) => rows[level][bucket],
        }
    }

    #[inline]
    fn set(&mut self, level: usize, bucket: usize, value: u32) {
        match self {
            Counters::Narrow(rows) => match u16::try_from(value) {
                Ok(v) => rows[level][bucket] = v,
                Err(_) => {
                    let mut wide: Vec<Row<u32>> = rows
                        .iter()
                        .map(|row| std::array::from_fn(|i| u32::from(row[i])))
                        .collect();
                    wide[level][bucket] = value;
                    *self = Counters::Wide(wide);
                }
            },
            Counters::Wide(rows) => rows[level][bucket] = value,
        }
    }

    fn resize(&mut self, len: usize) {
        match self {
            Counters::Narrow(rows) => rows.resize(len, [0; BUCKETS]),
            Counters::Wide(rows) => rows.resize(len, [0; BUCKETS]),
        }
    }

    fn row_bytes(&self) -> usize {
        match self {
            Counters::Narrow(_) => std::mem::size_of::<Row<u16>>(),
            Counters::Wide(_) => std::mem::size_of::<Row<u32>>(),
        }
    }
}

impl Default for Counters {
    fn default() -> Self {
        Counters::Narrow(Vec::new())
    }
}

/// One `(scope, key)` sketch. Canonical: the level table ends at the
/// highest non-empty level, so two sketches of the same value multiset
/// hold the same counters whatever writes built them (only the counter
/// width can differ, after a bucket once passed 65 535).
#[derive(Debug, Clone)]
pub(super) struct DistinctSketch {
    /// Entities counted (sum of every counter).
    entries: u64,
    /// Non-zero buckets per level, inline so an estimate reads one
    /// cache line instead of one per level.
    occupied: [u8; MAX_LEVELS],
    /// Levels in use: `counters` has this many rows.
    levels: u8,
    /// [`Self::compute_estimate`], refreshed whenever an occupancy
    /// changes (the only input it has), so reading it is free.
    estimate: u64,
    counters: Counters,
}

impl Default for DistinctSketch {
    fn default() -> Self {
        Self {
            entries: 0,
            occupied: [0; MAX_LEVELS],
            levels: 0,
            estimate: 0,
            counters: Counters::default(),
        }
    }
}

#[inline]
fn position(hash: u64) -> (usize, usize) {
    let level = (hash.trailing_zeros() as usize).min(MAX_LEVELS - 1);
    let bucket = (hash >> (64 - BUCKET_BITS)) as usize;
    (level, bucket)
}

impl DistinctSketch {
    fn add(&mut self, hash: u64) {
        let (level, bucket) = position(hash);
        if usize::from(self.levels) <= level {
            self.levels = (level + 1) as u8;
            self.counters.resize(level + 1);
        }
        let count = self.counters.get(level, bucket);
        self.counters.set(level, bucket, count.saturating_add(1));
        self.entries += 1;
        if count == 0 {
            self.occupied[level] += 1;
            self.estimate = self.compute_estimate();
        }
    }

    /// Undo one [`Self::add`] of `hash`. A hash that was never added is
    /// ignored (it cannot happen while every write goes through the
    /// graph's hooks).
    fn remove(&mut self, hash: u64) {
        let (level, bucket) = position(hash);
        if level >= usize::from(self.levels) || self.counters.get(level, bucket) == 0 {
            debug_assert!(false, "distinct sketch: removing an absent value");
            return;
        }
        let count = self.counters.get(level, bucket) - 1;
        self.counters.set(level, bucket, count);
        self.entries -= 1;
        if count == 0 {
            self.occupied[level] -= 1;
            if self.occupied[level] == 0 && level + 1 == usize::from(self.levels) {
                let mut len = level;
                while len > 0 && self.occupied[len - 1] == 0 {
                    len -= 1;
                }
                self.levels = len as u8;
                self.counters.resize(len);
            }
            self.estimate = self.compute_estimate();
        }
    }

    fn occupied(&self, level: usize) -> u32 {
        u32::from(self.occupied[level])
    }

    /// Estimated number of distinct values (0 for an empty sketch).
    pub(super) fn estimate(&self) -> u64 {
        self.estimate
    }

    /// The estimate from the occupancies alone. Deliberately not clamped
    /// to `entries`: that would move it on every insert of a unique key
    /// (where it often binds), and the cached planner stats with it.
    fn compute_estimate(&self) -> u64 {
        if self.levels == 0 {
            return 0;
        }
        let levels = usize::from(self.levels);
        // First level from which every level up is unsaturated.
        let mut base = levels;
        while base > 0 && self.occupied(base - 1) <= FILL_LIMIT {
            base -= 1;
        }
        // Only reachable if the top level saturates (more than ~2^45
        // distinct values): count from it anyway.
        let base = base.min(levels - 1);
        // Σ linear counts = Σ B·ln(B / empty) = B·ln(Π B / empty): one
        // logarithm per sketch (this runs on every plan-cache miss). The
        // product stays below 64^40 = 2^240.
        let buckets = BUCKETS as f64;
        let product: f64 = (base..levels)
            .map(|l| buckets / (BUCKETS as u32 - self.occupied(l)).max(1) as f64)
            .product();
        let scaled = buckets * product.ln() * (base as f64).exp2();
        (scaled.round() as u64).max(1)
    }

    /// Approximate heap bytes. Counts the level table by length, not
    /// capacity, so the figure is canonical like the sketch itself.
    fn heap_bytes(&self) -> usize {
        usize::from(self.levels) * self.counters.row_bytes()
    }
}

/// Name → shared value, sorted by name. Sorted vectors rather than hash
/// maps: a label has few keys and a graph few labels, one binary search
/// is cheaper than hashing the name, a clone is a refcount bump per
/// entry, and iteration order is deterministic.
type Table<T> = Vec<(Arc<str>, Arc<T>)>;
type KeySketches = Table<DistinctSketch>;
type ScopeSketches = Table<KeySketches>;

fn find<T>(table: &Table<T>, name: &str) -> Result<usize, usize> {
    table.binary_search_by(|(k, _)| (**k).cmp(name))
}

/// [`find`] for an interned key: property keys are shared `Arc<str>`s,
/// and the tables keep the first one they saw, so a short table is
/// matched by pointer before any string is compared.
#[inline]
fn find_interned<T>(table: &Table<T>, name: &Arc<str>) -> Result<usize, usize> {
    if table.len() <= 16 {
        if let Some(i) = table.iter().position(|(k, _)| Arc::ptr_eq(k, name)) {
            return Ok(i);
        }
    }
    find(table, name)
}

/// The (copied-on-write) entry at a `find` result, created at the
/// insertion point if absent; `*changed` is set when it was created.
fn entry_at<'t, T: Clone + Default>(
    table: &'t mut Table<T>,
    found: Result<usize, usize>,
    name: impl FnOnce() -> Arc<str>,
    changed: &mut bool,
) -> &'t mut T {
    let i = match found {
        Ok(i) => i,
        Err(i) => {
            table.insert(i, (name(), Arc::default()));
            *changed = true;
            i
        }
    };
    Arc::make_mut(&mut table[i].1)
}

/// Source of layout ids; see [`DistinctStats::layout`].
static NEXT_LAYOUT: AtomicU64 = AtomicU64::new(1);

/// The map [`DistinctStats::distinct_values`] last built, with what it
/// was built from.
#[derive(Debug, Default)]
struct Published {
    layout: u64,
    estimates: Vec<usize>,
    map: Arc<DistinctValues>,
}

/// The sketches of one entity kind (nodes or relationships), by scope
/// (label / type) then property key. Every level is copy-on-write: a
/// clone is two refcount bumps, the first write after it copies the
/// scope table and the scope's key table (two refcount bumps per entry)
/// and the touched sketch (one allocation and a `memcpy`).
#[derive(Debug, Clone, Default)]
pub(super) struct DistinctStats {
    scopes: Arc<ScopeSketches>,
    /// Names the set of (scope, key) pairs: replaced by a process-wide
    /// fresh id whenever a sketch is created or dropped, kept otherwise,
    /// so two stats (clones included) with the same id have the same
    /// pairs in the same order. 0 is the empty layout.
    layout: u64,
    /// The last map [`Self::distinct_values`] built, shared by every
    /// clone of these stats (staged copies, reader snapshots) and reused
    /// by any of them whose layout and estimates it matches. Most writes
    /// leave every estimate unchanged, so the planner's stats after a
    /// write usually cost the estimates and a comparison rather than two
    /// `String`s per (scope, key).
    published: Arc<Mutex<Published>>,
}

impl DistinctStats {
    /// `(scope, key) → estimated distinct values` for every counted key,
    /// as [`super::GraphStats`] holds it.
    pub(super) fn distinct_values(&self) -> Arc<DistinctValues> {
        let mut published = match self.published.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if published.layout == self.layout
            && published
                .estimates
                .iter()
                .copied()
                .eq(self.estimates().map(|(_, _, d)| d))
        {
            return Arc::clone(&published.map);
        }
        let estimates: Vec<usize> = self.estimates().map(|(_, _, d)| d).collect();
        let map: Arc<DistinctValues> = Arc::new(
            self.estimates()
                .map(|(scope, key, distinct)| ((scope.to_owned(), key.to_owned()), distinct))
                .collect(),
        );
        *published = Published {
            layout: self.layout,
            estimates,
            map: Arc::clone(&map),
        };
        map
    }

    fn relayout(&mut self) {
        self.layout = NEXT_LAYOUT.fetch_add(1, Ordering::Relaxed);
    }

    /// Count every property of one entity under `scope`. Values the hash
    /// index can't key (NaN, durations, points, vectors) are not
    /// counted, as they can't be sought either.
    pub(super) fn add_all<'a>(
        &mut self,
        scope: &str,
        properties: impl IntoIterator<Item = (&'a Arc<str>, &'a PropertyValue)>,
    ) {
        let mut hashed = properties
            .into_iter()
            .filter_map(|(key, value)| value_hash(value).map(|hash| (key, hash)));
        // The scope is created only once something is counted in it.
        let Some(first) = hashed.next() else {
            return;
        };
        let mut changed = false;
        let scopes = Arc::make_mut(&mut self.scopes);
        let found = find(scopes, scope);
        let keys = entry_at(scopes, found, || Arc::from(scope), &mut changed);
        for (key, hash) in std::iter::once(first).chain(hashed) {
            let found = find_interned(keys, key);
            entry_at(keys, found, || Arc::clone(key), &mut changed).add(hash);
        }
        if changed {
            self.relayout();
        }
    }

    /// Count `value` under `(scope, key)`; see [`Self::add_all`].
    pub(super) fn add(&mut self, scope: &str, key: &str, value: &PropertyValue) {
        let Some(hash) = value_hash(value) else {
            return;
        };
        let mut changed = false;
        let scopes = Arc::make_mut(&mut self.scopes);
        let found = find(scopes, scope);
        let keys = entry_at(scopes, found, || Arc::from(scope), &mut changed);
        let found = find(keys, key);
        entry_at(keys, found, || crate::intern(key), &mut changed).add(hash);
        if changed {
            self.relayout();
        }
    }

    /// Undo [`Self::add`] of `value`, dropping the sketch (and scope)
    /// once nothing is counted.
    pub(super) fn remove(&mut self, scope: &str, key: &str, value: &PropertyValue) {
        let Some(hash) = value_hash(value) else {
            return;
        };
        let Some((s, k)) = find(&self.scopes, scope)
            .ok()
            .and_then(|s| find(&self.scopes[s].1, key).ok().map(|k| (s, k)))
        else {
            debug_assert!(false, "distinct sketch: removing from an absent key");
            return;
        };
        let scopes = Arc::make_mut(&mut self.scopes);
        let keys = Arc::make_mut(&mut scopes[s].1);
        let sketch = Arc::make_mut(&mut keys[k].1);
        sketch.remove(hash);
        if sketch.entries == 0 {
            keys.remove(k);
            if keys.is_empty() {
                scopes.remove(s);
            }
            self.relayout();
        }
    }

    /// Replace `old` by `new` under `(scope, key)`; a no-op when both
    /// hash the same.
    pub(super) fn replace(
        &mut self,
        scope: &str,
        key: &str,
        old: Option<&PropertyValue>,
        new: &PropertyValue,
    ) {
        if let Some(old) = old {
            if value_hash(old) == value_hash(new) {
                return;
            }
            self.remove(scope, key, old);
        }
        self.add(scope, key, new);
    }

    /// `(scope, key, estimated distinct values)` for every counted key,
    /// sorted by scope, then key.
    pub(super) fn estimates(&self) -> impl Iterator<Item = (&str, &str, usize)> + '_ {
        self.scopes.iter().flat_map(|(scope, keys)| {
            keys.iter()
                .map(move |(key, sketch)| (&**scope, &**key, sketch.estimate() as usize))
        })
    }

    /// Approximate retained heap bytes (see `MemoryReport`).
    pub(super) fn heap_bytes(&self) -> usize {
        // A table entry (`Arc<str>` + `Arc`) plus the `Arc` header.
        const ENTRY: usize = 32 + 16;
        self.scopes
            .iter()
            .map(|(scope, keys)| {
                ENTRY
                    + scope.len()
                    + 16
                    + keys
                        .iter()
                        .map(|(_, s)| {
                            ENTRY + std::mem::size_of::<DistinctSketch>() + s.heap_bytes()
                        })
                        .sum::<usize>()
            })
            .sum()
    }

    /// Number of `(scope, key)` sketches.
    pub(super) fn sketch_count(&self) -> usize {
        self.scopes.iter().map(|(_, keys)| keys.len()).sum()
    }
}

/// Builds a [`DistinctStats`] from scratch without the copy-on-write
/// checks (an atomic compare-and-swap per table level per value) that
/// [`DistinctStats::add_all`] pays: snapshot load counts every property
/// of every entity once, into plain owned tables, then wraps them.
/// A scope's sketches while a [`DistinctBuilder`] owns them.
type OwnedKeySketches = Vec<(Arc<str>, DistinctSketch)>;

#[derive(Default)]
pub(super) struct DistinctBuilder {
    scopes: Vec<(Arc<str>, OwnedKeySketches)>,
    /// Index of the scope used last: consecutive entities mostly share it.
    last: usize,
}

impl DistinctBuilder {
    /// As [`DistinctStats::add_all`].
    pub(super) fn add_all<'a>(
        &mut self,
        scope: &str,
        properties: impl IntoIterator<Item = (&'a Arc<str>, &'a PropertyValue)>,
    ) {
        let mut hashed = properties
            .into_iter()
            .filter_map(|(key, value)| value_hash(value).map(|hash| (key, hash)));
        let Some(first) = hashed.next() else {
            return;
        };
        if self
            .scopes
            .get(self.last)
            .is_none_or(|(s, _)| &**s != scope)
        {
            self.last = match self.scopes.binary_search_by(|(s, _)| (**s).cmp(scope)) {
                Ok(i) => i,
                Err(i) => {
                    self.scopes.insert(i, (Arc::from(scope), Vec::new()));
                    i
                }
            };
        }
        let keys = &mut self.scopes[self.last].1;
        for (key, hash) in std::iter::once(first).chain(hashed) {
            let found = match keys.iter().position(|(k, _)| Arc::ptr_eq(k, key)) {
                Some(i) => Ok(i),
                None => keys.binary_search_by(|(k, _)| (**k).cmp(&**key)),
            };
            let i = found.unwrap_or_else(|i| {
                keys.insert(i, (Arc::clone(key), DistinctSketch::default()));
                i
            });
            keys[i].1.add(hash);
        }
    }

    pub(super) fn finish(self) -> DistinctStats {
        let scopes: ScopeSketches = self
            .scopes
            .into_iter()
            .map(|(scope, keys)| {
                let keys: KeySketches = keys
                    .into_iter()
                    .map(|(key, sketch)| (key, Arc::new(sketch)))
                    .collect();
                (scope, Arc::new(keys))
            })
            .collect();
        let mut stats = DistinctStats {
            scopes: Arc::new(scopes),
            ..DistinctStats::default()
        };
        if !stats.scopes.is_empty() {
            stats.relayout();
        }
        stats
    }
}

/// The node and relationship sketches of a graph.
#[derive(Debug, Clone, Default)]
pub(super) struct DistinctStatsRegistry {
    pub(super) nodes: DistinctStats,
    pub(super) relationships: DistinctStats,
}

// ---------------------------------------------------------------------------
// value hashing
// ---------------------------------------------------------------------------

/// A deterministic 64-bit hash of `value`, equal for values the hash
/// property index treats as equal (see `PropertyIndexKey::from_value`):
/// floats normalise `-0.0`, temporals hash the instant they denote plus
/// offset and zone, and `None` for the values that index can't key.
/// Stable across processes (no random seed), so a restarted process
/// builds the same sketches.
pub(super) fn value_hash(value: &PropertyValue) -> Option<u64> {
    let mut h = Mix::default();
    h.value(value)?;
    Some(h.finish())
}

#[derive(Default)]
struct Mix(u64);

const K: u64 = 0x517c_c1b7_2722_0a95;

impl Mix {
    #[inline]
    fn word(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(K);
    }

    #[inline]
    fn bytes(&mut self, bytes: &[u8]) {
        self.word(bytes.len() as u64);
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            self.word(u64::from_le_bytes(chunk.try_into().expect("8 bytes")));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut tail = [0u8; 8];
            tail[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(tail));
        }
    }

    fn i128(&mut self, x: i128) {
        self.word(x as u64);
        self.word((x >> 64) as u64);
    }

    fn value(&mut self, value: &PropertyValue) -> Option<()> {
        match value {
            PropertyValue::Null => self.word(0),
            PropertyValue::Bool(v) => {
                self.word(1);
                self.word(u64::from(*v));
            }
            PropertyValue::Int(v) => {
                self.word(2);
                self.word(*v as u64);
            }
            PropertyValue::Float(v) => {
                if v.is_nan() {
                    return None;
                }
                self.word(3);
                self.word(if *v == 0.0 { 0 } else { v.to_bits() });
            }
            PropertyValue::String(v) => {
                self.word(4);
                self.bytes(v.as_bytes());
            }
            PropertyValue::Binary(v) => {
                // Byte-streamed: equal contents hash the same however
                // they are segmented.
                self.word(5);
                self.word(v.chunks().map(|c| c.len() as u64).sum());
                for byte in v.chunks().flatten() {
                    self.word(u64::from(*byte));
                }
            }
            PropertyValue::List(values) => {
                self.word(6);
                self.word(values.len() as u64);
                for v in values {
                    self.value(v)?;
                }
            }
            PropertyValue::Map(values) => {
                self.word(7);
                self.word(values.len() as u64);
                for (k, v) in values {
                    self.bytes(k.as_bytes());
                    self.value(v)?;
                }
            }
            PropertyValue::Date(v) => self.temporal(0, v.order_nanos(), 0, None),
            PropertyValue::LocalTime(v) => self.temporal(1, v.order_nanos(), 0, None),
            PropertyValue::Time(v) => self.temporal(2, v.order_nanos(), v.offset_seconds, None),
            PropertyValue::LocalDateTime(v) => self.temporal(3, v.order_nanos(), 0, None),
            PropertyValue::DateTime(v) => {
                let zone = v.zone.map(|z| {
                    use std::hash::{Hash, Hasher};
                    let mut zh = Mix::default();
                    struct H<'a>(&'a mut Mix);
                    impl Hasher for H<'_> {
                        fn finish(&self) -> u64 {
                            self.0 .0
                        }
                        fn write(&mut self, bytes: &[u8]) {
                            self.0.bytes(bytes);
                        }
                        fn write_u16(&mut self, x: u16) {
                            self.0.word(u64::from(x));
                        }
                    }
                    z.hash(&mut H(&mut zh));
                    zh.0
                });
                self.temporal(4, v.order_nanos(), v.offset_seconds, zone);
            }
            PropertyValue::Duration(_) | PropertyValue::Point(_) | PropertyValue::Vector(_) => {
                return None
            }
        }
        Some(())
    }

    fn temporal(&mut self, kind: u64, nanos: i128, offset: i32, zone: Option<u64>) {
        self.word(8);
        self.word(kind);
        self.i128(nanos);
        self.word(offset as u32 as u64);
        match zone {
            None => self.word(0),
            Some(z) => {
                self.word(1);
                self.word(z);
            }
        }
    }

    /// MurmurHash3's 64-bit finaliser: every input bit reaches every
    /// output bit, which the level (low bits) and bucket (high bits)
    /// choices rely on.
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 33)).wrapping_mul(0xff51_afd7_ed55_8ccd);
        z = (z ^ (z >> 33)).wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        z ^ (z >> 33)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sketch_of(values: impl IntoIterator<Item = PropertyValue>) -> DistinctSketch {
        let mut s = DistinctSketch::default();
        for v in values {
            s.add(value_hash(&v).unwrap());
        }
        s
    }

    #[test]
    fn small_counts_are_exact() {
        for n in 1..=12i64 {
            // Each value several times: duplicates don't count.
            let s = sketch_of((0..n * 3).map(|i| PropertyValue::Int(i % n)));
            assert_eq!(s.estimate(), n as u64, "n={n}");
        }
    }

    #[test]
    fn large_counts_are_close() {
        let mut worst: f64 = 0.0;
        for &n in &[
            50u64, 100, 300, 1_000, 3_000, 10_000, 30_000, 100_000, 300_000, 1_000_000,
        ] {
            for seed in 0..3u64 {
                let s = sketch_of((0..n).map(|i| PropertyValue::String(format!("v{seed}-{i}"))));
                let err = (s.estimate() as f64 - n as f64).abs() / n as f64;
                worst = worst.max(err);
                assert!(err < 0.35, "n={n} seed={seed} est={}", s.estimate());
            }
        }
        eprintln!("worst relative error {worst:.3}");
    }

    #[test]
    fn deletes_restore_the_rebuilt_state() {
        let mut stats = DistinctStats::default();
        let mut rebuilt = DistinctStats::default();
        for i in 0..5_000i64 {
            stats.add("L", "k", &PropertyValue::Int(i));
        }
        for i in 0..5_000i64 {
            if i % 3 == 0 {
                stats.remove("L", "k", &PropertyValue::Int(i));
            } else {
                rebuilt.add("L", "k", &PropertyValue::Int(i));
            }
        }
        let a: Vec<_> = stats.estimates().collect();
        let b: Vec<_> = rebuilt.estimates().collect();
        assert_eq!(a, b);
        assert_eq!(stats.heap_bytes(), rebuilt.heap_bytes());
        for i in 0..5_000i64 {
            if i % 3 != 0 {
                stats.remove("L", "k", &PropertyValue::Int(i));
            }
        }
        assert_eq!(stats.estimates().count(), 0);
        assert_eq!(stats.sketch_count(), 0);
    }

    #[test]
    fn counters_widen_past_u16() {
        let mut stats = DistinctStats::default();
        for i in 0..70_000i64 {
            stats.add("L", "k", &PropertyValue::Int(i % 3));
        }
        stats.add("L", "k", &PropertyValue::Int(7));
        assert_eq!(stats.estimates().next().unwrap().2, 4);
        for i in 0..70_000i64 {
            stats.remove("L", "k", &PropertyValue::Int(i % 3));
        }
        assert_eq!(stats.estimates().next().unwrap().2, 1);
        stats.remove("L", "k", &PropertyValue::Int(7));
        assert_eq!(stats.sketch_count(), 0);
    }

    #[test]
    fn hash_follows_index_equality() {
        let h = |v: PropertyValue| value_hash(&v);
        assert_eq!(h(PropertyValue::Float(0.0)), h(PropertyValue::Float(-0.0)));
        assert_ne!(h(PropertyValue::Int(1)), h(PropertyValue::Float(1.0)));
        assert_ne!(
            h(PropertyValue::String("1".into())),
            h(PropertyValue::Int(1))
        );
        assert_eq!(h(PropertyValue::Float(f64::NAN)), None);
        assert_eq!(
            h(PropertyValue::Binary(crate::LoraBinary::from(vec![
                1, 2, 3
            ]))),
            h(PropertyValue::Binary(crate::LoraBinary::from(vec![
                1, 2, 3
            ])))
        );
    }

    #[test]
    fn clone_shares_until_written() {
        let mut a = DistinctStats::default();
        a.add("L", "k", &PropertyValue::Int(1));
        let b = a.clone();
        assert!(Arc::ptr_eq(&a.scopes, &b.scopes));
        a.add("L", "k", &PropertyValue::Int(2));
        assert_eq!(b.estimates().next().unwrap().2, 1);
        assert_eq!(a.estimates().next().unwrap().2, 2);
    }
}
/// `cargo test --release -p lora-store --lib error_profile -- --ignored
/// --nocapture` prints the estimator's relative error by size.
#[cfg(test)]
mod profile {
    use super::*;
    #[test]
    #[ignore]
    fn error_profile() {
        let mut n = 10u64;
        while n <= 2_000_000 {
            let mut errs = Vec::new();
            for seed in 0..20u64 {
                let mut s = DistinctSketch::default();
                for i in 0..n {
                    s.add(value_hash(&PropertyValue::Int((seed << 40 | i) as i64)).unwrap());
                }
                errs.push((s.estimate() as f64 - n as f64) / n as f64);
            }
            let rms = (errs.iter().map(|e| e * e).sum::<f64>() / errs.len() as f64).sqrt();
            let max = errs.iter().fold(0f64, |a, e| a.max(e.abs()));
            let mean = errs.iter().sum::<f64>() / errs.len() as f64;
            eprintln!("n={n:>8} rms={rms:.3} max={max:.3} bias={mean:+.3}");
            n = n * 3 / 2 + 1;
        }
    }
}
