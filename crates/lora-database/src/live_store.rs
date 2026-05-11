//! Authoritative graph storage with in-place writer mutation.
//!
//! Wraps `Arc<S>` in a `RwLock` so writers can call `Arc::make_mut`
//! against the live `Arc` directly. When no in-flight reader holds a
//! cloned `Arc<S>`, the live `Arc`'s strong count is 1 and `make_mut`
//! returns `&mut S` *without* cloning the graph — restoring the v0.6
//! single-writer cost for `CREATE` / `SET` / `DELETE` against large
//! graphs.
//!
//! Why not `ArcSwap`: `ArcSwap` always holds an internal `Arc<S>`, so
//! every `load_full()` produces a second strong reference. With the
//! writer mutex held, we'd still see strong count >= 2 from
//! `load_full`, forcing `Arc::make_mut` to deep-clone the graph on every
//! mutating query — that's the v0.6→v0.8 write regression. `RwLock<Arc<S>>`
//! lets the writer take exclusive access via the write guard without
//! bumping the inner refcount.
//!
//! Tradeoff: `load_full()` now goes through a brief read-lock acquire
//! (one atomic CAS in the uncontended case) instead of an `ArcSwap`
//! atomic load. For embedded use the difference is in the noise; the
//! win on the write side is many microseconds per mutating query at
//! 10k+ node graphs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

/// Authoritative graph state. Reads obtain an independent `Arc<S>`
/// snapshot; writes obtain exclusive in-place access.
///
/// The `epoch` field is a monotonic write counter: every published
/// mutation (via [`Self::store`] or a dropped [`WriteHandle`]) bumps it.
/// Read-mostly callers (the plan cache) use it as a cheap fingerprint
/// instead of recomputing the full [`GraphStats`] hash on every execute.
pub(crate) struct LiveStore<S> {
    inner: RwLock<Arc<S>>,
    epoch: AtomicU64,
}

impl<S> LiveStore<S> {
    pub(crate) fn new(value: Arc<S>) -> Self {
        Self {
            inner: RwLock::new(value),
            epoch: AtomicU64::new(1),
        }
    }

    /// Snapshot the current state. The returned `Arc<S>` is independent
    /// of the lock — callers may hold it across query execution and the
    /// snapshot stays consistent (writers won't mutate it; they'll
    /// `Arc::make_mut`-clone if any reader Arc is alive).
    pub(crate) fn load_full(&self) -> Arc<S> {
        Arc::clone(&*self.inner.read().unwrap_or_else(|p| p.into_inner()))
    }

    /// Snapshot the current state together with the epoch that protects it.
    /// The epoch is read while holding the same read lock as the Arc clone, so
    /// callers can safely use it as a cache key for work derived from this
    /// exact snapshot.
    pub(crate) fn load_full_with_epoch(&self) -> (Arc<S>, u64) {
        let guard = self.inner.read().unwrap_or_else(|p| p.into_inner());
        let snapshot = Arc::clone(&*guard);
        let epoch = self.epoch.load(Ordering::Acquire);
        (snapshot, epoch)
    }

    /// Replace the inner `Arc<S>` wholesale. Used at publish points
    /// that already produced a finished new state — snapshot restore,
    /// transaction commit's merged state, etc.
    pub(crate) fn store(&self, value: Arc<S>) {
        let mut guard = self.inner.write().unwrap_or_else(|p| p.into_inner());
        *guard = value;
        self.epoch.fetch_add(1, Ordering::Release);
    }
}

impl<S: Clone> LiveStore<S> {
    /// Take the writer guard and return mutable access to the inner
    /// `S` via `Arc::make_mut`. When the inner refcount is 1 (no
    /// reader Arc clones in flight) this is a zero-copy `&mut`; when
    /// readers hold snapshots it clones once and the readers keep
    /// observing the pre-mutation state.
    ///
    /// The caller serializes with other writers via [`Database::writer`].
    /// The returned guard, when dropped, releases the lock and the
    /// post-mutation state becomes the live state.
    pub(crate) fn write(&self) -> WriteHandle<'_, S> {
        let guard = self.inner.write().unwrap_or_else(|p| p.into_inner());
        WriteHandle {
            guard,
            epoch: &self.epoch,
        }
    }
}

/// Writer's exclusive access to the live graph. Mutations happen
/// in-place via `Arc::make_mut`; on drop, the post-mutation state is
/// already live, so there's no explicit "publish" step.
pub(crate) struct WriteHandle<'a, S> {
    guard: RwLockWriteGuard<'a, Arc<S>>,
    epoch: &'a AtomicU64,
}

impl<S: Clone> WriteHandle<'_, S> {
    /// `&mut` to the live state. First call after a snapshot reader
    /// took an `Arc<S>` clone may pay one graph clone; subsequent calls
    /// while this handle is alive are free.
    pub(crate) fn as_mut(&mut self) -> &mut S {
        Arc::make_mut(&mut *self.guard)
    }

    /// Snapshot the current Arc without releasing the write lock.
    /// Used by paths that need to feed the post-mutation state to
    /// `observe_commit` while the writer mutex is still held.
    pub(crate) fn snapshot(&self) -> Arc<S> {
        Arc::clone(&*self.guard)
    }
}

impl<S> Drop for WriteHandle<'_, S> {
    /// Every write handle that goes out of scope bumps the epoch so the
    /// plan cache reliably refreshes after any in-place mutation. The
    /// occasional unnecessary bump (handle acquired, no mutation) only
    /// invalidates plan cache entries — never a correctness issue, and
    /// in practice `write()` is only acquired on confirmed write paths.
    fn drop(&mut self) {
        self.epoch.fetch_add(1, Ordering::Release);
    }
}
