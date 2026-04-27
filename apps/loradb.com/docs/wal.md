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
| Rust (`lora-database`) | `Database::open_with_wal(...)`, `Database::recover(...)` | Full WAL config, recovery, checkpoints, status, truncation |
| Node (`@loradb/lora-node`) | `await createDatabase("app", { databaseDir: "./data" })` | Archive-backed embedded database at `./data/app.loradb` |
| Python (`lora_python`) | `Database.create("app", {"database_dir": "./data"})`, `Database("app", {"database_dir": "./data"})`, `await AsyncDatabase.create("app", {"database_dir": "./data"})` | Archive-backed embedded database at `./data/app.loradb` |
| Go (`lora-go`) | `lora.New("app", lora.Options{DatabaseDir: "./data"})`, `lora.NewDatabase("app", lora.Options{DatabaseDir: "./data"})` | Archive-backed embedded database at `./data/app.loradb` |
| Ruby (`lora-ruby`) | `LoraRuby::Database.create("app", { database_dir: "./data" })`, `LoraRuby::Database.new("app", { database_dir: "./data" })` | Archive-backed embedded database at `./data/app.loradb` |
| HTTP server (`lora-server`) | `--wal-dir`, `--wal-sync-mode`, `--restore-from` | Recovery, sync-mode control, `/admin/checkpoint`, `/admin/wal/status`, `/admin/wal/truncate` |
| WASM (`@loradb/lora-wasm`) | Not exposed | Snapshot-only today |

The default embedded-binding WAL settings are `SyncMode::PerCommit`
and an 8 MiB segment target. Rust can override both through
`WalConfig::Enabled`. `lora-server` exposes sync mode through
`--wal-sync-mode`; it uses the same 8 MiB segment target.

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
import { createDatabase } from '@loradb/lora-node';

const scratch = await createDatabase();                       // in-memory
const db = await createDatabase('app', { databaseDir: './data' }); // ./data/app.loradb
```

The name is validated and resolved under `databaseDir` as a `.loradb`
archive. Relative paths resolve from the current working directory.
Reopening the same name and directory replays committed WAL records before
the handle is returned.

This first Node WAL surface intentionally stays small: it does not yet
expose checkpoint, truncate, status, or sync-mode controls.

### Python

```python
from lora_python import Database, AsyncDatabase

scratch = Database.create()            # in-memory
db = Database.create("app", {"database_dir": "./data"})          # archive-backed
also_db = Database("app", {"database_dir": "./data"})            # equivalent

async_db = await AsyncDatabase.create("app", {"database_dir": "./data"})
```

### Go

```go
scratch, err := lora.New()        // in-memory
db, err := lora.New("app", lora.Options{DatabaseDir: "./data"})      // archive-backed
```

### Ruby

```ruby
scratch = LoraRuby::Database.create          # in-memory
db = LoraRuby::Database.create("app", {"database_dir": "./data"})      # archive-backed
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
| Sync mode | `SyncMode::PerCommit` |
| Segment target | 8 MiB |

Use the explicit enum variant when you need different knobs:

```rust
use lora_database::{Database, SyncMode, WalConfig};

let db = Database::open_with_wal(WalConfig::Enabled {
    dir: "./app".into(),
    sync_mode: SyncMode::Group { interval_ms: 50 },
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
| Successful mutating query | Writes `TxBegin`, one or more `Mutation` records, then `TxCommit` |
| Failed mutating query | Writes `TxAbort`; replay discards that query's mutation records |

Replay is query-atomic even though the log records individual primitive
mutations. A crashed process can leave an uncommitted tail; replay
drops it. A torn record in the active segment is truncated back to the
last valid record before new appends resume.

## Recovery and checkpoints

Recovery follows the same steps in Rust and `lora-server`:

1. Load the snapshot if one was supplied. Its `walLsn` becomes the
   replay fence. A pure snapshot has `walLsn: null`, which is treated as
   fence `0`.
2. Open the WAL directory or `.loradb` archive and replay every committed transaction above
   the fence.
3. Install the live WAL recorder, then accept new queries.

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

## Sync modes

`lora-server --wal-sync-mode` and the Rust API expose three durability
cadences:

| Mode | `fsync` cadence | Crash window | When to use |
|---|---|---|---|
| `per-commit` | Before each mutating query returns | 0 for observed successful writes | Strong durability, simplest contract |
| `group` | Background fsync, currently about every 50 ms in `lora-server` | Up to the group interval | Higher write rates where a short loss window is acceptable |
| `none` | Never fsyncs | OS-dependent | Testing, replicas, or external durability layers |

In `group` mode, append and commit records are written before the query
returns, but the `fsync` happens on a background cadence. If that
background fsync fails, the failure is latched: future WAL operations
return a poisoned error and `/admin/wal/status` reports `bgFailure`.
Restart from the last consistent snapshot + WAL after fixing the
underlying disk issue.

For checkpointed deployments, prefer `per-commit` unless you are
deliberately managing the group-mode lag. A checkpoint snapshot is
stamped with the WAL's current `durableLsn`; in `group` mode that fence
can trail the newest writes until the background fsync catches up.

In `none` mode, `durableLsn` is only a logical fence for checkpointing.
It is not a power-loss guarantee.

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

`/admin/checkpoint` can omit the body only when `--snapshot-path` is
configured. Without a configured default, the request body must include
`path` or the route returns `400 Bad Request`.

`/admin/wal/truncate` can omit the body; in that case it truncates up
to the WAL's current `durableLsn`.

## Boundaries

- **No automatic checkpoint loop yet.** Checkpoints are explicit. Host
  code or operators decide when they run.
- **No auth on the admin surface.** Snapshot and WAL admin routes are
  off by default but unauthenticated when enabled. Put them behind
  authenticated ingress only.
- **No shared WAL/archive root.** One live handle owns one WAL directory or
  `.loradb` archive. Opening the same root from another process, or from a second live
  handle in the same process, fails until the first handle is closed.
- **Binding support is asymmetric.** The filesystem-backed bindings can
  open WAL-backed databases, but only Rust and `lora-server` expose full
  checkpoint, truncate, status, and sync-mode controls. WASM stays
  snapshot-only.
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
  `createDatabase("app", { databaseDir: "./data" })` in context.
- [**Rust guide**](./getting-started/rust#persisting-your-graph) -
  embedding the engine directly.
