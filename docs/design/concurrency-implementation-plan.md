# Concurrent Write Implementation Plan

This plan captures the current concurrency state and the phase-by-phase route
to concurrent reads, concurrent writes, and concurrent file syncs.

## Current State

Reads are snapshot-based and concurrent: `LiveStore`
(`crates/lora-database/src/live_store.rs`) holds `Arc<S>` behind a `RwLock`,
read paths clone the current `Arc`, and live read streams pin that snapshot
for their lifetime.

Writes are serialized. Auto-commit writes, explicit read-write transactions,
mutating streams, checkpointing, and snapshot restore all take the database
writer mutex. Auto-commit writes take one of two shapes
(`database/occ.rs`, `database/write_guard.rs`):

- **Staged** (the default): the closure runs on a structural copy of the
  graph, which is published only after the WAL commit succeeds. A failure
  discards the copy and leaves the live graph unchanged.
- **Live fast path**: a plan provably unable to fail midway (one write
  clause: a node-only `CREATE`, a `SET` of literal or parameter values, or
  a `DELETE`; no `MERGE`, `REMOVE`, `FOREACH`, `CALL { }` or `UNION`) with
  no deadline mutates the live graph in place through `Arc::make_mut`. If such
  a plan fails after emitting WAL events anyway, the recorder is poisoned
  and the database must recover from snapshot + WAL.

The `LockTable` and `MutationWriteSet` types exist in `lora-store`, and the
database builds a `LockTable`, but no commit path uses them yet for
fine-grained concurrent commits.

The WAL path is single-threaded. `WalRecorder` buffers mutation events in
memory, `abort` only clears that buffer, and commits use the one-shot
`Wal::commit_tx` (or `commit_tx_lsn`, which also returns the commit LSN for
the change feed) to write the begin/batch/commit triple in one critical
section, so an aborted query never reaches disk. `SyncMode::GroupSync`
commits write bytes to the OS, while the background flusher, `force_fsync`,
checkpoint, `Database::sync`, and clean drop provide the fsync boundary.

## Performance Guard

Every phase below is gated by the `concurrency_guard` bench: capture a
baseline on the same machine and session before the phase, rerun after, and
compare with `scripts/check-bench-delta.mjs` at `--threshold 1.15`. The
commands, the covered cases and how to read a failure are in
[Perf smoke → Concurrency guard](../performance/perf-smoke.md#concurrency-guard).

## Phase 0: Baseline And Documentation

Goal: make the existing single-writer design a clean reference point.

- Keep the current `WalRecorder` simplification and one-shot `Wal::commit_tx`
  path.
- Remove or keep removed orphan future-flusher code that is no longer compiled.
- Update stale benchmark comments that still describe the old ArcSwap/CAS
  roadmap.
- Run `concurrency_guard` before and after any cleanup. This phase
  should have near-zero performance movement.

Exit criteria:

- Current read/write/WAL behavior is documented.
- Guard comparison passes at `--threshold 1.15`.
- Focused transaction, WAL, managed snapshot, and named archive tests pass.

## Phase 1: Define Isolation And Conflict Semantics

Goal: write down the first supported concurrent-write contract before changing
commit mechanics.

- Start with snapshot isolation plus write-write conflict detection.
- Define conflicts for same node, same relationship, relationship endpoint
  adjacency, deletes, detach-delete, and `Clear`.
- Treat snapshot restore, explicit checkpoint, truncation, and global admin
  mutations as barriers.
- Add tests that assert conflict behavior without yet making writers parallel.

Performance guard usage:

- Capture a baseline before adding checks.
- Compare after adding classification/conflict bookkeeping.
- Any read-only regression should be considered suspicious, because this phase
  should mostly touch write analysis.

## Phase 2: Stage Auto-Commit Writes

Goal: stop mutating the live graph directly during auto-commit execution.

Status: mostly in place. Auto-commit writes are staged unless the plan is
provably failure-free (see Current State); what remains is the write
attempt below, carrying its events, write-set and base snapshot to a
commit step.

- Make auto-commit writes execute on a staged graph with a buffering recorder,
  matching the explicit transaction model.
- Return a write attempt containing rows, buffered `MutationEvent`s,
  `MutationWriteSet`, and base snapshot metadata.
- Commit or discard the staged result atomically.
- Remove the current trade-off where a failed mutating query can leave live
  memory partially ahead of durable state.

Performance guard usage:

- Watch `write_create_one_steady`, `write_set_existing_1k`,
  `tx_write_create_one`, and `wal_*_create_delete_one`.
- A small write regression may be expected here, but it should be explained and
  capped before later phases stack on top.

## Phase 3: Add Fine-Grained Commit Protocol

Goal: allow disjoint writers to commit concurrently while preserving conflict
correctness.

- Build `MutationWriteSet` from buffered events.
- Acquire `WriteSetLocks` in sorted order.
- Validate touched records against the base snapshot.
- Apply buffered events to the live graph only after validation.
- Keep `Clear` and global admin operations on the writer barrier.
- Add retry for auto-commit conflicts; keep explicit transaction conflict
  errors visible unless automatic transaction retry is explicitly designed.

Performance guard usage:

- Compare with a baseline from the end of Phase 2.
- `write_set_existing_1k` is the main single-thread overhead signal.
- `mixed_4_readers_1_writer` should not regress meaningfully.
- Run the wider `concurrent` manually to confirm disjoint writers
  are improving, not just preserving single-thread numbers.

## Phase 4: Make ID Allocation Concurrent-Safe

Goal: prevent duplicate ids when concurrent staged writers create records.

- Move node and relationship id allocation to database-owned atomics or reserve
  id ranges before execution.
- Initialize allocators from recovery and snapshot load fences.
- Ensure aborted attempts do not reuse ids unless the design explicitly allows
  gaps.
- Add concurrent create stress tests.

Performance guard usage:

- Watch `write_create_one_steady`, `tx_write_create_one`, and both WAL create
  benches.
- If atomics show up in read paths, stop and isolate them.

## Phase 5: WAL Ordering And Concurrent Fsync

Goal: preserve commit-order WAL semantics while allowing concurrent durability
coordination.

- Keep append ordering as commit ordering.
- Extend `Wal::commit_tx` to expose the commit LSN or target durable LSN.
- Reintroduce an `FsyncCoord` only after commit protocol work is stable.
- Batch GroupSync waiters and surface background fsync failures through
  `Wal::bg_failure`.

Performance guard usage:

- Use `wal_group_sync_create_delete_one` to isolate append/encode overhead and
  catch coordination overhead.
- For fsync-sensitive work, rerun the guard once before calling a regression.

## Phase 6: Concurrent Snapshot And Archive Syncs

Goal: move expensive file sync work off the write hot path where safe.

- Schedule managed checkpoint work instead of encoding snapshots inline on a
  commit path.
- Pin an `Arc<InMemoryGraph>` and WAL fence for snapshot workers.
- Encode, fsync, rename, append checkpoint marker, and truncate using the
  existing durability contract.
- Surface archive worker failures through the same health path that rejects
  future writes.

Performance guard usage:

- Run the guard before and after moving work off-thread.
- Expect `write_*` and `wal_*` to improve or stay stable.
- Also run WAL/managed snapshot integration tests because the guard only
  catches performance, not recovery correctness.

## Phase 7: Verification And Rollout

Goal: prove the final system is faster under concurrency without quietly
slowing the embedded single-thread case.

- Add deterministic stress tests for stable read snapshots, disjoint writes,
  overlapping conflicts, concurrent creates, WAL replay order, checkpoint
  recovery, and background sync failure poisoning.
- Run `concurrency_guard` for single-thread regression control.
- Run `concurrent` for scaling behavior.
- Run `perf_smoke` so the broad CI canary remains healthy.
- Keep the current single-writer path behind a fallback flag or configuration
  until the concurrent path has enough soak time.

Exit criteria:

- Guard passes against the chosen final baseline.
- Disjoint writer throughput improves under `concurrent`.
- Read-only and streaming read costs stay within the local threshold.
- WAL recovery and snapshot tests pass under the new commit/file-sync model.
