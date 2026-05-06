//! Write-path lock plumbing for [`Database<S>`].
//!
//! The pieces in here form one shared mutating shape and the two
//! published entry points that wrap it:
//!
//! * [`Database::run_with_durable_recorder`] — single canonical write
//!   shape. Acquires the writer mutex, takes the [`LiveStore`] write
//!   handle, arms the durable recorder when a WAL is attached,
//!   reinstalls the recorder on the post-CoW staged graph, runs the
//!   caller's closure, and either commits via [`Wal::commit_tx`] (and
//!   triggers managed-snapshot accounting) or aborts. Used by both
//!   the auto-commit OCC fast path and the admin live-mutate path so
//!   the WAL semantics live in exactly one place.
//! * [`Database::with_logged_store_mut`] — admin entry. Dispatches
//!   InMemoryGraph traffic to `run_with_durable_recorder`; falls back
//!   to the pessimistic clone-then-publish path for any other backend.
//! * [`Database::with_logged_write_guard`] — pessimistic path. Stages
//!   a clone, runs the closure under the WAL scope, and atomically
//!   publishes on success (or aborts per `WalAbortPolicy` on failure).
//!   Reserved for backends that don't expose the
//!   `set_mutation_recorder` hook.
//! * [`WriteGuard`] — working copy + writer-mutex lease produced by
//!   [`Database::write_store`]. Mutate, then `publish()` to swap the
//!   new state into the live store, or drop to discard.
//! * [`Database::write_store_deadline`] / [`Database::read_store_deadline`]
//!   — deadline-aware variants that participate in the cooperative
//!   query-timeout flow.
//!
//! Trade-off shared by `run_with_durable_recorder` and the
//! `WalAbortPolicy::PoisonIfMutated` flavour of `with_logged_write_guard`:
//! a query that fails mid-execution can leave the live in-memory graph
//! partially mutated. The WAL is aborted (no commit record is written),
//! so durable recovery from snapshot+WAL stays consistent.

use std::any::{Any, TypeId};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use lora_executor::ExecutorError;
use lora_store::{GraphStorage, GraphStorageMut, InMemoryGraph, MutationRecorder};
use lora_wal::WalRecorder;

use crate::database::Database;
use crate::wal::write_scope::{ensure_wal_query_can_start, WalAbortPolicy, WalWriteScope};

use super::replay::install_recorder_if_inmemory;

/// Working copy + writer-mutex lease produced by
/// [`Database::write_store`]. The caller mutates the inner `S`, then
/// calls [`WriteGuard::publish`] to atomically swap the new state into
/// the live store. Dropping without `publish` discards the staged copy
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
    /// either calls `publish()` to install it (atomically replacing the live
    /// store) or drops the guard to discard the changes.
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
        // Reads only take the LiveStore read lock long enough to clone the
        // current Arc. Timeout-aware read lock acquisition can be added here
        // if reader starvation ever shows up in practice.
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

    /// Admin entry into the live-mutate write path.
    ///
    /// For `InMemoryGraph` (the default backend) this delegates to
    /// [`Self::run_with_durable_recorder`], so the same shape governs
    /// both query auto-commit and the direct admin / `graph_api`
    /// mutators. For other backends — which don't expose
    /// `set_mutation_recorder` and therefore can't redirect buffered
    /// events — we fall back to the pessimistic clone-then-publish
    /// path with `WalAbortPolicy::AbortOnly`.
    pub(crate) fn with_logged_store_mut<R>(
        &self,
        f: impl FnOnce(&mut S) -> Result<R>,
    ) -> Result<R> {
        if TypeId::of::<S>() == TypeId::of::<InMemoryGraph>() {
            return self.run_with_durable_recorder(f);
        }
        let guard = self.write_store();
        self.with_logged_write_guard(guard, WalAbortPolicy::AbortOnly, f)
    }

    /// Canonical mutating shape for `InMemoryGraph` writes. Used by
    /// both the auto-commit OCC fast path and the admin live-mutate
    /// path so the WAL bracketing lives in exactly one place.
    ///
    /// Sequence:
    ///
    /// 1. Take the writer mutex (serialises commit ordering with
    ///    other writers).
    /// 2. If a WAL is attached, check it isn't poisoned and `arm` the
    ///    durable recorder so subsequent `MutationEvent`s buffer
    ///    inside it.
    /// 3. Take the `LiveStore` write handle and `Arc::make_mut` the
    ///    live graph in place. The CoW clone (when any reader holds a
    ///    snapshot) drops the recorder, so we reinstall it on the
    ///    staged copy before running `f`.
    /// 4. Run `f`. On `Err`: `abort` the recorder (buffered events
    ///    are discarded; nothing reaches the durable log). On `Ok`:
    ///    `commit` the recorder, which routes through
    ///    [`Wal::commit_tx`] for the begin/batch/commit/fsync triple,
    ///    and trigger managed-snapshot accounting if a commit record
    ///    was actually written.
    pub(crate) fn run_with_durable_recorder<R>(
        &self,
        f: impl FnOnce(&mut S) -> Result<R>,
    ) -> Result<R> {
        let _commit_lock = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(rec) = self.wal.as_ref() {
            ensure_wal_query_can_start(rec)?;
            rec.arm()?;
        }

        let mut handle = self.store.write();

        let result = {
            let staged = handle.as_mut();
            if let Some(rec) = self.wal.as_ref() {
                install_recorder_if_inmemory(
                    staged,
                    Some(rec.clone() as Arc<dyn MutationRecorder>),
                );
            }
            f(staged)
        };

        if let Some(rec) = self.wal.as_ref() {
            match &result {
                Ok(_) => {
                    if rec.commit()?.wrote() {
                        let live = handle.snapshot();
                        self.observe_snapshot_commit_if_needed(&*live, rec)?;
                    }
                }
                Err(_) => {
                    let _ = rec.abort();
                }
            }
        }

        result
    }

    /// Run `f` against the staged graph inside a WAL transaction. On
    /// success, atomically publishes the staged graph to the live
    /// store; on error, the staged copy is dropped (no observable
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
