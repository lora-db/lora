# Write-ahead log

LoraDB's WAL (write-ahead log) gives the in-memory engine **continuous
durability**: every mutating query is appended to a durable log before
the call returns, so a crashed process can replay committed writes on
the next boot. The WAL is fully optional — without `--wal-dir` the
server still runs as a pure in-memory database with snapshot-only
durability.

This document is the operator-facing reference. For the design rationale
and the seams under the hood, read
[../decisions/0004-wal.md](../decisions/0004-wal.md).

## Scope and surface

The WAL is shipped today through:

- The **Rust API** on `lora-database`:
  - `Database::open_with_wal(WalConfig)`
  - `Database::recover(snapshot, WalConfig)`
  - `Database::checkpoint_to(path)`
- The **HTTP server** `lora-server` via the `--wal-dir`,
  `--wal-sync-mode`, and `--restore-from` flags, and the admin routes
  `/admin/wal/status`, `/admin/wal/truncate`, and `/admin/checkpoint`.

The Python, Node.js, Ruby, WASM, and Go FFI bindings **do not** expose
WAL configuration in v0.3.x; they remain snapshot-only. If you need
WAL durability, run `lora-server` and talk to it over HTTP, or depend
on `lora-database` directly from Rust.

## Quick start

```bash
# Fresh boot with a WAL.
lora-server --wal-dir /var/lib/lora/wal

# Crash recovery on next start: same flag, same dir.
lora-server --wal-dir /var/lib/lora/wal

# WAL + snapshot hybrid: load snapshot, replay WAL above its fence,
# and use the snapshot path as the default checkpoint target.
lora-server --wal-dir /var/lib/lora/wal \
            --snapshot-path /var/lib/lora/graph.bin \
            --restore-from /var/lib/lora/graph.bin
```

## Sync modes

`--wal-sync-mode` controls when the WAL `fsync`s. There is no global
"right" answer — it is a wallclock-budget knob.

| Mode | `fsync` cadence | Crash window | When to use |
|---|---|---|---|
| `per-commit` (default) | Per commit, before the call returns | 0 — every observed result is durable | Strong durability, write rate fits the disk's `fsync` budget |
| `group` | On a 50 ms timer in a background thread | Up to ~50 ms of writes | Write-heavy workloads where ~50 ms is acceptable |
| `none` / `off` | Never (relies on the OS) | Whatever the kernel decides | CDC-only, read replicas, or testing |

### Group mode honesty

If the background flusher's `fsync` fails (full disk, hardware error,
revoked permissions), the failure is **latched** onto the WAL itself.
From that moment:

- Every subsequent `commit` / `flush` / `force_fsync` returns
  `WalError::Poisoned`.
- The recorder's `poisoned()` flag becomes `Some(...)`, so the next
  query through `Database::execute_with_params` fails with a clear
  durability error.
- `/admin/wal/status` reports the cause in `bgFailure`.

The expected operator response is: stop accepting writes, restart from
the last consistent snapshot + WAL, and remediate the underlying disk
problem.

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

Segment rotation happens at `TxBegin` boundaries when the active
segment crosses `segment_target_bytes` (default 8 MiB), so a single
transaction never spans segments.

## Records

Every record carries `lsn` (monotonic, allocated under the WAL's internal lock)
and most carry `tx_begin_lsn` to associate per-mutation entries with
their owning query.

| Kind | Body | When written |
|---|---|---|
| `TxBegin` | — | Lazily, on the first mutation event of a query |
| `Mutation` | `MutationEvent` (bincode) | Per primitive mutation in the query |
| `TxCommit` | — | After the query returned `Ok` |
| `TxAbort` | — | After the query returned `Err` (and a `TxBegin` had been issued) |
| `Checkpoint` | `snapshot_lsn` | After a checkpoint snapshot has been renamed into place |

Read-only queries fire **no** records — the recorder is *armed* on
every query but only allocates a `TxBegin` LSN on the first mutation
event. Pure reads therefore cost zero log bytes and zero `fsync`.

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

## Admin routes

The WAL admin routes mount when `--wal-dir` is set, **independent** of
`--snapshot-path`:

```http
POST /admin/wal/status
```

Returns a JSON snapshot of WAL state:

```json
{
  "durableLsn": 4815,
  "nextLsn": 4820,
  "activeSegmentId": 3,
  "oldestSegmentId": 2,
  "bgFailure": null
}
```

```http
POST /admin/wal/truncate
Content-Type: application/json

{ "fenceLsn": 4815 }
```

Drops sealed segments whose entire range is at or below `fenceLsn`. The
active segment and the segment immediately preceding it are always
retained. With no body, the WAL truncates up to its current
`durableLsn`.

```http
POST /admin/checkpoint
Content-Type: application/json

{ "path": "/var/lib/lora/checkpoint.bin" }
```

Writes a snapshot stamped with the WAL's `durable_lsn`, appends a
`Checkpoint` marker, and truncates the log up to that fence. When
`--snapshot-path` is configured, the body's `path` is optional — the
snapshot path is the default. When `--snapshot-path` is **not**
configured, the body must include a `path` or the call returns 400.

> ⚠️ **Security.** The admin routes share the auth (or lack thereof)
> story of the snapshot routes — see
> [snapshots.md](snapshots.md#the-http-admin-surface). Deploy behind
> authenticated transport only.

## Failure modes and what to do

| Symptom | Cause | Operator action |
|---|---|---|
| Query fails with `WAL flush failed: ...` | `fsync` returned an OS error | Investigate disk, restart from last checkpoint |
| `/admin/wal/status` shows `bgFailure: "..."` | Group-mode bg flusher hit a fsync error | Same as above |
| Boot prints "snapshot at LSN X is older than the newest checkpoint marker" | Operator passed a stale `--restore-from` | Check whether a more recent snapshot exists; replay from the older one is still safe but does extra work |
| A `*.wal.tmp` is left in the WAL dir | Crash mid-rotation | Safe to delete — segment rotation never relies on `.tmp` files |

## See also

- [Snapshots](snapshots.md) — point-in-time saves and the
  `wal_lsn` checkpoint fence.
- [0004-wal.md](../decisions/0004-wal.md) — design decision and
  trade-offs.
- [Known risks](../design/known-risks.md) — open gaps in the storage
  layer.
