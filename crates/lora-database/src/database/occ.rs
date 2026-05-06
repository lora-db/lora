//! Single-writer auto-commit write path for `InMemoryGraph`.
//!
//! Earlier revisions cloned the entire snapshot into a "staged" graph
//! per mutating query (see `git log` for the OCC + `ArcSwap` design).
//! That made every CREATE / SET / DELETE pay an O(N+E) graph clone,
//! which dominated wall-clock for write-heavy benches against large
//! graphs.
//!
//! The current implementation acquires the writer mutex, takes the
//! `RwLock` write guard inside [`LiveStore`], and uses
//! `Arc::make_mut` against the live `Arc<S>`. When no in-flight reader
//! holds a snapshot Arc clone, `make_mut` returns `&mut S` *without*
//! cloning the graph — restoring v0.6's mutate-in-place cost.
//! Concurrent readers, when present, force a single CoW clone and keep
//! observing their pre-mutation snapshot via the Arc they already
//! hold.
//!
//! Trade-off vs. the prior OCC clone-then-publish: a query that
//! fails mid-execution leaves the live graph partially mutated. The
//! WAL records are not appended (the buffer is drained without
//! commit), so durable state stays consistent and recovery from
//! snapshot+WAL is unaffected. The same caveat applies to the
//! pessimistic `with_logged_write_guard` path with
//! `WalAbortPolicy::PoisonIfMutated`.

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use lora_compiler::CompiledQuery;
use lora_executor::{LoraValue, MutableExecutionContext, MutableExecutor, Row};
use lora_store::{GraphStorage, GraphStorageMut, MutationEvent, MutationRecorder};

use crate::database::Database;
use crate::transaction::BufferingRecorder;
use crate::wal::write_scope::ensure_wal_query_can_start;

use super::replay::install_recorder_if_inmemory;

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Single-writer auto-commit write path. Takes the writer mutex,
    /// mutates the live store in place via `Arc::make_mut`, and (for
    /// WAL-backed databases) appends a single batched commit record.
    pub(crate) fn execute_mutating_optimistic(
        &self,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        compiled: &Arc<CompiledQuery>,
    ) -> Result<Vec<Row>> {
        // Serialize commit ordering so WAL records appear in the same
        // order writers publish to the live store.
        let _commit_lock = self
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Buffer mutation events tx-locally. We replay them into the
        // durable WAL only after the executor returns success — a
        // failed query leaves no on-disk trace.
        let buffer = Arc::new(Mutex::new(Vec::<MutationEvent>::new()));
        let buffering_rec: Arc<dyn MutationRecorder> =
            Arc::new(BufferingRecorder::new(buffer.clone()));

        let mut handle = self.store.write();

        // Install the buffering recorder *after* `make_mut` so the
        // (rare) CoW clone that `make_mut` performs when concurrent
        // readers hold a snapshot doesn't drop the recorder we just
        // installed — `InMemoryGraph::clone` intentionally drops the
        // recorder on the cloned copy.
        let exec_result = {
            let staged = handle.as_mut();
            install_recorder_if_inmemory(staged, Some(buffering_rec));
            let mut executor = MutableExecutor::with_deadline(
                MutableExecutionContext {
                    storage: staged,
                    params,
                },
                deadline,
            );
            let r = executor.execute_compiled_rows(compiled);
            install_recorder_if_inmemory(staged, None);
            r
        };

        let rows = match exec_result {
            Ok(rows) => rows,
            Err(e) => return Err(anyhow::Error::from(e)),
        };

        let events: Vec<MutationEvent> = std::mem::take(&mut buffer.lock().unwrap());

        if events.is_empty() {
            // Mutating-shape query that didn't actually mutate
            // (e.g., MATCH that found nothing to SET). Reinstall the
            // durable recorder if there is one, then return.
            if let Some(rec) = self.wal.as_ref() {
                let staged = handle.as_mut();
                install_recorder_if_inmemory(
                    staged,
                    Some(rec.clone() as Arc<dyn MutationRecorder>),
                );
            }
            return Ok(rows);
        }

        // Durable WAL append. WAL goes first so a crash between WAL
        // and the in-memory state stays recoverable: the next boot
        // replays our events on top of the snapshot.
        let mut wrote_commit = false;
        if let Some(rec) = self.wal.as_ref() {
            ensure_wal_query_can_start(rec)?;
            wrote_commit = rec.commit_events(events)?.wrote();
        }

        // Reinstall the durable recorder so the post-publish live
        // store keeps observing future mutations.
        if let Some(rec) = self.wal.as_ref() {
            let staged = handle.as_mut();
            install_recorder_if_inmemory(staged, Some(rec.clone() as Arc<dyn MutationRecorder>));
        }

        if wrote_commit {
            if let Some(rec) = self.wal.as_ref() {
                let live = handle.snapshot();
                self.observe_snapshot_commit_if_needed(&*live, rec)?;
            }
        }

        Ok(rows)
    }
}
