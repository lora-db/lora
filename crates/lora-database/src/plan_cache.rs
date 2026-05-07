//! Compiled-plan cache keyed by raw query text.
//!
//! Parse + analyze + compile costs the same handful of microseconds for every
//! `Database::execute_with_params` call, even when the query text is reused
//! across thousands of executions with different parameters. Caching the
//! `CompiledQuery` collapses that cost to a hashmap lookup on the steady-state
//! hot path.
//!
//! # Why this is safe without invalidation
//!
//! The compiled plan is a pure function of the parsed `Document`:
//! - The analyzer reads the store only to validate that label /
//!   relationship-type / property-key names exist (analyzer.rs:1110–1170);
//!   it does not embed any store-derived data into the resolved query.
//! - The optimizer is fully store-agnostic (optimizer.rs:20–25): its
//!   rewrites and physical lowering depend only on the resolved query.
//! - Index decisions are made dynamically by the storage layer at execution
//!   time (e.g. `indexed_node_property_candidates` in
//!   `lora-store/src/memory.rs`), not baked into the plan.
//!
//! Consequence: once a plan compiles successfully, replaying it against any
//! later store state produces correct results. The most aggressive thing that
//! can happen is `db.clear()` — a cached plan that referenced labels which
//! no longer exist will simply return zero rows, which is the same answer it
//! would give if recompiled.
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

/// Content-addressed cache mapping query text → compiled plan.
///
/// Cloning a `PlanCache` is meaningful: callers wrap it in `Arc` so all
/// `Database` clones (and the read/write phases of a single `execute`) share
/// the same map.
pub(crate) struct PlanCache {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: HashMap<String, Entry>,
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

    /// Look up a cached plan for `query`. Returns `None` on miss.
    ///
    /// On hit, the entry's last-used timestamp is bumped so it survives
    /// eviction longer.
    pub(crate) fn get(&self, query: &str) -> Option<Arc<CompiledQuery>> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        let entry = guard.entries.get_mut(query)?;
        entry.last_used = counter;
        Some(entry.plan.clone())
    }

    /// Insert a freshly-compiled plan. If the cache is at capacity, evict
    /// the entry with the oldest `last_used` stamp.
    pub(crate) fn insert(&self, query: &str, plan: Arc<CompiledQuery>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.capacity == 0 {
            return;
        }
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        if guard.entries.len() >= guard.capacity && !guard.entries.contains_key(query) {
            evict_oldest(&mut guard.entries);
        }
        guard.entries.insert(
            query.to_owned(),
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

fn evict_oldest(entries: &mut HashMap<String, Entry>) {
    let oldest_key = entries
        .iter()
        .min_by_key(|(_, e)| e.last_used)
        .map(|(k, _)| k.clone());
    if let Some(k) = oldest_key {
        entries.remove(k.as_str());
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
        assert!(cache.get(q).is_none());
        cache.insert(q, dummy_plan());
        let hit = cache.get(q).expect("expected cache hit");
        // Inserting again should not duplicate the entry.
        cache.insert(q, hit.clone());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn distinct_queries_are_independent() {
        let cache = PlanCache::new();
        cache.insert("MATCH (n) RETURN n", dummy_plan());
        cache.insert("MATCH (m) RETURN m", dummy_plan());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("MATCH (n) RETURN n").is_some());
        assert!(cache.get("MATCH (m) RETURN m").is_some());
    }

    #[test]
    fn lru_evicts_oldest() {
        let cache = PlanCache::with_capacity(2);
        cache.insert("a", dummy_plan());
        cache.insert("b", dummy_plan());
        // Touch "a" so "b" becomes the LRU.
        let _ = cache.get("a");
        cache.insert("c", dummy_plan());
        assert_eq!(cache.len(), 2);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_none());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn zero_capacity_disables_storage() {
        let cache = PlanCache::with_capacity(0);
        cache.insert("MATCH (n) RETURN n", dummy_plan());
        assert_eq!(cache.len(), 0);
        assert!(cache.get("MATCH (n) RETURN n").is_none());
    }
}
