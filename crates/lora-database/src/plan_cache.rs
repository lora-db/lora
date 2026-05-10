//! Compiled-plan cache keyed by raw query text plus graph-stats fingerprint.
//!
//! Parse + analyze + compile costs the same handful of microseconds for every
//! `Database::execute_with_params` call, even when the query text is reused
//! across thousands of executions with different parameters. Caching the
//! `CompiledQuery` collapses that cost to a hashmap lookup on the steady-state
//! hot path.
//!
//! # Why this is safe
//!
//! The compiled plan is a pure function of the parsed `Document` plus the
//! cardinality snapshot read at compile time:
//! - The analyzer reads the store only to validate that label /
//!   relationship-type / property-key names exist (analyzer.rs:1110–1170);
//!   it does not embed any store-derived data into the resolved query.
//! - The optimizer reads `GraphStats` for cost-based selection between
//!   competing index rewrites (`use_indexed_node_scans` in
//!   `lora-compiler/src/optimizer.rs`). The cache key includes the
//!   stats fingerprint, so cardinality shifts and catalog changes
//!   compile into a fresh entry instead of reusing stale operator
//!   choices.
//! - The storage layer still has the final say on index contents at
//!   execution time; the cache only avoids redoing analysis and planning
//!   while the stats/catalog fingerprint remains unchanged.
//!
//! # Eviction
//!
//! A small bounded LRU keeps the working set hot without unbounded growth.
//! On overflow we evict the entry with the oldest access counter. The
//! eviction scan is `O(capacity)` with `capacity = 256` — a few microseconds
//! at most, paid only on cache miss.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use lora_compiler::CompiledQuery;

/// Default capacity. 256 entries comfortably covers the working set of the
/// realistic benchmark suites without burning memory on plans that are
/// allocated once and never reused.
const DEFAULT_CAPACITY: usize = 256;

/// Content-addressed cache mapping `(query text, stats fingerprint)` →
/// compiled plan.
///
/// Cloning a `PlanCache` is meaningful: callers wrap it in `Arc` so all
/// `Database` clones (and the read/write phases of a single `execute`) share
/// the same map.
pub(crate) struct PlanCache {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: HashMap<CacheKey, Entry>,
    /// Monotonic counter used as the "last accessed" stamp for LRU eviction.
    /// Wrapping at u64 takes longer than any reasonable process lifetime, so
    /// we don't worry about overflow.
    counter: u64,
    capacity: usize,
}

struct Entry {
    plan: Arc<CompiledQuery>,
    last_used: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    query: String,
    stats_fingerprint: u64,
}

impl CacheKey {
    fn new(query: &str, stats_fingerprint: u64) -> Self {
        Self {
            query: query.to_owned(),
            stats_fingerprint,
        }
    }
}

impl Default for PlanCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanCache {
    pub(crate) fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: HashMap::with_capacity(capacity),
                counter: 0,
                capacity,
            }),
        }
    }

    /// Look up a cached plan for `query` under `stats_fingerprint`.
    /// Returns `None` on miss.
    ///
    /// On hit, the entry's last-used timestamp is bumped so it survives
    /// eviction longer.
    pub(crate) fn get(&self, query: &str, stats_fingerprint: u64) -> Option<Arc<CompiledQuery>> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        let entry = guard
            .entries
            .get_mut(&CacheKey::new(query, stats_fingerprint))?;
        entry.last_used = counter;
        Some(entry.plan.clone())
    }

    /// Insert a freshly-compiled plan. If the cache is at capacity, evict
    /// the entry with the oldest `last_used` stamp.
    pub(crate) fn insert(&self, query: &str, stats_fingerprint: u64, plan: Arc<CompiledQuery>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.capacity == 0 {
            return;
        }
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        let key = CacheKey::new(query, stats_fingerprint);
        if guard.entries.len() >= guard.capacity && !guard.entries.contains_key(&key) {
            evict_oldest(&mut guard.entries);
        }
        guard.entries.insert(
            key,
            Entry {
                plan,
                last_used: counter,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .len()
    }
}

fn evict_oldest(entries: &mut HashMap<CacheKey, Entry>) {
    let oldest_key = entries
        .iter()
        .min_by_key(|(_, e)| e.last_used)
        .map(|(k, _)| k.clone());
    if let Some(k) = oldest_key {
        entries.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lora_compiler::{PhysicalOp, PhysicalPlan};

    fn dummy_plan() -> Arc<CompiledQuery> {
        // Minimal placeholder; we only care that the same Arc is handed out.
        Arc::new(CompiledQuery {
            physical: PhysicalPlan {
                root: 0,
                nodes: vec![PhysicalOp::Argument(lora_compiler::ArgumentExec)],
            },
            unions: Vec::new(),
        })
    }

    #[test]
    fn miss_then_hit() {
        let cache = PlanCache::new();
        let q = "MATCH (n) RETURN n";
        assert!(cache.get(q, 1).is_none());
        cache.insert(q, 1, dummy_plan());
        let hit = cache.get(q, 1).expect("expected cache hit");
        // Inserting again should not duplicate the entry.
        cache.insert(q, 1, hit.clone());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn distinct_queries_are_independent() {
        let cache = PlanCache::new();
        cache.insert("MATCH (n) RETURN n", 1, dummy_plan());
        cache.insert("MATCH (m) RETURN m", 1, dummy_plan());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("MATCH (n) RETURN n", 1).is_some());
        assert!(cache.get("MATCH (m) RETURN m", 1).is_some());
    }

    #[test]
    fn distinct_stats_fingerprints_are_independent() {
        let cache = PlanCache::new();
        let q = "MATCH (n) RETURN n";
        cache.insert(q, 1, dummy_plan());
        cache.insert(q, 2, dummy_plan());
        assert_eq!(cache.len(), 2);
        assert!(cache.get(q, 1).is_some());
        assert!(cache.get(q, 2).is_some());
    }

    #[test]
    fn lru_evicts_oldest() {
        let cache = PlanCache::with_capacity(2);
        cache.insert("a", 1, dummy_plan());
        cache.insert("b", 1, dummy_plan());
        // Touch "a" so "b" becomes the LRU.
        let _ = cache.get("a", 1);
        cache.insert("c", 1, dummy_plan());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("a", 1).is_some());
        assert!(cache.get("b", 1).is_none());
        assert!(cache.get("c", 1).is_some());
    }

    #[test]
    fn zero_capacity_disables_storage() {
        let cache = PlanCache::with_capacity(0);
        cache.insert("MATCH (n) RETURN n", 1, dummy_plan());
        assert_eq!(cache.len(), 0);
        assert!(cache.get("MATCH (n) RETURN n", 1).is_none());
    }
}
