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
//! # Write-heavy workloads
//!
//! Every write bumps the epoch, so a mutating query never hits a plan
//! compiled for an earlier epoch. Two things keep that cheap:
//! - The parsed [`Document`] is a pure function of the query text, so it
//!   is kept per query across epochs and a recompile skips the parser
//!   (the parser is the largest share of compile time).
//! - Only the [`EPOCHS_PER_QUERY`] newest plans are kept per query.
//!   Epochs only move forward, so older plans can never hit again once a
//!   newer epoch is live; keeping them would fill the cache with dead
//!   entries and push out plans for other queries.
//!
//! # Eviction
//!
//! A small bounded LRU over query texts keeps the working set hot without
//! unbounded growth. On overflow we evict the query with the oldest access
//! counter. The eviction scan is `O(capacity)` with `capacity = 256` and
//! allocates nothing; it is paid only when a new query text is admitted.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use lora_ast::Document;
use lora_compiler::CompiledQuery;

/// Default capacity, counted in cached plans. 256 entries comfortably
/// covers the working set of the realistic benchmark suites without
/// burning memory on plans that are allocated once and never reused.
const DEFAULT_CAPACITY: usize = 256;

/// Plans kept per query text. Two covers a reader that compiled against
/// the epoch just before a concurrent write landed.
const EPOCHS_PER_QUERY: usize = 2;

/// Content-addressed cache mapping `(query text, live-store epoch)` →
/// compiled plan, plus query text → parsed document.
///
/// Cloning a `PlanCache` is meaningful: callers wrap it in `Arc` so all
/// `Database` clones (and the read/write phases of a single `execute`) share
/// the same map.
pub(crate) struct PlanCache {
    inner: Mutex<Inner>,
}

struct Inner {
    entries: HashMap<String, Slot>,
    /// Monotonic counter used as the "last accessed" stamp for LRU eviction.
    /// Wrapping at u64 takes longer than any reasonable process lifetime, so
    /// we don't worry about overflow.
    counter: u64,
    /// Maximum number of cached plans across all queries.
    capacity: usize,
    /// Number of cached plans across all queries.
    len: usize,
}

struct Slot {
    /// Parsed form of the query text, reused across epochs. `None` when
    /// the plan was inserted without one (tests, callers that parsed
    /// elsewhere).
    document: Option<Arc<Document>>,
    /// At most [`EPOCHS_PER_QUERY`] plans, one per epoch.
    plans: Vec<Entry>,
    last_used: u64,
}

struct Entry {
    store_epoch: u64,
    plan: Arc<CompiledQuery>,
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Look up a cached plan for `query` under a live-store epoch.
    /// Returns `None` on miss.
    ///
    /// On hit, the entry's last-used timestamp is bumped so it survives
    /// eviction longer.
    pub(crate) fn get(&self, query: &str, store_epoch: u64) -> Option<Arc<CompiledQuery>> {
        let mut guard = self.lock();
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;
        let slot = guard.entries.get_mut(query)?;
        let plan = slot
            .plans
            .iter()
            .find(|entry| entry.store_epoch == store_epoch)?
            .plan
            .clone();
        slot.last_used = counter;
        Some(plan)
    }

    /// The cached parse of `query`, if any epoch of it has been compiled.
    pub(crate) fn document(&self, query: &str) -> Option<Arc<Document>> {
        self.lock()
            .entries
            .get(query)
            .and_then(|slot| slot.document.clone())
    }

    /// Insert a freshly-compiled plan. If the cache is at capacity, evict
    /// the least recently used query.
    #[cfg(test)]
    pub(crate) fn insert(&self, query: &str, store_epoch: u64, plan: Arc<CompiledQuery>) {
        self.insert_with_document(query, store_epoch, None, plan);
    }

    pub(crate) fn insert_with_document(
        &self,
        query: &str,
        store_epoch: u64,
        document: Option<Arc<Document>>,
        plan: Arc<CompiledQuery>,
    ) {
        let mut guard = self.lock();
        if guard.capacity == 0 {
            return;
        }
        let counter = guard.counter.wrapping_add(1);
        guard.counter = counter;

        if let Some(slot) = guard.entries.get_mut(query) {
            slot.last_used = counter;
            if slot.document.is_none() {
                slot.document = document;
            }
            if let Some(entry) = slot
                .plans
                .iter_mut()
                .find(|entry| entry.store_epoch == store_epoch)
            {
                entry.plan = plan;
                return;
            }
            if slot.plans.len() >= EPOCHS_PER_QUERY {
                // Replace the oldest epoch in place; the plan count is
                // unchanged. A plan older than everything cached is
                // already stale and not worth keeping.
                let (oldest_idx, oldest_epoch) = slot
                    .plans
                    .iter()
                    .enumerate()
                    .map(|(idx, entry)| (idx, entry.store_epoch))
                    .min_by_key(|&(_, epoch)| epoch)
                    .expect("slot holds plans");
                if store_epoch > oldest_epoch {
                    slot.plans[oldest_idx] = Entry { store_epoch, plan };
                }
                return;
            }
            slot.plans.push(Entry { store_epoch, plan });
            guard.len += 1;
            while guard.len > guard.capacity {
                if !evict_oldest(&mut guard, Some(query)) {
                    break;
                }
            }
            return;
        }

        while guard.len >= guard.capacity {
            if !evict_oldest(&mut guard, None) {
                break;
            }
        }
        guard.entries.insert(
            query.to_owned(),
            Slot {
                document,
                plans: vec![Entry { store_epoch, plan }],
                last_used: counter,
            },
        );
        guard.len += 1;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len
    }
}

/// Drop the least recently used query (all of its plans), skipping
/// `keep`. Returns `false` when nothing could be evicted.
fn evict_oldest(guard: &mut Inner, keep: Option<&str>) -> bool {
    let oldest = guard
        .entries
        .iter()
        .filter(|(query, _)| Some(query.as_str()) != keep)
        .min_by_key(|(_, slot)| slot.last_used)
        .map(|(query, _)| query.clone());
    let Some(query) = oldest else {
        return false;
    };
    if let Some(slot) = guard.entries.remove(&query) {
        guard.len = guard.len.saturating_sub(slot.plans.len());
    }
    true
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
            parameters: Default::default(),
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
    fn keeps_only_newest_epochs_per_query() {
        let cache = PlanCache::new();
        let q = "CREATE (:N)";
        for epoch in 1..=10 {
            cache.insert(q, epoch, dummy_plan());
        }
        assert_eq!(cache.len(), EPOCHS_PER_QUERY);
        assert!(cache.get(q, 10).is_some());
        assert!(cache.get(q, 9).is_some());
        assert!(cache.get(q, 8).is_none());
        // An out-of-order insert for an already-stale epoch is dropped.
        cache.insert(q, 3, dummy_plan());
        assert!(cache.get(q, 3).is_none());
    }

    #[test]
    fn document_survives_epoch_changes() {
        let cache = PlanCache::new();
        let q = "MATCH (n) RETURN n";
        let doc = Arc::new(lora_parser::parse_query(q).unwrap());
        cache.insert_with_document(q, 1, Some(doc.clone()), dummy_plan());
        cache.insert(q, 2, dummy_plan());
        cache.insert(q, 3, dummy_plan());
        assert!(Arc::ptr_eq(&cache.document(q).unwrap(), &doc));
    }

    #[test]
    fn zero_capacity_disables_storage() {
        let cache = PlanCache::with_capacity(0);
        cache.insert("MATCH (n) RETURN n", 1, dummy_plan());
        assert_eq!(cache.len(), 0);
        assert!(cache.get("MATCH (n) RETURN n", 1).is_none());
    }
}
