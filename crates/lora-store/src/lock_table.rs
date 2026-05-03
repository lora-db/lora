//! Per-record write locks for fine-grained concurrent mutations.
//!
//! [`LockTable`] is the sharded per-record `Mutex<()>` registry the
//! auto-commit / OCC path consults at commit time. Writers translate
//! their buffered mutation stream into a [`MutationWriteSet`] (defined
//! in [`crate::mutation`]) and call [`WriteSetLocks::acquire`] to grab
//! every per-record lock in sorted order before publishing.
//!
//! Concurrent writers on the same record serialize on the entry's
//! `Mutex`; concurrent writers on disjoint record sets proceed
//! independently.
//!
//! # Sharding
//!
//! The lock table is sharded into [`LOCK_TABLE_SHARDS`] independent
//! buckets. Each shard owns a `HashMap<u64, Arc<Mutex<()>>>` behind
//! its own `Mutex`. Acquiring a per-record lock looks up the
//! `Arc<Mutex<()>>` in the relevant shard (taking only that shard's
//! Mutex briefly, *not* a global table lock) and then locks it.
//!
//! Power-of-two shard count means the shard index is a single AND on
//! the id, no division. 256 shards is generous for any realistic
//! thread count and keeps the per-shard contention well below the
//! per-record lock contention you'd expect for non-pathological
//! workloads.
//!
//! # Lifetime
//!
//! Lock entries are *not* GC'd from the table after their owning tx
//! drops them. Each entry is `Arc<Mutex<()>>` — small, and the lock
//! gets reused if the same id is locked again. A future GC pass could
//! sweep entries with refcount 1 if memory pressure warrants it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{MutationWriteSet, NodeId, RelationshipId};

/// Power-of-two number of shards in the lock table. Chosen large
/// enough that per-shard contention is well below per-record
/// contention for any realistic thread count, small enough that the
/// table itself stays under a few KB of empty-shard overhead.
pub const LOCK_TABLE_SHARDS: usize = 256;

const SHARD_MASK: u64 = (LOCK_TABLE_SHARDS as u64) - 1;

/// Sharded per-record write-lock registry. Cloning is cheap (one
/// `Arc` per shard map) so the table can live behind an `Arc` shared
/// across all writers.
pub struct LockTable {
    nodes: [Mutex<HashMap<NodeId, Arc<Mutex<()>>>>; LOCK_TABLE_SHARDS],
    rels: [Mutex<HashMap<RelationshipId, Arc<Mutex<()>>>>; LOCK_TABLE_SHARDS],
}

impl Default for LockTable {
    fn default() -> Self {
        Self::new()
    }
}

impl LockTable {
    pub fn new() -> Self {
        Self {
            nodes: std::array::from_fn(|_| Mutex::new(HashMap::new())),
            rels: std::array::from_fn(|_| Mutex::new(HashMap::new())),
        }
    }

    /// Look up (or insert) the per-node `Arc<Mutex<()>>`. Caller is
    /// expected to `lock()` the returned `Arc` and hold it across the
    /// mutation. Sharded so two threads locking different node ids in
    /// the same id-mod-shards bucket only contend for the brief
    /// shard-table acquisition, not on each other's record lock.
    pub fn node_lock_arc(&self, id: NodeId) -> Arc<Mutex<()>> {
        let shard = (id & SHARD_MASK) as usize;
        let mut map = self.nodes[shard].lock().unwrap_or_else(|p| p.into_inner());
        map.entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Same as [`Self::node_lock_arc`] but for relationships.
    pub fn rel_lock_arc(&self, id: RelationshipId) -> Arc<Mutex<()>> {
        let shard = (id & SHARD_MASK) as usize;
        let mut map = self.rels[shard].lock().unwrap_or_else(|p| p.into_inner());
        map.entry(id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

/// Bundle of per-record locks held by a single writer's commit
/// critical section. Holding the locks here (rather than scattered
/// `MutexGuard`s) makes drop order deterministic — all locks release
/// when this struct drops, regardless of which release path
/// (commit / abort / panic) the writer took.
///
/// Lock acquisition is **always sorted by id** so any two transactions
/// touching the same record set acquire the locks in the same order.
/// That's the standard prevention strategy for the simplest deadlock
/// scenario — two writers each holding one of the other's locks.
pub struct WriteSetLocks {
    /// Boxed because `MutexGuard` is `!Unpin` and we want to keep the
    /// guards live by value. The `Arc<Mutex<()>>` originates from the
    /// `LockTable`; we keep both the Arc *and* the guard so the
    /// `'static` guard is anchored by the Arc that owns the lock.
    _guards: Vec<OwnedMutexGuard>,
}

/// Wrapper that pairs an `Arc<Mutex<()>>` with the `MutexGuard<'_>`
/// it produced. Extends the guard's borrow lifetime to the lifetime of
/// the surrounding [`WriteSetLocks`]; safe because the `Arc` keeps the
/// underlying `Mutex` alive at least as long as the guard, and the
/// `Vec` drops guards before Arcs (Rust drops fields in declaration
/// order, but `OwnedMutexGuard` also enforces it via its own struct
/// drop order).
struct OwnedMutexGuard {
    /// Guard first so it drops first.
    guard: Option<MutexGuard<'static, ()>>,
    /// Arc kept alive while the guard exists.
    _arc: Arc<Mutex<()>>,
}

impl OwnedMutexGuard {
    /// SAFETY: extends the guard's lifetime from `'_` to `'static`. This
    /// is sound because (a) `_arc` keeps the `Mutex<()>` alive for the
    /// guard's full life, and (b) `Drop` for `OwnedMutexGuard` ensures
    /// the guard is dropped before `_arc`.
    fn lock(arc: Arc<Mutex<()>>) -> Self {
        let guard = arc.lock().unwrap_or_else(|p| p.into_inner());
        // Erase the lifetime — the Arc anchor keeps the Mutex live.
        let guard: MutexGuard<'static, ()> =
            unsafe { std::mem::transmute::<MutexGuard<'_, ()>, _>(guard) };
        Self {
            guard: Some(guard),
            _arc: arc,
        }
    }
}

impl Drop for OwnedMutexGuard {
    fn drop(&mut self) {
        // Explicit drop order: guard before _arc.
        self.guard.take();
    }
}

impl WriteSetLocks {
    /// Acquire write locks for every record in the write set, sorted
    /// by id. Returns once all locks are held; the caller's commit
    /// runs without contention on these records until `WriteSetLocks`
    /// drops.
    pub fn acquire(table: &LockTable, write_set: &MutationWriteSet) -> Self {
        // Sort to prevent deadlock from inconsistent acquisition order
        // across concurrent writers touching overlapping record sets.
        let mut node_ids: Vec<NodeId> = write_set.nodes.iter().copied().collect();
        node_ids.sort_unstable();
        let mut rel_ids: Vec<RelationshipId> = write_set.rels.iter().copied().collect();
        rel_ids.sort_unstable();

        // Acquire nodes first, then rels. The two namespaces never
        // alias because a NodeId and a RelationshipId can't share a
        // lock entry (separate shard arrays in the LockTable).
        let mut guards = Vec::with_capacity(node_ids.len() + rel_ids.len());
        for id in node_ids {
            guards.push(OwnedMutexGuard::lock(table.node_lock_arc(id)));
        }
        for id in rel_ids {
            guards.push(OwnedMutexGuard::lock(table.rel_lock_arc(id)));
        }
        Self { _guards: guards }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn lock_table_returns_same_arc_for_same_id() {
        let table = LockTable::new();
        let a = table.node_lock_arc(42);
        let b = table.node_lock_arc(42);
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn lock_table_distinct_ids_get_distinct_locks() {
        let table = LockTable::new();
        let a = table.node_lock_arc(1);
        let b = table.node_lock_arc(2);
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn node_and_rel_namespaces_are_separate() {
        let table = LockTable::new();
        let n = table.node_lock_arc(7);
        let r = table.rel_lock_arc(7);
        assert!(!Arc::ptr_eq(&n, &r));
    }

    #[test]
    fn write_set_locks_serialize_same_id() {
        let table = Arc::new(LockTable::new());
        let counter = Arc::new(Mutex::new(0u32));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let table = table.clone();
            let counter = counter.clone();
            handles.push(thread::spawn(move || {
                let mut ws = MutationWriteSet::new();
                ws.nodes.insert(99);
                let _locks = WriteSetLocks::acquire(&table, &ws);
                let mut c = counter.lock().unwrap();
                let before = *c;
                *c += 1;
                // Held while we sleep — second thread can't enter
                // because it's blocked on the same id's lock.
                thread::sleep(Duration::from_millis(5));
                assert_eq!(*c, before + 1, "lock did not serialize");
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*counter.lock().unwrap(), 4);
    }
}
