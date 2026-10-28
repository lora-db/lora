# Write-ahead log: internals

LoraDB's WAL (write-ahead log) gives the in-memory engine **continuous
durability**: every mutating query is appended to the log before the
call returns, and GroupSync creates storage durability boundaries on a
background cadence or explicit sync. A crashed process can replay
committed writes on the next boot. The WAL is fully optional — without
`--wal-dir` the server still runs as a pure in-memory database with
snapshot-only durability.

This page covers how the WAL works inside `lora-wal` and `lora-database`.
**Operating it** (quick start per binding, sync mode, admin routes, what is in
the directory, troubleshooting) is documented on the website:
[WAL and Checkpoints](../../apps/loradb.com/docs/wal.md). For the design
rationale, read [../decisions/0004-wal.md](../decisions/0004-wal.md).

## Scope and surface

The WAL is shipped today through:

- The **Rust API** on `lora-database`:
  - `Database::open_with_wal(WalConfig)`
  - `Database::recover(snapshot, WalConfig)`
  - `Database::checkpoint_to(path)`
  - `Database::open_with_wal_snapshots(wal_config, snapshot_config)`
  - `Database::open_named(name, options)` for `.loradb` container-backed WAL mirrors
  - `Database::snapshot_to_dir(target)` and `Database::checkpoint()` on a
    named database (see [Online snapshots](#online-snapshots-of-a-named-database))
- The **Node.js binding** through named archive databases and explicit
  `openWalDatabase(...)`, with `db.snapshot()`, `db.checkpoint()` and the
  `lora snapshot` command.
- The **Python, Go, and Ruby bindings** through named archive databases and raw
  WAL opens with managed snapshot options.
- The **HTTP server** `lora-server` via the `--wal-dir`,
  `--wal-sync-mode`, and `--restore-from` flags, and the admin routes
  `/admin/wal/status`, `/admin/wal/truncate`, and `/admin/checkpoint`.

The WASM binding remains snapshot-only because it has no filesystem path
surface.

## Sync mode

`SyncMode` (`crates/lora-wal/src/config.rs`) has one variant,
`GroupSync { interval_ms }`, defaulting to 50 ms. Commit bytes are written to
the OS before the call returns; `fsync` runs on the background flusher
(`src/wal/group_flusher.rs`), on explicit sync, on checkpoint, and on clean
drop. `lora-server` only accepts `group-sync` (`config/env.rs`).

### Change feeds wait for durability

`Database::changes` on a WAL-backed database tags each batch with the LSN
of its `TxCommit` record. That record is in the OS page cache when the
commit returns, but only durable after the next `fsync`; a power loss or
kernel crash before then loses it, and the reopened WAL hands the same LSN
to the next transaction (`next_lsn` is the highest LSN replay found, plus
one). A consumer that had seen the lost batch would then skip the new one
or attribute it to the old one.

So a feed only hands out durable commits. The commit path builds the batch
under the writer lock as before and parks it in the `ChangeHub`
(`crates/lora-database/src/changes/mod.rs`) with the LSN it waits for. The
first `changes()` call installs a durable listener on the WAL
(`Wal::set_durable_listener`), which `flush_inner` calls after every `fsync`
that advances `durable_lsn`; the listener releases the parked batches it
covers, on the thread that ran the `fsync`. Parking a batch also calls
`Wal::request_fsync`, which wakes the GroupSync flusher early. The flusher
still keeps one interval between two `fsync`s: a request right after one
waits for the rest of the interval, so a busy writer sees the usual cadence
and only the first commit after a quiet spell is made durable at once.
Serving every request at once cut commit throughput to about a seventh in
measurements with `lora-database/examples/wal_write_probe.rs`, because the disk flush
competes with the writes. A feed resuming from history that reaches past
the durable LSN (commits made just before the first `changes()` call)
forces an `fsync` before it replays. Snapshot-restore `Reset` batches wait
only for the commits before them; the restore itself is checkpointed.

The regression test is
`lora-database/tests/change_feed.rs::feed_never_shows_a_commit_a_crash_can_lose`:
it holds the WAL's `fsync` (`Wal::hold_fsync_for_tests`), cuts the active
segment back to its last durable length to simulate the crash, and checks
that every LSN the consumer saw names the same transaction after recovery.

### GroupSync failure latching

If the background flusher's `fsync` fails (full disk, hardware error,
revoked permissions), the failure is **latched** onto the WAL itself.
From that moment:

- Every subsequent `commit` / `flush` / `force_fsync` returns
  `WalError::Poisoned`.
- The recorder's `poisoned()` flag becomes `Some(...)`, so the next
  query through `Database::execute_with_params` fails with a clear
  durability error.
- `WalAdmin::wal_status` (and `/admin/wal/status`) reports the cause in
  `bgFailure`.

## Directory lock

`Wal::open` takes a best-effort advisory lock on `<dir>/.lora-wal.lock`
(`crates/lora-wal/src/lock.rs`, `flock` on Unix) and holds it for the handle's
lifetime. A second live open of the same directory, from another process or
the same one, fails with `WalError::AlreadyOpen`, which `lora-database` maps to
`LoraErrorCode::Locked` (`LORA_LOCKED`). Acquisition retries for up to 100 ms
to absorb the race between a clean drop releasing the lock and an immediate
reopen. `.loradb` archives take their own lock
(`crates/lora-database/src/wal/archive/lock.rs`).

## File layout

A WAL directory holds a sequence of segment files:

```
<wal-dir>/
  0000000001.wal      sealed, oldest
  0000000002.wal      sealed
  0000000003.wal      active
```

Each segment has a self-describing header (magic, format version, base
LSN, sealed flag, header CRC) and a sequence of length-prefixed,
CRC-checked records. The active segment is always the file with the
highest numeric id — there is no separate `CURRENT` pointer file.

Segment files are named by a zero-padded 10-digit id (`src/dir.rs`); files
that don't match the pattern (a stray `.tmp`, `.lora-wal.lock`) are ignored
when listing segments.

Segment rotation happens before appending a new record when the active segment
crosses `segment_target_bytes` (default 8 MiB). Transaction-style record groups
are kept together by rotating at the transaction boundary.

## Records

Every record carries `lsn` (monotonic, allocated under the WAL's internal lock)
and most carry `tx_begin_lsn` to associate per-mutation entries with
their owning query.

| Kind | Body | When written |
|---|---|---|
| `TxBegin` | — | Transaction boundary for legacy/multi-record write scopes |
| `Mutation` | `MutationEvent` | One primitive mutation record |
| `MutationBatch` | `Vec<MutationEvent>` | The common auto-commit path buffers a successful mutating query into one committed batch |
| `TxCommit` | — | Commit marker for a transaction-style write scope |
| `TxAbort` | — | Abort marker for a transaction-style write scope that opened a WAL transaction |
| `Checkpoint` | `snapshot_lsn` | After a checkpoint snapshot has been renamed into place |

Read-only queries fire **no** records. Mutations are recorded only after the
query or transaction reaches a successful commit point; replay ignores any
uncommitted transaction records.

## Recovery

`Database::recover(snapshot_path, WalConfig::Enabled { dir, ... })`:

1. Load the snapshot, capturing its `wal_lsn` (the fence). A missing
   snapshot file is treated as "fresh start", so operators can pass
   the same path on every boot. Snapshots saved from a WAL-backed
   database carry the fence whether they came from `checkpoint_to` or
   `save_snapshot_to*` (see [Snapshots](snapshots.md)); a snapshot saved
   without a WAL has none and counts as fence 0, a seed for a fresh WAL
   directory.
2. Open the WAL at `dir` with that fence. `replay_segments` walks
   every segment, drops records at or below the fence (already in the
   loaded snapshot), buffers per-transaction events, and emits only
   *committed* events in commit order. A transaction is buffered only
   up to `PENDING_BUDGET_BYTES` (4 MiB) of encoded records; past that
   its events go to the sink as they are read (`ReplayStep::Partial`),
   followed by `Commit` with the rest or `Discard` when it aborts, never
   commits, or another transaction's record comes first. `lora-database`
   applies the parts over a copy-on-write clone of the graph and puts
   the clone back on `Discard`, so an open holds one WAL record of a
   large transaction decoded rather than all of it.
3. Apply the replay events to the in-memory graph **before** the
   `WalRecorder` is installed, so replay's mutations don't get
   re-recorded.
4. Install the recorder; the server is ready.

A torn tail is truncated to the offset just before the bad bytes, and
subsequent appends pick up at that boundary. A bad frame (CRC mismatch,
length out of range, or a record running past end of file) counts as a
torn tail only when it is in the newest segment, that segment is not
sealed, and no CRC-valid record with a higher LSN starts anywhere after
it. The cut bytes are copied to `<segment>.wal.torn-<offset>` first.
Anything else fails the open with `WalError::Corrupt`: a bad frame with
valid records after it, a bad frame in an older or sealed segment, or a
CRC-valid record whose payload does not decode. Read errors fail the open
as `WalError::Io`.

`WalConfig::Enabled { truncate_corrupt_tail: true, .. }`
(`Wal::open_replaying_with(.., true, ..)`, `lora-server
--wal-truncate-corrupt-tail` / `LORA_SERVER_WAL_TRUNCATE_CORRUPT_TAIL`) is
the incident-recovery way out of `WalError::Corrupt`. Replay stops at the
first corrupt point instead of failing, so only the transactions committed
before it are applied, then `cut_corrupt_log` (`src/wal/wal.rs`):

1. copies the corrupt segment's bytes from the offset to its end into
   `<segment>.wal.corrupt-<offset>` and fsyncs it;
2. renames every later segment to
   `<segment>.wal.after-corrupt-<corrupt segment id>-<offset>` (names the
   segment listing ignores) and fsyncs the directory;
3. truncates the corrupt segment at the offset and fsyncs it.

A crash part way leaves the corruption in place, so the next forced open
cuts again. If the corrupt segment is sealed it is now the newest one, and
the open creates its successor as after a rotation crash. The next LSN is
above both the last record kept and the snapshot fence. The cut is logged
with `tracing::warn!` and kept on the handle as `Wal::corrupt_cut()`;
`lora-server` prints it on stderr. Only `WalError::Corrupt` is cut:
`WalError::Gap`, `Malformed` structure, a bad segment header and I/O errors
still fail the open. Named databases (`DatabaseOpenOptions`) do not take the
option, because their work directory, where the cut data would be kept, is
removed on a clean close. The operator procedure is in
[WAL and Checkpoints](../../apps/loradb.com/docs/wal.md#corrupt-wal-incident-procedure).

Replay also fails with `WalError::Gap` when the oldest segment starts
above `fence + 1` (its predecessors were removed by a checkpoint whose
snapshot was not loaded) or when a segment id is missing between the
first and last segment.

Segment rotation fsyncs the old segment, creates and fsyncs the next one,
then seals the old one. Open repairs the two states a crash in that
sequence can leave: a newest segment without a valid header and no
records is removed, and a sealed newest segment gets its successor
created.

If the WAL contains a `Checkpoint` marker newer than the snapshot's
`wal_lsn`, recovery prints a one-line warning to stderr — the
operator probably meant to pass a more recent snapshot. Replay still
proceeds from the snapshot's fence (conservative-correct).

## Truncation

`Wal::truncate_up_to(fence)` (`src/wal/wal.rs`) drops sealed segments whose
entire LSN range is at or below the fence. The active segment is never
deleted, and the segment immediately before it is kept so a crash before the
next checkpoint still finds a self-describing log start. `checkpoint_to` calls
it after writing the `Checkpoint` marker, under the writer mutex;
`/admin/wal/truncate` calls it without taking any store lock. It also restarts
the growth count behind the automatic checkpoint below.

## Automatic checkpoint by WAL size

A process that writes for days and never calls `sync()` or a checkpoint
would keep every segment, and the next open would replay all of them. So a
database with somewhere to put a snapshot checkpoints on its own once the
WAL has grown by a threshold since the last checkpoint:

| Configuration | Option (0 turns it off) | Default |
|---|---|---|
| Managed snapshots (`open_with_wal_snapshots`) | `SnapshotConfig::checkpoint_wal_bytes` / `.checkpoint_wal_bytes(n)` | 64 MiB |
| Named `.loradb` database (`open_named`) | `DatabaseOpenOptions::checkpoint_wal_bytes` | 64 MiB |
| Plain WAL (`open_with_wal`, `recover`, `lora-server --wal-dir`) | none | warns once at 64 MiB |

The default is `DEFAULT_CHECKPOINT_WAL_BYTES`, the same size as
`compact_on_open_wal_bytes`, at which a named database's open checkpoints.
It is independent of `checkpoint_every_commits`, which still works as before.

Every checkpoint rewrites the whole snapshot, so the trigger grows with the
snapshot: the WAL listens for `max(checkpoint_wal_bytes, a quarter of the
current snapshot's size)` of growth, recomputed at open, after each automatic
checkpoint, `sync()` and `checkpoint_managed()`. Checkpoint writes then stay
at most about four times the WAL bytes written. With a fixed 64 MiB a bulk
load into a large graph would rewrite the full snapshot back to back. The
fraction is a quarter rather than a whole snapshot because the WAL also
bounds replay on open: replay runs on one thread at about 85 MB/s while the
snapshot loads on every core, so a whole snapshot of WAL on a multi-gigabyte
database could add tens of seconds to a restart. Since snapshots are built on
every core, the more frequent checkpoints cost little. A named database's open
compacts by the same fraction: at least `compact_on_open_wal_bytes` of WAL and
more than a quarter of the snapshot.

`Wal` counts the bytes of every committed transaction
(`Wal::bytes_since_checkpoint`); `truncate_up_to`, which every checkpoint
ends with, restarts the count. After a commit that leaves the count at or
past the threshold, the WAL calls the growth listener
(`Wal::set_growth_listener`), installed by
`crates/lora-database/src/wal/auto_checkpoint.rs`. The listener runs on the
committing thread and only starts a `lora-checkpoint` thread (one at a time)
and restarts the count. The thread:

- on a named database runs `sync()`'s checkpoint (`checkpoint_named`):
  archive checkpoint lock, then the writer lock just long enough to fsync
  the WAL and take the graph and its durable LSN, then the snapshot is
  written and installed without the writer lock;
- with managed snapshots runs
  `ManagedSnapshotStore::checkpoint_behind_writers`: the writer lock just
  long enough to fsync and take the graph and LSN, then, under the store's
  checkpoint lock, the snapshot, the `Checkpoint` marker, the fsync and the
  truncation. If a checkpoint at or past that LSN was installed meanwhile it
  writes nothing, so an older snapshot never becomes `CURRENT` after a newer
  checkpoint dropped the WAL it would need. The commit-count checkpoint skips
  its turn while one of these runs instead of making writers wait.

So writers wait for an fsync, not for the snapshot. The thread needs the
writer lock, so it starts only after the commit that triggered it is
published. A failure is logged with `tracing::warn!` and never reaches a
write; the WAL still holds every commit, and the next attempt comes after
another threshold of growth. Dropping the database waits for a running
checkpoint.

A plain WAL has nowhere to put a snapshot (`recover` reads its snapshot path
but does not own it), so nothing is checkpointed: when the WAL has grown by
64 MiB it logs one warning and stops listening. Checkpoint it with
`checkpoint_to` (`POST /admin/checkpoint` on `lora-server`) or open it with
managed snapshots. `lora-server` does not install a `tracing` subscriber, so
the warning is not printed there.

## Online snapshots of a named database

`Database::snapshot_to_dir(target)` (`src/snapshot/directory.rs`) copies
a named database while it runs, without the work directory:

1. Check `target` is missing or empty, create it.
2. Under the writer mutex: `force_fsync_wal_only`, then take the graph
   `Arc` and the WAL's `durable_lsn`. That is the whole writer pause:
   the graph's storage is copy-on-write, so later writes copy the parts
   they change and the taken version stays as it was.
3. Stream a snapshot of that graph, fenced at the LSN, into
   `snapshot.lsnap` in a staging directory beside the target, named like
   the container's temporary files
   (`<name>.loradb.<pid>.<nanos>.<seq>.tmp`), which an open of the target
   directory removes.
4. Write a container holding only that snapshot frame through the same
   `write_archive_atomic` the archive uses: a temporary file, fsync,
   rename to `<target>/<name>.loradb`, fsync of the directory.

The copy opens like any container: the snapshot loads, the WAL is empty,
and new LSNs start above the fence. A kill at any point leaves either no
`<name>.loradb` in the target or a complete one. It holds the archive's
checkpoint lock not at all, so it composes with `sync()` and any
automatic checkpoint, which only touch the live work directory.

## See also

- [WAL and Checkpoints](../../apps/loradb.com/docs/wal.md) — operator guide:
  quick start, sync mode, admin routes, directory contents.
- [Snapshots](snapshots.md) — point-in-time saves and the
  `wal_lsn` checkpoint fence.
- [0004-wal.md](../decisions/0004-wal.md) — design decision and
  trade-offs.
- [Known risks](../design/known-risks.md) — open gaps in the storage
  layer.
