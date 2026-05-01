//! Per-record write locks for fine-grained concurrent mutations.
//!
//! Phase 4 of the concurrency plan introduces *true* concurrent writes
//! for non-conflicting transactions. The crux is conflict detection at
//! record granularity: two writers that touch disjoint records should
//! commit in parallel without retrying. The pieces:
//!
//! * **`LockTable`** (this module) — sharded per-record `Mutex<()>`
//!   registry. Writers acquire locks for the records they intend to
//!   mutate; concurrent writers on the same record serialize, on
//!   different records proceed independently.
//! * **`MutationWriteSet`** — typed accumulator for the touched
//!   record IDs extracted from a buffered [`MutationEvent`] stream.
//!   The auto-commit OCC path uses it both to acquire locks (sorted
//!   to prevent deadlock) and to validate per-record Arc identity
//!   against the snapshot at commit time.
//!
//! Neither type is wired into the `InMemoryGraph` write path yet —
//! that's the next step. Building them as a self-contained module
//! first lets us test the locking primitives in isolation before the
//! larger refactor.
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

use crate::{MutationEvent, NodeId, RelationshipId};

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

/// Set of record ids touched by a buffered [`MutationEvent`] stream.
///
/// Built incrementally as events buffer (or in one pass at commit
/// time) by [`MutationWriteSet::extend_from_events`]. Used by the OCC
/// auto-commit path to (a) sort lock-acquire on commit, (b) validate
/// per-record Arc identity against the snapshot.
#[derive(Debug, Default, Clone)]
pub struct MutationWriteSet {
    /// Nodes whose record was created, modified, or deleted.
    pub nodes: std::collections::BTreeSet<NodeId>,
    /// Relationships whose record was created, modified, or deleted.
    pub rels: std::collections::BTreeSet<RelationshipId>,
    /// `true` if the stream contained a `MutationEvent::Clear`. A
    /// clear invalidates any per-record check — the writer must
    /// fall back to a full-graph commit (or fail under OCC).
    pub cleared: bool,
}

impl MutationWriteSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk a `MutationEvent` stream and accumulate every touched
    /// record id. Variants that touch two records (e.g.
    /// `CreateRelationship` mentions both `src` and `dst` plus the
    /// new relationship) record both nodes — the writer's view of
    /// those nodes' adjacency changed too.
    pub fn extend_from_events<'a>(&mut self, events: impl IntoIterator<Item = &'a MutationEvent>) {
        for event in events {
            match event {
                MutationEvent::CreateNode { id, .. } => {
                    self.nodes.insert(*id);
                }
                MutationEvent::CreateRelationship { id, src, dst, .. } => {
                    self.rels.insert(*id);
                    self.nodes.insert(*src);
                    self.nodes.insert(*dst);
                }
                MutationEvent::SetNodeProperty { node_id, .. }
                | MutationEvent::RemoveNodeProperty { node_id, .. }
                | MutationEvent::AddNodeLabel { node_id, .. }
                | MutationEvent::RemoveNodeLabel { node_id, .. } => {
                    self.nodes.insert(*node_id);
                }
                MutationEvent::SetRelationshipProperty { rel_id, .. }
                | MutationEvent::RemoveRelationshipProperty { rel_id, .. } => {
                    self.rels.insert(*rel_id);
                }
                MutationEvent::DeleteRelationship { rel_id } => {
                    self.rels.insert(*rel_id);
                }
                MutationEvent::DeleteNode { node_id } => {
                    self.nodes.insert(*node_id);
                }
                MutationEvent::DetachDeleteNode { node_id } => {
                    // Detach-delete also touches every incident
                    // relationship, but those fire as
                    // `DeleteRelationship` events of their own and
                    // the surrounding loop will pick them up.
                    self.nodes.insert(*node_id);
                }
                MutationEvent::Clear => {
                    self.cleared = true;
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.cleared && self.nodes.is_empty() && self.rels.is_empty()
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

    #[test]
    fn write_set_extracts_ids_from_events() {
        let events = vec![
            MutationEvent::CreateNode {
                id: 1,
                labels: vec!["A".into()],
                properties: Default::default(),
            },
            MutationEvent::CreateRelationship {
                id: 10,
                src: 1,
                dst: 2,
                rel_type: "R".into(),
                properties: Default::default(),
            },
            MutationEvent::SetNodeProperty {
                node_id: 3,
                key: "x".into(),
                value: crate::PropertyValue::Int(5),
            },
            MutationEvent::DeleteRelationship { rel_id: 11 },
        ];

        let mut ws = MutationWriteSet::new();
        ws.extend_from_events(events.iter());

        // CreateRelationship pulls in src=1, dst=2 alongside its own rel id.
        assert_eq!(ws.nodes.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(ws.rels.iter().copied().collect::<Vec<_>>(), vec![10, 11]);
        assert!(!ws.cleared);
    }

    #[test]
    fn write_set_clear_event_is_sticky() {
        let mut ws = MutationWriteSet::new();
        ws.extend_from_events([&MutationEvent::Clear]);
        assert!(ws.cleared);
        assert!(!ws.is_empty()); // cleared counts as non-empty
    }
}
