---
title: WAL and Checkpoints
sidebar_label: WAL and Checkpoints
description: Continuous local durability in LoraDB via the write-ahead log. Enable WAL on Rust, Node, Python, Go, Ruby, or lora-server; recover committed writes; checkpoint to snapshots; inspect and truncate the log.
---

# WAL and Checkpoints

LoraDB is still an in-memory engine: the live graph is held in RAM and
one process owns it. The write-ahead log adds a local durability layer
for surfaces that own a filesystem and a process lifecycle. Mutating
queries are recorded as WAL transactions, then a later boot can reopen
the same directory and replay only committed writes.

Snapshots still matter. A snapshot is the portable file you can copy,
archive, seed another process from, or use as a checkpoint fence. The
WAL covers the gap between snapshots; checkpoints fold the two
together.

## Where WAL exists today

| Surface | How to enable it | What you get |
|---|---|---|
| Rust (`lora-database`) | `Database::open_with_wal(...)`, `Database::recover(...)`, `Database::open_named(...)` | Full WAL config, recovery, checkpoints, status, truncation; `snapshot_to_dir` online backups of a named database |
| Node (`@loradb/lora-node`) | `createDatabase("app", { databaseDir })` or `openWalDatabase({ walDir, snapshotDir })` | Archive-backed `.loradb` files, or explicit WAL directories with managed commit-count snapshots and sync-mode control; `db.snapshot()` online backups and `db.checkpoint()` |
| Python (`lora_python`) | `Database.create("app", {"database_dir": ...})` or `Database.open_wal(wal_dir, options)`; async mirrors both | Archive-backed `.loradb` files, or explicit WAL directories with managed commit-count snapshots |
| Go (`lora-go`) | `lora.New("app", lora.Options{DatabaseDir: ...})` or `lora.OpenWal(lora.WalOptions{...})` | Archive-backed `.loradb` files, or explicit WAL directories with managed commit-count snapshots |
| Ruby (`lora-ruby`) | `LoraRuby::Database.create("app", { database_dir: ... })` or `LoraRuby::Database.open_wal(wal_dir, options)` | Archive-backed `.loradb` files, or explicit WAL directories with managed commit-count snapshots |
| HTTP server (`lora-server`) | `--wal-dir`, `--wal-sync-mode`, `--restore-from` | Recovery, sync-mode control, `/admin/checkpoint`, `/admin/wal/status`, `/admin/wal/truncate` |
| WASM (`@loradb/lora-wasm`) | Not exposed | Snapshot-only today |

Raw WAL helpers default to `SyncMode::GroupSync` and an 8 MiB segment
target. Archive-backed named databases default to GroupSync with a 1s
interval. Rust can override the low-level WAL config directly;
Node exposes `syncMode` on both container-backed and explicit WAL opens;
`lora-server` exposes it through `--wal-sync-mode`.

## Pick a persistence shape

| Shape | How to start | Recovery behavior |
|---|---|---|
| In-memory only | `createDatabase()`, `Database::in_memory()`, or `lora-server` with no durability flags | Restart starts from an empty graph |
| Snapshot only | Save with `save_snapshot` / `POST /admin/snapshot/save`, restore with `load_snapshot` / `--restore-from` | Restart returns to the last snapshot only |
| WAL only | Open the same WAL directory or `.loradb` archive again | Replays committed writes from the WAL into an empty graph |
| Snapshot + WAL | Restore a snapshot and open the same WAL directory or `.loradb` archive | Loads the snapshot, reads its `walLsn` fence, then replays committed WAL records newer than that fence |

Use WAL-only when the log is small enough to replay from scratch. Add
checkpoints when replay time or log size starts to matter.

## Quick start

### Node.js

```ts
import { createDatabase, openWalDatabase } from '@loradb/lora-node';

const scratch = await createDatabase();                       // in-memory
const db = await createDatabase('app', { databaseDir: './data' }); // ./data/app.loradb

const walDb = await openWalDatabase({
  walDir: './data/app.wal',
  snapshotDir: './data/app.snapshots',
  snapshotEveryCommits: 1000,
  snapshotOptions: { compression: { format: 'gzip', level: 1 } },
  syncMode: 'groupSync',
});
```

The name is validated and resolved under `databaseDir` as a `.loradb`
archive. Relative paths resolve from the current working directory.
Reopening the same name and directory replays committed WAL records before
the handle is returned. The open runs on the libuv threadpool, so the event
loop keeps serving timers and requests while a large database loads.

Use `openWalDatabase` when you want a raw WAL directory instead of a
portable `.loradb` archive. Pair it with `snapshotDir` to let the
database write managed checkpoints every `snapshotEveryCommits`
committed transactions, and in any case each time the WAL has grown by
64 MiB or by a quarter of the snapshot's size, whichever is larger. Without `snapshotDir` nothing is checkpointed automatically. Node does not expose WAL status or truncate
helpers yet; use Rust or `lora-server` for those operator controls.

### Python

```python
from lora_python import Database, AsyncDatabase

scratch = Database.create()            # in-memory
db = Database.create("app", {"database_dir": "./data"})          # container-backed
also_db = Database("app", {"database_dir": "./data"})            # equivalent

wal_db = Database.open_wal(
    "./data/app.wal",
    {
        "snapshot_dir": "./data/app.snapshots",
        "snapshot_every_commits": 1000,
        "snapshot_options": {"compression": {"format": "gzip", "level": 1}},
    },
)

async_db = await AsyncDatabase.create("app", {"database_dir": "./data"})
async_wal = await AsyncDatabase.open_wal(
    "./data/async.wal",
    {"snapshot_dir": "./data/async.snapshots", "snapshot_every_commits": 1000},
)
```

### Go

```go
scratch, err := lora.New()        // in-memory
db, err := lora.New("app", lora.Options{DatabaseDir: "./data"})      // container-backed
walDb, err := lora.OpenWal(lora.WalOptions{
    WalDir:               "./data/app.wal",
    SnapshotDir:          "./data/app.snapshots",
    SnapshotEveryCommits: 1000,
})
```

### Ruby

```ruby
scratch = LoraRuby::Database.create          # in-memory
db = LoraRuby::Database.create("app", {"database_dir": "./data"})      # container-backed
wal_db = LoraRuby::Database.open_wal(
  "./data/app.wal",
  snapshot_dir: "./data/app.snapshots",
  snapshot_every_commits: 1000,
)
```

### Rust

```rust
use lora_database::{Database, WalConfig};

let db = Database::open_with_wal(WalConfig::enabled("./app"))?;
db.execute("CREATE (:Person {name: 'Ada'})", None)?;

// Later: restore a snapshot, then replay WAL above its fence.
let recovered = Database::recover("graph.bin", WalConfig::enabled("./app"))?;
```

`WalConfig::enabled(path)` uses the current defaults:

| Setting | Value |
|---|---|
| Sync mode | `SyncMode::GroupSync { interval_ms: 50 }` |
| Segment target | 8 MiB |

Use the explicit enum variant when you need different knobs:

```rust
use lora_database::{Database, SyncMode, WalConfig};

let db = Database::open_with_wal(WalConfig::Enabled {
    dir: "./app".into(),
    sync_mode: SyncMode::GroupSync { interval_ms: 50 },
    segment_target_bytes: 16 * 1024 * 1024,
})?;
```

### HTTP server

```bash
# Fresh boot with a WAL.
lora-server --wal-dir /var/lib/lora/wal

# Snapshot + WAL recovery, with checkpoint default path.
lora-server \
  --wal-dir /var/lib/lora/wal \
  --snapshot-path /var/lib/lora/graph.bin \
  --restore-from /var/lib/lora/graph.bin
```

When `--restore-from` and `--wal-dir` are both set, the server loads
the snapshot first and then replays committed WAL records newer than
the snapshot's `walLsn`. If the snapshot file is missing, recovery
falls back to WAL-only and starts from an empty graph before replay.

`--snapshot-path` is not required for WAL recovery. It only enables the
snapshot save/load admin routes and gives `/admin/checkpoint` a default
target path.

## What gets logged

The database arms the WAL once a query has parsed and compiled. The WAL
does not allocate a transaction until the first primitive mutation
fires.

| Query outcome | WAL behavior |
|---|---|
| Read-only query | Writes no records and does not fsync |
| Successful mutating query | Usually writes one committed `MutationBatch`; transaction-style scopes may write `TxBegin`, `Mutation` records, then `TxCommit` |
| Failed mutating query | Does not publish a committed batch; transaction-style scopes that opened a WAL transaction may write `TxAbort` |

Replay is query-atomic even when the log records individual primitive
mutations. A crashed process can leave an uncommitted tail; replay drops it. A
torn record at the end of the active segment is truncated back to the last
valid record before new appends resume, and the cut bytes are kept beside the
segment as `<segment>.wal.torn-<offset>`.

Damage anywhere else fails the open with `LORA_WAL_CORRUPTION` instead of
being cut off: a bad record followed by valid records, a bad record in an
older or sealed segment, or a record whose checksum is valid but whose
contents do not decode. So does a WAL that does not reach back to the replay
fence, for example a log whose early segments a checkpoint removed, opened
without that checkpoint's snapshot, or a log missing a segment in the middle.

Opening a corrupt WAL needs an explicit decision from you; see
[Corrupt WAL: incident procedure](#corrupt-wal-incident-procedure).

A transaction too large for one WAL record is split across several
`MutationBatch` records inside one `TxBegin`/`TxCommit` pair. Property values
nested more than 128 lists or maps deep are rejected when they are written.

## Recovery and checkpoints

Recovery follows the same steps in Rust and `lora-server`:

1. Load the snapshot if one was supplied. Its `walLsn` becomes the
   replay fence. A snapshot saved without a WAL has `walLsn: null`,
   which is treated as fence `0`.
2. Open the WAL directory or `.loradb` archive and replay every committed transaction above
   the fence.
3. Install the live WAL recorder, then accept new queries.

Any snapshot saved from a WAL-backed database carries a fence, whether
it came from `saveSnapshot`, `POST /admin/snapshot/save` or a checkpoint.
The save fsyncs the WAL and reads its durable LSN while holding the
writer lock, so the snapshot and the fence describe the same graph and
recovery replays exactly the writes made after the save. Writers pause
for that fsync, not for the snapshot encode. A plain save does not
write a `Checkpoint` marker or drop WAL segments.

A snapshot with `walLsn: null` (saved without a WAL) is a seed: start
it over an empty WAL directory with
`lora-server --restore-from seed.bin --wal-dir <dir>`, and keep passing
the same seed on later boots. Over a WAL whose early segments a
checkpoint removed, recovery fails with `LORA_WAL_CORRUPTION` instead of
replaying a partial log onto it. If the WAL is poisoned, a save writes
`walLsn: null` too and logs a warning; use that file as a seed for a
new WAL directory.

Checkpointing creates a new fence:

1. Drain the WAL according to the configured sync mode.
2. Read the WAL's current `durableLsn`.
3. Save a snapshot stamped with that LSN in its `walLsn` header field.
4. Rename the snapshot into place.
5. Append a WAL `Checkpoint` marker.
6. Best-effort truncate sealed WAL segments that are safe to drop.

The checkpoint marker is written after the snapshot rename succeeds, so
a marker implies the snapshot existed at the time of the checkpoint.
If recovery sees a newer checkpoint marker than the snapshot you
supplied, it prints a warning and still replays from the snapshot's own
fence. That is safe, but it may do more replay work than necessary.

## Online backups of a named database

A named database (`createDatabase("app", { databaseDir })`,
`Database::open_named`) belongs to the process that opened it. Copying
its directory while that process writes does not give a database: the
WAL can be cut inside a record and the `.loradb` container lags the
WAL. A second process cannot open the directory to copy it either,
because the directory lock refuses it with `LORA_LOCKED`.

Take the copy from the owning process instead. `db.snapshot(targetDir)`
writes a self-contained `<targetDir>/<name>.loradb` while reads and
writes keep running:

```ts
import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase('festimap', { databaseDir: './data' });
await db.execute('UNWIND range(1, 100000) AS i CREATE (:N {i: i})');

const snapshot = await db.snapshot('./backups/2026-10-26');
console.log(snapshot);
// {
//   bytes: 69283,
//   lsn: 3,
//   nodeCount: 100000,
//   path: './backups/2026-10-26/festimap.loradb',
//   relationshipCount: 0
// }

// The copy opens like any database directory.
const copy = await createDatabase('festimap', { databaseDir: './backups/2026-10-26' });
await copy.execute('MATCH (n:N) RETURN count(n) AS nodes'); // [{ nodes: 100000 }]
```

What the copy holds and what it costs:

- **One committed transaction.** The snapshot holds every transaction
  committed at or below `lsn` and none above it, never part of one. The
  change feed batch with that `lsn` is the last write in it.
- **Writers do not wait for the copy.** The call takes the writer lock
  only to fsync the WAL and take the graph; the graph's storage is
  shared copy-on-write, so later writes copy what they change and the
  snapshot keeps reading the version it took. It runs off the JS thread
  and streams, so memory stays bounded by its buffers plus whatever the
  concurrent writes change.
- **Complete or absent.** The container is written under a temporary
  name, fsynced, and renamed into place. If the process dies mid-copy
  (even `kill -9`), the target holds no `.loradb`, only a temporary
  `<name>.loradb.<pid>.<time>.<n>.tmp` that the next open of that
  directory removes. A copy whose promise resolved is durable.
- **The target must be missing or empty** (`LORA_VALIDATION`
  otherwise) and may be on another filesystem: only the rename stays
  inside it. Ship the directory off-host with any tool once the call
  resolves.
- **Named databases only.** An in-memory database, or one opened with
  `openWalDatabase`, rejects with `LORA_CONFIG`: save those with
  `saveSnapshot`.

`db.checkpoint()` checkpoints now and reports the snapshot it wrote. On
a named database it does what `sync()` does (the container becomes a
snapshot of the graph plus the WAL since); with `walDir` and
`snapshotDir` it writes a managed snapshot:

```ts
await db.checkpoint();
// { formatVersion: 2, nodeCount: 100000, relationshipCount: 0, walLsn: 3 }
```

In Rust the same calls are `Database::snapshot_to_dir`, which returns a
`DirectorySnapshot { path, bytes, lsn, node_count, relationship_count }`,
and `Database::checkpoint`, which returns a `SnapshotMeta`:

```rust
use lora_database::{Database, DatabaseOpenOptions};

let db = Database::open_named("festimap", DatabaseOpenOptions::default().with_database_dir("./data"))?;
let snapshot = db.snapshot_to_dir("./backups/2026-10-26")?;
println!("{} bytes up to LSN {}", snapshot.bytes, snapshot.lsn);
```

A GraphQL server built on `@loradb/lora-graphql` exposes the same copy
as `lora.snapshot(targetDir)`; see [Serving](./graphql/serving#backups).

### A closed directory: `lora snapshot`

For a database no process has open, `@loradb/lora-node` ships a `lora`
command:

```bash
npx lora snapshot ./data ./backups/offline
# ./backups/offline/festimap.loradb: 69283 bytes, 100000 nodes, 0 relationships, at LSN 3
```

It opens the database, which takes the directory lock, then writes the
same snapshot. A directory another process owns is refused rather than
copied mid-write:

```text
lora: database `festimap` in `./data` is open in another process; snapshot it from that process with `await db.snapshot(targetDir)`
```

When the directory holds several databases, pick one with
`--name <name>`. Opening also recovers a directory a crash left behind
before the copy is taken.

## Corrupt WAL: incident procedure

When the WAL is damaged in the middle (a flipped bit on disk, a segment
overwritten by another tool), the database refuses to open with
`LORA_WAL_CORRUPTION`. The error names the segment file, the byte
offset, and what was wrong there, for example:

```text
WAL segment `/var/lib/lora/wal/0000000003.wal` is corrupt at byte 52113: CRC mismatch ...; a valid record follows at byte 52190, so this is not a torn tail
```

Nothing is changed on disk. Work through these steps:

1. **Stop and copy.** Keep the server stopped and copy the whole WAL
   directory (and your snapshots) somewhere safe before trying anything.
2. **Prefer a better copy.** If you have a backup, a replica, or a
   recent snapshot plus an intact WAL elsewhere, restore that instead.
3. **Decide whether to cut.** If the WAL in place is the only copy,
   you can start from everything before the damage. Writes recorded
   after the corrupt point are not recovered. If the damage is in an
   older segment, that includes every later segment.
4. **Start once with the cut enabled**, then restart without it:

   ```bash
   lora-server --wal-dir /var/lib/lora/wal --wal-truncate-corrupt-tail
   # or LORA_SERVER_WAL_TRUNCATE_CORRUPT_TAIL=true
   ```

   In Rust, set `truncate_corrupt_tail: true` on `WalConfig::Enabled`
   for `Database::open_with_wal`, `open_with_wal_snapshots` or
   `recover`.

With the option set, the open cuts the log at the first corrupt record
and starts with the transactions committed before it. Nothing is
deleted:

| What | Where it goes |
|---|---|
| Bytes of the corrupt segment from the bad record to its end | `<segment>.wal.corrupt-<offset>` beside it |
| Every later segment | Renamed to `<segment>.wal.after-corrupt-<corrupt segment>-<offset>` |
| The corrupt segment | Truncated at the bad record; if it was sealed, a new segment starts after it |

`lora-server` prints each of these on stderr at startup, and the
embedded library logs them as a `tracing` warning. In Rust,
`Wal::corrupt_cut()` (through `db.wal()`) reports them too. The open
only acts on `LORA_WAL_CORRUPTION` from a damaged record; a missing
segment (a gap) or an unreadable segment header still fails.

Once the server is up, take a snapshot or checkpoint, and do not keep
the option on: a later open should fail loudly if the disk damages the
log again. The option is not wired into named databases
(`openDatabase(name)` and the `.loradb` container) or the language
bindings' WAL options.

## Sync modes

`lora-server --wal-sync-mode` and the Rust API expose GroupSync
durability:

| Mode | `fsync` cadence | Crash window | When to use |
|---|---|---|---|
| `group-sync` | Background fsync, explicit sync, checkpoint, or clean drop | Up to the group interval | Higher write rates with explicit sync points when needed |

In `group-sync` mode, append and commit records are written before the query
returns, but the `fsync` happens on a background cadence. If that
background fsync fails, the failure is latched: future WAL operations
return a poisoned error and `/admin/wal/status` reports `bgFailure`.
Restart from the last consistent snapshot + WAL after fixing the
underlying disk issue.

`group-sync` is the only mode; `lora-server` runs its background fsync
every 50 ms, so a power loss or kernel crash can lose up to that much
acknowledged work. A process crash alone loses nothing that was already
written to the OS.

A change feed on a WAL-backed database only delivers a commit once it is
fsynced, because a commit lost in a crash would have its LSN reused by the
next write after the restart. The feed asks for the fsync straight away, so
an occasional write reaches the feed after one fsync; under a steady stream
of writes batches trail the commit by up to the group interval.

For checkpointed deployments, call `sync()` when you need an immediate
durability point. On a named `.loradb` database `sync()` is also the
checkpoint: it writes the graph as the archive's base snapshot, streamed so it
holds no second copy of the graph, and drops the WAL segments that snapshot
covers. A change feed can then resume only from the last `sync()` on. An open
that replays more WAL than a quarter of its snapshot, and at least 64 MiB of it,
takes the same checkpoint before it returns, so the next open loads the
snapshot instead of replaying the log again. A named database also takes
that checkpoint on its own, on a background thread, each time its WAL has grown
by 64 MiB since the last one, or by a quarter of its snapshot's size when
that is larger (`DatabaseOpenOptions::checkpoint_wal_bytes`, 0 turns it off), so a
process that never calls `sync()` does not keep every segment and a bulk load
into a large graph does not rewrite the snapshot over and over. A checkpoint snapshot is stamped with the WAL's current
`durableLsn`; in `group-sync` mode that fence
can trail the newest writes until the background fsync catches up.

## What's in the WAL directory

```text
<wal-dir>/
  .lora-wal.lock      held while a handle has the directory open
  0000000001.wal      sealed, oldest
  0000000002.wal      sealed
  0000000003.wal      active (highest id)
```

Segments rotate at the segment target (8 MiB by default), always at a
transaction boundary. There is no `CURRENT` pointer file: the active
segment is the one with the highest id. Files that don't match the
segment name pattern (such as a leftover `.tmp`) are ignored at boot.
Treat the directory as one unit: back it up, move it, or delete it
whole, never segment by segment.

## HTTP admin routes

When `lora-server` starts with `--wal-dir`, these routes are mounted:

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/admin/wal/status` | Inspect durable LSN, next LSN, segment ids, and any latched background fsync failure |
| `POST` | `/admin/wal/truncate` | Drop sealed WAL segments up to a fence LSN |
| `POST` | `/admin/checkpoint` | Write a checkpoint snapshot and truncate safe WAL history |

Examples:

```bash
curl -sX POST http://127.0.0.1:4747/admin/wal/status

curl -sX POST http://127.0.0.1:4747/admin/wal/truncate \
  -H 'content-type: application/json' \
  -d '{"fenceLsn": 4815}'

curl -sX POST http://127.0.0.1:4747/admin/checkpoint \
  -H 'content-type: application/json' \
  -d '{"path": "/var/lib/lora/checkpoint.bin"}'
```

`/admin/wal/status` returns:

```json
{
  "durableLsn": 4815,
  "nextLsn": 4820,
  "activeSegmentId": 3,
  "oldestSegmentId": 2,
  "bgFailure": null
}
```

`/admin/checkpoint` can omit the body only when `--snapshot-path` is
configured. Without a configured default, the request body must include
`path` or the route returns `400 Bad Request`. A checkpoint blocks writes
while it runs; reads continue.

`/admin/wal/truncate` can omit the body; in that case it truncates up
to the WAL's current `durableLsn`. It drops only sealed segments whose
whole LSN range is at or below the fence. The active segment and the
one just before it are always kept.

## Boundaries

- **Checkpoint automation is by WAL size and commit count, not by time.**
  Named databases and managed snapshots checkpoint on a background thread
  each time the WAL has grown by 64 MiB, or by the size of the current
  snapshot when that is larger (`checkpoint_wal_bytes` in Rust; the
  bindings use the default). Explicit WAL helpers in Node, Python, Go, and
  Ruby can also write managed snapshots after N committed transactions. A
  plain WAL directory with no snapshot directory, which is what `lora-server
  --wal-dir` opens, has nowhere to put a snapshot: it keeps every segment
  until `POST /admin/checkpoint` or a checkpoint call, and logs one warning
  once it passes 64 MiB. There is no wall-clock scheduler built into the
  engine.
- **No auth on the admin surface.** Snapshot and WAL admin routes are
  off by default but unauthenticated when enabled. Put them behind
  authenticated ingress only.
- **No shared WAL/container root.** One live handle owns one WAL directory or
  `.loradb` archive, enforced by an advisory lock (`.lora-wal.lock` inside a
  WAL directory; archives take their own lock).
  Opening the same root from another process, or from a second live handle in
  the same process, fails with `LORA_LOCKED` until the first handle is closed.
  See [Troubleshooting](./troubleshooting#walcontainer-root-is-already-open).
- **Binding support is asymmetric.** The filesystem-backed bindings can
  open WAL-backed databases. Rust and `lora-server` expose full
  checkpoint, truncate, status, and sync-mode controls. Node also
  exposes sync-mode control. Python, Go, and Ruby expose simple raw WAL
  opens plus managed snapshot options. WASM stays snapshot-only.
- **No cross-version WAL compatibility guarantee yet.** Snapshots are
  the portable backup and migration artifact. Treat WAL directories as
  local runtime state for the same deployment line unless a release
  explicitly says otherwise.

## See also

- [**Snapshots**](./snapshot) - save / load the graph as a single file.
- [**HTTP server guide**](./getting-started/server) - run
  `lora-server`, configure flags, and probe it with `curl`.
- [**HTTP API**](./api/http) - route-by-route request / response
  reference.
- [**Troubleshooting**](./troubleshooting#wal-and-checkpoints) - common
  WAL setup and recovery errors.
- [**Node guide**](./getting-started/node#persisting-your-graph) -
  `createDatabase("app", { databaseDir: "./data" })` and
  `openWalDatabase({ walDir })` in context.
- [**Rust guide**](./getting-started/rust#persisting-your-graph) -
  embedding the engine directly.
