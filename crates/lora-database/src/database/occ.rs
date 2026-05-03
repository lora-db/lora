//! Phase 4.2 optimistic auto-commit write path.
//!
//! Multiple concurrent writers can build their staged copies in
//! parallel; at commit time we validate only that *the records this
//! writer touched* have not been modified since our snapshot. Disjoint
//! writers pass validation without retry.
//!
//! See [`Database::execute_mutating_optimistic`] for the contract; the
//! merge mechanics live in [`crate::replay`].

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};
use lora_compiler::CompiledQuery;
use lora_executor::{LoraValue, MutableExecutionContext, MutableExecutor, Row};
use lora_store::{
    GraphStorage, GraphStorageMut, MutationEvent, MutationRecorder, MutationWriteSet,
};

use crate::database::Database;
use crate::transaction::BufferingRecorder;
use crate::wal::write_scope::ensure_wal_query_can_start;

use super::replay::{
    install_recorder_if_inmemory, merge_events_into, validate_write_set_unchanged,
};

impl<S> Database<S>
where
    S: GraphStorage + GraphStorageMut + Any + Clone + Send + Sync + 'static,
{
    /// Optimistic auto-commit write path with per-record conflict
    /// detection (Phase 4.2). Multiple concurrent writers can build
    /// their staged copies in parallel; at commit time we validate
    /// only that *the records this writer touched* have not been
    /// modified since our snapshot. Disjoint writers pass validation
    /// without retry.
    ///
    /// The merge: a winning writer publishes
    /// `current + my_events_replayed`, not `my_snapshot + my_writes`.
    /// That preserves any concurrent (disjoint) writer's updates that
    /// landed between our snapshot and our commit.
    ///
    /// Per-record locks are acquired (sorted by id) for the write set
    /// so two writers with overlapping write sets serialize at the
    /// commit boundary rather than racing on validation.
    pub(crate) fn execute_mutating_optimistic(
        &self,
        params: BTreeMap<String, LoraValue>,
        deadline: Option<Instant>,
        compiled: &Arc<CompiledQuery>,
    ) -> Result<Vec<Row>> {
        // Cap retries so a livelock under heavy contention surfaces
        // as an error. With per-record validation, retries happen
        // only when this writer's own write set actually overlaps
        // a concurrent writer's; the typical case is one iteration.
        const MAX_RETRIES: usize = 64;

        for _ in 0..MAX_RETRIES {
            let snapshot = self.store.load_full();
            let mut staged: S = (*snapshot).clone();

            // Buffer mutation events tx-locally. Replayed into the
            // durable WAL only on the winning commit; a losing
            // retry leaves no on-disk trace.
            let buffer = Arc::new(Mutex::new(Vec::<MutationEvent>::new()));
            let buffering_rec: Arc<dyn MutationRecorder> =
                Arc::new(BufferingRecorder::new(buffer.clone()));
            install_recorder_if_inmemory(&mut staged, Some(buffering_rec));

            let exec_result = {
                let mut executor = MutableExecutor::with_deadline(
                    MutableExecutionContext {
                        storage: &mut staged,
                        params: params.clone(),
                    },
                    deadline,
                );
                executor.execute_compiled_rows(compiled)
            };

            let rows = match exec_result {
                Ok(rows) => rows,
                Err(e) => return Err(anyhow!(e)),
            };

            install_recorder_if_inmemory(&mut staged, None);

            // Drain the buffered events. Build the write set from
            // them — this is the set of records we're going to
            // validate and apply at commit time.
            let events: Vec<MutationEvent> = std::mem::take(&mut buffer.lock().unwrap());

            if events.is_empty() {
                // Mutating-shape query that didn't actually mutate
                // (e.g., MATCH that found nothing to SET). Nothing
                // to publish — the rows are valid against snapshot.
                return Ok(rows);
            }

            let mut write_set = MutationWriteSet::new();
            write_set.extend_from_events(events.iter());

            // Brief commit critical section: WAL append + state
            // publish must serialize on the writer Mutex so the
            // WAL records appear in commit order. The per-record
            // `LockTable` exists in `Database` but is intentionally
            // not used here — the writer Mutex already provides
            // single-writer-at-commit-time semantics, so per-record
            // locks would be redundant overhead. They become
            // load-bearing in a future phase that drops the writer
            // Mutex in favour of ArcSwap CAS for true concurrent
            // commits; until then the table is plumbed but idle.
            let _commit_lock = self
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());

            // Fast path: nothing else committed since our snapshot.
            // `staged` already reflects exactly what we want to
            // publish — no merge needed. This restores Phase 3's
            // single-writer cost (one mutation pass, one graph
            // clone) when there's no concurrent activity.
            //
            // Slow path (concurrent writer landed): validate per
            // record, then rebuild publish state by replaying our
            // events onto `current`. Roughly doubles per-write cost
            // but is the price of preserving concurrent updates.
            let current = self.store.load_full();
            let publish_state: S = if Arc::ptr_eq(&current, &snapshot) {
                staged
            } else {
                if !validate_write_set_unchanged(&*snapshot, &*current, &write_set) {
                    drop(_commit_lock);
                    continue; // retry from a fresh snapshot
                }
                let mut merged: S = (*current).clone();
                if !merge_events_into(&mut merged, &events) {
                    // The replay couldn't apply (e.g. id allocation
                    // collision the validation missed). Retry.
                    drop(_commit_lock);
                    continue;
                }
                merged
            };
            let mut publish_state = publish_state;

            // Durable WAL append. Order matters: WAL goes first so
            // a crash between WAL and publish replays cleanly.
            let mut wrote_commit = false;
            if let Some(rec) = self.wal.as_ref() {
                ensure_wal_query_can_start(rec)?;
                wrote_commit = rec.commit_events(events)?.wrote();
            }

            // Reinstall the durable recorder on the merged state so
            // the post-publish live store keeps observing mutations.
            if let Some(rec) = self.wal.as_ref() {
                install_recorder_if_inmemory(
                    &mut publish_state,
                    Some(rec.clone() as Arc<dyn MutationRecorder>),
                );
            }

            self.store.store(Arc::new(publish_state));

            if wrote_commit {
                if let Some(rec) = self.wal.as_ref() {
                    let live = self.store.load_full();
                    self.observe_snapshot_commit_if_needed(&*live, rec)?;
                }
            }

            return Ok(rows);
        }

        Err(anyhow!(
            "auto-commit write conflict: exceeded {MAX_RETRIES} retries"
        ))
    }
}
