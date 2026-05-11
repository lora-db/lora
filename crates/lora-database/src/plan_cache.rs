//! Compiled-plan cache keyed by raw query text plus live-store epoch.
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
//! graph/catalog snapshot read at compile time:
//! - The analyzer reads the store only to validate that label /
//!   relationship-type / property-key names exist (analyzer.rs:1110–1170);
//!   it does not embed any store-derived data into the resolved query.
//! - The optimizer reads `GraphStats` for cost-based selection between
//!   competing index rewrites (`use_indexed_node_scans` in
//!   `lora-compiler/src/optimizer.rs`). The cache key includes the
//!   live-store epoch, which bumps after every write, so cardinality shifts
//!   and catalog changes compile into a fresh entry instead of reusing stale
//!   operator choices.
//! - The storage layer still has the final say on index contents at
//!   execution time; the cache only avoids redoing analysis and planning
//!   while the graph/catalog epoch remains unchanged.
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

/// Content-addressed cache mapping `(query text, live-store epoch)` →
/// compiled plan.
///
/// Cloning a `PlanCache` is meaningful: callers wrap it in `Arc` so all
/// `Database` clones (and the read/write phases of a single `execute`) share
/// the same map.
pub(crate) struct PlanCache {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: HashMap<String, Vec<Entry>>,
    /// Monotonic counter used as the "last accessed" stamp for LRU eviction.
    /// Wrapping at u64 takes longer than any reasonable process lifetime, so
    /// we don't worry about overflow.
    counter: u64,
    capacity: usize,
    len: usize,
}

struct Entry {
    store_epoch: u64,
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
                len: 0,
            }),
        }
    }

    /// Look up a cached plan for `query` under a live-store epoch.
    /// Returns `None` on miss.
    ///
    /// On hit, the entry's last-used timestamp is bumped so it survives
    /// eviction longer.
    pub(crate) fn get(&self, query: &str, store_epoch: u64) -> Option<Arc<CompiledQuery>> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        let entry = guard
            .entries
            .get_mut(query)?
            .iter_mut()
            .find(|entry| entry.store_epoch == store_epoch)?;
        entry.last_used = counter;
        Some(entry.plan.clone())
    }

    /// Insert a freshly-compiled plan. If the cache is at capacity, evict
    /// the entry with the oldest `last_used` stamp.
    pub(crate) fn insert(&self, query: &str, store_epoch: u64, plan: Arc<CompiledQuery>) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.capacity == 0 {
            return;
        }
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        if let Some(entries) = guard.entries.get_mut(query) {
            if let Some(entry) = entries
                .iter_mut()
                .find(|entry| entry.store_epoch == store_epoch)
            {
                entry.plan = plan;
                entry.last_used = counter;
                return;
            }
        }

        if guard.len >= guard.capacity {
            evict_oldest(&mut guard);
        }

        guard
            .entries
            .entry(query.to_owned())
            .or_default()
            .push(Entry {
                store_epoch,
                plan,
                last_used: counter,
            });
        guard.len += 1;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len
    }
}

fn evict_oldest(guard: &mut Inner) {
    let oldest = guard
        .entries
        .iter()
        .flat_map(|(query, entries)| {
            entries
                .iter()
                .enumerate()
                .map(move |(idx, entry)| (query.clone(), idx, entry.last_used))
        })
        .min_by_key(|(_, _, last_used)| *last_used);

    if let Some((query, idx, _)) = oldest {
        if let Some(entries) = guard.entries.get_mut(&query) {
            entries.swap_remove(idx);
            guard.len = guard.len.saturating_sub(1);
            if entries.is_empty() {
                guard.entries.remove(&query);
            }
        }
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
    fn distinct_store_epochs_are_independent() {
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
