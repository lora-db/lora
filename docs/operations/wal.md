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
- The **Node.js binding** through named archive databases and explicit
  `openWalDatabase(...)`.
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
   the same path on every boot.
2. Open the WAL at `dir` with that fence. `replay_segments` walks
   every segment, drops records at or below the fence (already in the
   loaded snapshot), buffers per-transaction events, and emits only
   *committed* events in commit order.
3. Apply the replay events to the in-memory graph **before** the
   `WalRecorder` is installed, so replay's mutations don't get
   re-recorded.
4. Install the recorder; the server is ready.

A torn tail (CRC mismatch on the last record of the active segment) is
truncated to the offset just before the bad bytes. Subsequent appends
pick up at that boundary.

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
`/admin/wal/truncate` calls it without taking any store lock.

## See also

- [WAL and Checkpoints](../../apps/loradb.com/docs/wal.md) — operator guide:
  quick start, sync mode, admin routes, directory contents.
- [Snapshots](snapshots.md) — point-in-time saves and the
  `wal_lsn` checkpoint fence.
- [0004-wal.md](../decisions/0004-wal.md) — design decision and
  trade-offs.
- [Known risks](../design/known-risks.md) — open gaps in the storage
  layer.
