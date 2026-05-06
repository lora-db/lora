//! Write-path lock plumbing for [`Database<S>`].
//!
//! Three pieces live here:
//!
//! * [`WriteGuard`] — a working copy + writer-mutex lease produced by
//!   [`Database::write_store`]. Callers mutate the inner `S` and then
//!   `publish()` to atomically swap the new state into the live store,
//!   or drop the guard to discard the changes. Reserved for the
//!   pessimistic non-`InMemoryGraph` fallback in `execute.rs`; the
//!   InMemoryGraph admin paths bypass it via the live-mutate
//!   fast path below.
//! * [`Database::write_store_deadline`] / [`Database::read_store_deadline`]
//!   — deadline-aware variants that participate in the cooperative
//!   query-timeout flow.
//! * [`Database::with_logged_write_guard`] — a closure runner that
//!   brackets the staged mutation with WAL `arm` / `commit` / `abort`
//!   and atomically publishes on success.
//! * [`Database::with_logged_store_mut`] — the InMemoryGraph fast
//!   path: mutates the live `Arc<S>` in place via `Arc::make_mut`
//!   and only commits buffered events to the WAL on success. Skips
//!   the O(N+E) snapshot clone that `write_store()` pays.

use std::any::{Any, TypeId};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lora_executor::ExecutorError;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, MutationEvent, MutationRecorder};
use lora_wal::WalRecorder;

use crate::database::Database;
use crate::transaction::BufferingRecorder;
use crate::wal::write_scope::{ensure_wal_query_can_start, WalAbortPolicy, WalWriteScope};

use super::replay::install_recorder_if_inmemory;

/// Working copy + writer-mutex lease produced by
/// [`Database::write_store`]. The caller mutates the inner `S`, then
/// calls [`WriteGuard::publish`] to atomically swap the new state into
/// the `ArcSwap`. Dropping without `publish` discards the staged copy
/// (rollback semantics) and releases the writer lock, leaving the
/// authoritative store unchanged.
pub(crate) struct WriteGuard<'db, S> {
    db: &'db Database<S>,
    /// Held for the lifetime of the guard so commits are serialized
    /// (and so the WAL records appear in commit order).
    _writer_lock: MutexGuard<'db, ()>,
    /// `Some` until `publish` consumes the staged graph, `None` after.
    /// Drop on `None` is a no-op; drop on `Some` discards the staged
    /// changes.
    staged: Option<S>,
}

impl<S> Deref for WriteGuard<'_, S> {
    type Target = S;
    fn deref(&self) -> &S {
        self.staged
            .as_ref()
            .expect("staged graph already published or taken")
    }
}

impl<S> DerefMut for WriteGuard<'_, S> {
    fn deref_mut(&mut self) -> &mut S {
        self.staged
            .as_mut()
            .expect("staged graph already published or taken")
    }
}

impl<S> WriteGuard<'_, S>
where
    S: Send + Sync + 'static,
{
    /// Atomically replace the live store with the staged graph. After
    /// this returns, subsequent reads see the new state.
    pub(crate) fn publish(mut self) {
        if let Some(staged) = self.staged.take() {
            self.db.store.store(Arc::new(staged));
        }
    }
}

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Take the writer lease and clone the current snapshot into a
    /// staged working copy. The caller mutates the staged graph and
    /// either calls `publish()` to install it (atomically swapping the
    /// `ArcSwap`) or drops the guard to discard the changes.
    pub(crate) fn write_store(&self) -> WriteGuard<'_, S> {
        let lock = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let snapshot = self.store.load_full();
        let staged: S = (*snapshot).clone();
        WriteGuard {
            db: self,
            _writer_lock: lock,
            staged: Some(staged),
        }
    }

    pub(crate) fn read_store_deadline(&self, _deadline: Option<Instant>) -> Result<Arc<S>> {
        // Reads are lock-free; the deadline only mattered when readers
        // could be starved by an in-flight writer holding the RwLock.
        // ArcSwap reads are wait-free, so we always succeed immediately.
        Ok(self.store.load_full())
    }

    pub(crate) fn write_store_deadline(
        &self,
        deadline: Option<Instant>,
    ) -> Result<WriteGuard<'_, S>> {
        let Some(deadline) = deadline else {
            return Ok(self.write_store());
        };

        loop {
            match self.writer.try_lock() {
                Ok(lock) => {
                    let snapshot = self.store.load_full();
                    let staged: S = (*snapshot).clone();
                    return Ok(WriteGuard {
                        db: self,
                        _writer_lock: lock,
                        staged: Some(staged),
                    });
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    let lock = poisoned.into_inner();
                    let snapshot = self.store.load_full();
                    let staged: S = (*snapshot).clone();
                    return Ok(WriteGuard {
                        db: self,
                        _writer_lock: lock,
                        staged: Some(staged),
                    });
                }
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(ExecutorError::QueryTimeout.into());
                }
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    pub(crate) fn observe_snapshot_commit_if_needed(
        &self,
        store: &S,
        recorder: &WalRecorder,
    ) -> Result<()> {
        let Some(snapshots) = &self.snapshots else {
            return Ok(());
        };
        let graph = (store as &dyn Any)
            .downcast_ref::<InMemoryGraph>()
            .ok_or_else(|| anyhow!("managed snapshots require InMemoryGraph storage"))?;
        snapshots.observe_commit(graph, recorder)?;
        Ok(())
    }

    /// Run `f` against the live graph, mutating in place, and commit
    /// the buffered mutation events to the WAL on success.
    ///
    /// For `InMemoryGraph` (the default backend) this is the fast
    /// path: it skips the O(N+E) snapshot clone that `write_store()`
    /// pays, going through the same `LiveStore::write()` +
    /// `Arc::make_mut` flow that the optimistic auto-commit query
    /// path uses. Mutation events buffer into a tx-local `Vec` while
    /// the closure runs; on success they're appended to the WAL as a
    /// single transaction, on failure they're discarded so the WAL
    /// stays consistent. The trade-off matches the OCC fast path: a
    /// failure mid-closure can leave the in-memory graph partially
    /// mutated, but durable state stays consistent and recovery from
    /// snapshot+WAL replays only committed transactions.
    ///
    /// For non-`InMemoryGraph` backends the pessimistic clone-then-
    /// publish path is preserved: those backends don't expose the
    /// `set_mutation_recorder` hook, so a buffered-event redirect
    /// isn't possible.
    pub(crate) fn with_logged_store_mut<R>(
        &self,
        f: impl FnOnce(&mut S) -> Result<R>,
    ) -> Result<R> {
        if TypeId::of::<S>() == TypeId::of::<InMemoryGraph>() {
            return self.with_live_store_mut(f);
        }
        let guard = self.write_store();
        self.with_logged_write_guard(guard, WalAbortPolicy::AbortOnly, f)
    }

    fn with_live_store_mut<R>(&self, f: impl FnOnce(&mut S) -> Result<R>) -> Result<R> {
        let _commit_lock = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let buffer = Arc::new(Mutex::new(Vec::<MutationEvent>::new()));
        let buffering_rec: Arc<dyn MutationRecorder> =
            Arc::new(BufferingRecorder::new(buffer.clone()));

        let mut handle = self.store.write();

        let exec_result = {
            let staged = handle.as_mut();
            install_recorder_if_inmemory(staged, Some(buffering_rec));
            let r = f(staged);
            install_recorder_if_inmemory(staged, None);
            r
        };

        let events: Vec<MutationEvent> = std::mem::take(&mut buffer.lock().unwrap());

        let mut wrote_commit = false;
        if let Some(rec) = self.wal.as_ref() {
            if exec_result.is_ok() && !events.is_empty() {
                ensure_wal_query_can_start(rec)?;
                wrote_commit = rec.commit_events(events)?.wrote();
            }
            // Reinstall the durable recorder so the live graph keeps
            // observing future mutations after this scope exits.
            let staged = handle.as_mut();
            install_recorder_if_inmemory(staged, Some(rec.clone() as Arc<dyn MutationRecorder>));
        }

        if wrote_commit {
            if let Some(rec) = self.wal.as_ref() {
                let live = handle.snapshot();
                self.observe_snapshot_commit_if_needed(&*live, rec)?;
            }
        }

        exec_result
    }

    /// Run `f` against the staged graph inside a WAL transaction. On
    /// success, atomically publishes the staged graph to the live
    /// `ArcSwap`; on error, the staged copy is dropped (no observable
    /// state change) and the WAL is aborted per `abort_policy`.
    pub(crate) fn with_logged_write_guard<R>(
        &self,
        mut guard: WriteGuard<'_, S>,
        abort_policy: WalAbortPolicy,
        f: impl FnOnce(&mut S) -> Result<R>,
    ) -> Result<R> {
        let Some(rec) = self.wal.clone() else {
            // No WAL: just run the closure, publish on success.
            let result = f(&mut *guard);
            if result.is_ok() {
                guard.publish();
            }
            return result;
        };

        // Install the durable recorder on the staged graph so the
        // executor's mutations fire into it. `InMemoryGraph::clone`
        // intentionally drops the recorder, so the staged copy starts
        // without one.
        install_recorder_if_inmemory(&mut *guard, Some(rec.clone() as Arc<dyn MutationRecorder>));

        let scope = WalWriteScope::arm(&rec, abort_policy)?;
        let result = f(&mut *guard);
        let wrote_commit = scope.finish(&result)?;
        if wrote_commit {
            self.observe_snapshot_commit_if_needed(&*guard, &rec)?;
        }

        // Strip the per-mutation recorder before publish — the new live
        // store carries the durable recorder reinstalled below.
        install_recorder_if_inmemory(&mut *guard, None);

        if result.is_ok() {
            // Reinstall the durable recorder on staged so the new live
            // graph keeps observing mutations after the swap.
            install_recorder_if_inmemory(
                &mut *guard,
                Some(rec.clone() as Arc<dyn MutationRecorder>),
            );
            guard.publish();
        }
        result
    }
}
