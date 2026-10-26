//! Compiled-plan cache keyed by raw query text plus plan epoch.
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
//! stats snapshot read at compile time:
//! - The analyzer does not read the store at all (see
//!   `lora-analyzer/src/analyzer/state.rs`): whether a query is valid does
//!   not depend on which labels, types or keys exist.
//! - The planner and optimizer read only [`lora_store::GraphStats`]: counts
//!   per label / type, distinct-value estimates per counted key, and the
//!   online range / text / point / vector index scopes. They use them for
//!   cost alone (anchor choice, index rewrites, index-backed ordering);
//!   every index operator re-checks at execution time and falls back to a
//!   label scan when its index is gone, and the predicate it replaced is
//!   still evaluated. So any plan is correct against any graph; stats only
//!   decide how fast it runs.
//!
//! # Plan epochs
//!
//! Plans are keyed on a *plan epoch* rather than on the live store's write
//! epoch. The plan epoch advances only when the store's
//! [`plan_signature`](lora_store::GraphStorage::plan_signature) changes:
//! an online index appears or goes (CREATE / DROP INDEX, a constraint's
//! backing index), a label, type or counted property key appears or goes,
//! or a count crosses a power of two. Ordinary writes leave it alone, so a
//! repeated `CREATE` or `SET` reuses its plan instead of re-analyzing,
//! re-planning and rebuilding the stats on every call. The signature is
//! computed at most once per store write epoch, by the first query that
//! sees it.
//!
//! Because a cached plan may be reused while counts drift within their
//! buckets, a plan's operator choice can lag the exact stats by up to a
//! factor of two in any count. That changes cost, never results (see
//! above), with one visible effect: the row order of a query without
//! `ORDER BY` follows the chosen operators, as it always has.
//!
//! The parsed [`Document`] is a pure function of the query text, so it is
//! kept per query across plan epochs and a recompile skips the parser.
//! Only the [`EPOCHS_PER_QUERY`] newest plans are kept per query. Epochs
//! only move forward, so older plans can never hit again once a newer epoch
//! is live; keeping them would fill the cache with dead entries.
//!
//! # Eviction
//!
//! A small bounded LRU over query texts keeps the working set hot without
//! unbounded growth. On overflow we evict the query with the oldest access
//! stamp. The eviction scan is `O(capacity)` with `capacity = 256` and
//! allocates nothing; it is paid only when a new query text is admitted.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use lora_ast::Document;
use lora_compiler::CompiledQuery;

/// Default capacity, counted in cached plans. 256 entries comfortably
/// covers the working set of the realistic benchmark suites without
/// burning memory on plans that are allocated once and never reused.
const DEFAULT_CAPACITY: usize = 256;

/// Plans kept per query text. Two covers a reader that compiled against
/// the epoch just before a concurrent write landed.
const EPOCHS_PER_QUERY: usize = 2;

/// Content-addressed cache mapping `(query text, plan epoch)` →
/// compiled plan, plus query text → parsed document.
///
/// Cloning a `PlanCache` is meaningful: callers wrap it in `Arc` so all
/// `Database` clones (and the read/write phases of a single `execute`) share
/// the same map.
///
/// # Concurrency
///
/// Hits take only the read lock, so concurrent readers of cached plans
/// never wait on each other. The LRU stamp of a hit is an atomic that is
/// written only when it changes: the stamp is the miss clock, which moves
/// once per insert, so a hot query hit over and over writes nothing
/// shared beyond the plan's refcount.
pub(crate) struct PlanCache {
    inner: RwLock<Inner>,
    /// Logical clock for LRU stamps; advances once per insert. Hits stamp
    /// their slot with the current value, so slots hit since the last
    /// insert tie, and everything older sorts below them.
    clock: AtomicU64,
}

struct Inner {
    entries: HashMap<String, Slot>,
    /// Which plan epoch the newest resolved store write epoch maps to.
    resolved: Resolved,
    /// Maximum number of cached plans across all queries.
    capacity: usize,
    /// Number of cached plans across all queries.
    len: usize,
}

/// The store write epoch → plan epoch mapping, for the newest store
/// epoch seen.
#[derive(Default)]
struct Resolved {
    store_epoch: u64,
    /// Plan signature of that store epoch; `None` before the first.
    signature: Option<u64>,
    plan_epoch: u64,
}

struct Slot {
    /// Parsed form of the query text, reused across epochs. `None` when
    /// the plan was inserted without one (tests, callers that parsed
    /// elsewhere).
    document: Option<Arc<Document>>,
    /// At most [`EPOCHS_PER_QUERY`] plans, one per epoch.
    plans: Vec<Entry>,
    last_used: AtomicU64,
}

impl Slot {
    fn touch(&self, now: u64) {
        // Load first: a hit on a slot already stamped `now` stays a pure
        // read, so readers on different cores don't bounce its line.
        if self.last_used.load(Ordering::Relaxed) != now {
            self.last_used.store(now, Ordering::Relaxed);
        }
    }
}

struct Entry {
    epoch: u64,
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
            inner: RwLock::new(Inner {
                entries: HashMap::with_capacity(capacity),
                resolved: Resolved::default(),
                capacity,
                len: 0,
            }),
            clock: AtomicU64::new(0),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The plan epoch of store write epoch `store_epoch` if it is the
    /// newest resolved one, together with the cached plan for `query`
    /// under it. `(None, None)` means the caller must
    /// [`resolve`](Self::resolve) the store epoch first.
    pub(crate) fn lookup(
        &self,
        query: &str,
        store_epoch: u64,
    ) -> (Option<u64>, Option<Arc<CompiledQuery>>) {
        let guard = self.read();
        if guard.resolved.signature.is_none() || guard.resolved.store_epoch != store_epoch {
            return (None, None);
        }
        let epoch = guard.resolved.plan_epoch;
        (Some(epoch), self.get_locked(&guard, query, epoch))
    }

    /// Map store write epoch `store_epoch`, whose snapshot has plan
    /// signature `signature`, to a plan epoch. The plan epoch advances
    /// when a newer store epoch brings a new signature. `None` for an
    /// older snapshot whose signature differs from the newest one: plans
    /// for it are not cached.
    pub(crate) fn resolve(&self, store_epoch: u64, signature: u64) -> Option<u64> {
        let mut guard = self.write();
        let resolved = &mut guard.resolved;
        if resolved.signature == Some(signature) {
            resolved.store_epoch = resolved.store_epoch.max(store_epoch);
            return Some(resolved.plan_epoch);
        }
        if resolved.signature.is_some() && store_epoch <= resolved.store_epoch {
            return None;
        }
        resolved.store_epoch = store_epoch;
        resolved.signature = Some(signature);
        resolved.plan_epoch += 1;
        Some(resolved.plan_epoch)
    }

    /// Look up a cached plan for `query` under a plan epoch. Returns
    /// `None` on miss.
    ///
    /// On hit, the entry's last-used stamp is bumped so it survives
    /// eviction longer.
    pub(crate) fn get(&self, query: &str, epoch: u64) -> Option<Arc<CompiledQuery>> {
        self.get_locked(&self.read(), query, epoch)
    }

    fn get_locked(&self, guard: &Inner, query: &str, epoch: u64) -> Option<Arc<CompiledQuery>> {
        let slot = guard.entries.get(query)?;
        let plan = slot
            .plans
            .iter()
            .find(|entry| entry.epoch == epoch)?
            .plan
            .clone();
        slot.touch(self.clock.load(Ordering::Relaxed));
        Some(plan)
    }

    /// The cached parse of `query`, if any epoch of it has been compiled.
    pub(crate) fn document(&self, query: &str) -> Option<Arc<Document>> {
        self.read()
            .entries
            .get(query)
            .and_then(|slot| slot.document.clone())
    }

    /// Insert a freshly-compiled plan. If the cache is at capacity, evict
    /// the least recently used query.
    pub(crate) fn insert(&self, query: &str, epoch: u64, plan: Arc<CompiledQuery>) {
        self.insert_with_document(query, epoch, None, plan);
    }

    pub(crate) fn insert_with_document(
        &self,
        query: &str,
        epoch: u64,
        document: Option<Arc<Document>>,
        plan: Arc<CompiledQuery>,
    ) {
        let mut guard = self.write();
        if guard.capacity == 0 {
            return;
        }
        // Stamp with the current tick, then advance it: hits from now on
        // rank above this insert.
        let now = self.clock.fetch_add(1, Ordering::Relaxed);

        if let Some(slot) = guard.entries.get_mut(query) {
            *slot.last_used.get_mut() = now;
            if slot.document.is_none() {
                slot.document = document;
            }
            if let Some(entry) = slot.plans.iter_mut().find(|entry| entry.epoch == epoch) {
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
                    .map(|(idx, entry)| (idx, entry.epoch))
                    .min_by_key(|&(_, epoch)| epoch)
                    .expect("slot holds plans");
                if epoch > oldest_epoch {
                    slot.plans[oldest_idx] = Entry { epoch, plan };
                }
                return;
            }
            slot.plans.push(Entry { epoch, plan });
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
                plans: vec![Entry { epoch, plan }],
                last_used: AtomicU64::new(now),
            },
        );
        guard.len += 1;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.read().len
    }
}

/// Drop the least recently used query (all of its plans), skipping
/// `keep`. Returns `false` when nothing could be evicted.
fn evict_oldest(guard: &mut Inner, keep: Option<&str>) -> bool {
    let oldest = guard
        .entries
        .iter()
        .filter(|(query, _)| Some(query.as_str()) != keep)
        .min_by_key(|(_, slot)| slot.last_used.load(Ordering::Relaxed))
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
            null_params: Default::default(),
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
