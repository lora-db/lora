---
slug: loradb-v0-3-snapshots
title: "LoraDB v0.3: snapshots for saving and restoring graph state"
description: "LoraDB v0.3 adds manual point-in-time snapshots — a single-file dump of the in-memory graph, atomic on rename, restorable at boot or on demand, exposed through every binding and the HTTP admin surface."
authors: [loradb]
tags: [release-notes, announcement, persistence, operations]
---

LoraDB v0.3 adds manual point-in-time snapshots.

You can now dump the entire in-memory graph to a single file and
restore it later. The save is atomic on rename, the load replaces the
live graph in one shot, and the feature is exposed on every surface
that the engine talks through — the Rust core, the Python, Node, WASM,
Go, and Ruby bindings, the shared C FFI, and the HTTP server as an
opt-in admin endpoint.

What this release is **not** is full persistence. There is no
write-ahead log, no background checkpoint loop, no continuous
durability. A snapshot is exactly what the name says: a point-in-time
dump you take on demand. Data mutated between two saves is lost on
crash. That boundary is deliberate — making the explicit, operator-
controlled shape work cleanly is the foundation a WAL will sit on, and
it closes the "no persistence at all" gap for the workloads that only
need occasional checkpoints today (seeded services, notebooks,
controlled shutdowns, scheduled backups).

<!-- truncate -->

## What Changed

The short list:

- A new single-file snapshot format (`LORASNAP` magic, format version
  `1`, bincode-serialized payload, CRC32 footer).
- Atomic saves — writes go to `<path>.tmp`, are `fsync`'d, and then
  renamed over the target. A crashed save never leaves a half-written
  file at the target path.
- Atomic loads — the store mutex is held for the full restore, so
  concurrent queries see the old or the new graph, never a partial
  one.
- Reserved header space for a future WAL/checkpoint hybrid
  (`walLsn` / `has_wal_lsn`); pure snapshots emit it as null today.
- Forward-compatible reader — formats are dispatched by version, so
  today's v1 files will keep loading after the next format bump until
  support is deliberately dropped.
- Snapshot metadata (`formatVersion`, `nodeCount`,
  `relationshipCount`, `walLsn`) returned from every save and load.

Binding support that actually exists in v0.3:

| Surface | Save | Load | Shape |
|---|---|---|---|
| Rust (`lora-database`) | `save_snapshot_to(path)` | `load_snapshot_from(path)`, `in_memory_from_snapshot(path)` | file path |
| Python (sync `Database`) | `save_snapshot(path)` | `load_snapshot(path)` | file path |
| Python (`AsyncDatabase`) | `await save_snapshot(path)` | `await load_snapshot(path)` | file path |
| Node.js (`@loradb/lora-node`) | `await saveSnapshot(path)` | `await loadSnapshot(path)` | file path |
| WebAssembly (`@loradb/lora-wasm`) | `await saveSnapshotToBytes()` | `await loadSnapshotFromBytes(bytes)` | `Uint8Array` |
| Go (`lora-go`) | `db.SaveSnapshot(path)` | `db.LoadSnapshot(path)` | file path |
| Ruby (`lora-ruby`) | `db.save_snapshot(path)` | `db.load_snapshot(path)` | file path |
| C FFI (`lora-ffi`) | `lora_db_save_snapshot(handle, path, ...)` | `lora_db_load_snapshot(handle, path, ...)` | file path |
| HTTP server (`lora-server`) | `POST /admin/snapshot/save` | `POST /admin/snapshot/load` | file path on the server's disk |

WebAssembly is byte-oriented by design — WASM has no filesystem, so
the caller is responsible for persisting the `Uint8Array` to IndexedDB,
localStorage, `fs.writeFileSync`, a backend upload, or wherever their
app already stores state.

## Why Snapshots Matter

The v0.1 and v0.2 model was "one process, one in-memory graph, lost on
exit." That is fine for notebooks, tests, demos, and embedded
read-mostly caches, but it forces every operator into one of two
patterns neither of which the engine supported well:

- **Reload from source on every boot.** Works if the source is cheap,
  but adds real seeding time on restart and pushes reload logic into
  every deployment.
- **Rebuild a parallel persistence layer.** The application writes
  every mutation to Lan external store, then replays it on boot. A
  second data model to maintain, a second consistency story.

Neither is what you want for the shape of workload LoraDB is actually
good at: a graph view over data the host process already owns, or a
small seeded context that the agent / service accumulates in memory.
For those, the right primitive is a file on disk that captures "the
graph as of this moment" — cheap to take, cheap to restore, no second
data model.

That is what v0.3 ships. The Cypher surface does not change; the
storage tier gets one new verb (`save_snapshot`), one new verse
(`load_snapshot`), and one new file on disk.

## What A Snapshot Is Not

Same list as above, stated as the bright line:

- **Not continuous durability.** A crash between two saves loses every
  mutation in the window. If you need zero data loss, you need a WAL;
  LoraDB does not have one yet.
- **Not a checkpoint loop.** Nothing schedules saves for you. The host
  process, an external cron, or the admin HTTP endpoint decides when a
  save happens.
- **Not a general persistent storage tier.** There is no storage
  backend other than the in-memory graph; the snapshot is a dump of
  that graph, not a format a different engine writes into.
- **Not zero-cost at save time.** The store mutex is held for the
  duration of the save. Concurrent queries wait. Pick a snapshot
  cadence that leaves headroom.
- **Not a boundary for multi-tenancy.** One process still holds one
  graph; each process needs its own snapshot path.

Those are not roadmap omissions hidden behind marketing language. They
are what "simple, explicit, operator-controlled" means.

## Using Snapshots

### Save and load from Rust

The reference surface. Every other binding wraps these two methods.

```rust
use lora_database::Database;

let db = Database::in_memory();
db.execute("CREATE (:Person {name: 'Ada'})", None)?;

// Dump the full graph to disk.
let meta = db.save_snapshot_to("graph.bin")?;
println!(
    "{} nodes, {} relationships",
    meta.node_count, meta.relationship_count,
);

// Boot a fresh Database directly from the file.
let db2 = Database::in_memory_from_snapshot("graph.bin")?;

// Or restore onto an existing handle (concurrent queries block on the
// store mutex for the duration of the load).
db.load_snapshot_from("graph.bin")?;
```

Every save and load returns a `SnapshotMeta`:

```json
{
  "formatVersion": 1,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null
}
```

The `walLsn` field is reserved for the future WAL/checkpoint hybrid
and is always `null` for today's pure snapshots.

### Save and load from Python

```python
from lora_python import Database

db = Database.create()
db.execute("CREATE (:Person {name: 'Ada'})")

meta = db.save_snapshot("graph.bin")
print(meta["nodeCount"], meta["relationshipCount"])

db2 = Database.create()
db2.load_snapshot("graph.bin")
```

The `AsyncDatabase` wrapper exposes the same two methods as
coroutines:

```python
import asyncio
from lora_python import AsyncDatabase

async def main():
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:Person {name: 'Ada'})")
    await db.save_snapshot("graph.bin")

asyncio.run(main())
```

Both forms run with the GIL released / on a worker thread so the event
loop stays free during large saves.

### Save and load from Node / TypeScript

```ts
import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase();
await db.execute("CREATE (:Person {name: 'Ada'})");

const meta = await db.saveSnapshot('graph.bin');
console.log(meta.nodeCount, meta.relationshipCount);

const db2 = await createDatabase();
await db2.loadSnapshot('graph.bin');
```

`saveSnapshot` / `loadSnapshot` return Promises that resolve to a
`SnapshotMeta` object with the same `formatVersion` / `nodeCount` /
`relationshipCount` / `walLsn` fields as every other binding.

### Save and load from WebAssembly

WASM has no filesystem, so the snapshot API is byte-in / byte-out:

```ts
import { createDatabase } from '@loradb/lora-wasm';

const db = await createDatabase();
await db.execute("CREATE (:Person {name: 'Ada'})");

// Dump the graph to a Uint8Array.
const bytes: Uint8Array = await db.saveSnapshotToBytes();

// Persist the bytes wherever you already store state — IndexedDB,
// localStorage, a POST to your backend, `fs.writeFileSync` in Node.
// Later:
const db2 = await createDatabase();
await db2.loadSnapshotFromBytes(bytes);
```

The Worker-backed surface (`createWorkerDatabase`) does not yet plumb
snapshots through the worker protocol — for snapshotting from a
browser worker today, call `saveSnapshotToBytes` in-process in the
worker and post the bytes back to the main thread yourself. In-process
WASM (`createDatabase`) supports snapshots on both the Node and
bundler targets.

### Save and load from Go

```go
import lora "github.com/lora-db/lora/crates/lora-go"

db, err := lora.New()
if err != nil { log.Fatal(err) }
defer db.Close()

if _, err := db.Execute("CREATE (:Person {name: 'Ada'})", nil); err != nil {
    log.Fatal(err)
}

meta, err := db.SaveSnapshot("graph.bin")
if err != nil { log.Fatal(err) }
fmt.Printf("nodes=%d rels=%d\n", meta.NodeCount, meta.RelationshipCount)

db2, err := lora.New()
if err != nil { log.Fatal(err) }
defer db2.Close()

if _, err := db2.LoadSnapshot("graph.bin"); err != nil {
    log.Fatal(err)
}
```

The Go FFI header (`crates/lora-go/include/lora_ffi.h`) now declares
`lora_db_save_snapshot` / `lora_db_load_snapshot` alongside a
`LoraSnapshotMeta` struct; the Go wrapper turns that into an idiomatic
`*SnapshotMeta` with a nullable `WalLsn` pointer.

## Restoring And Saving Through The HTTP Server

`lora-server` exposes two opt-in admin endpoints for snapshot
operations. They do not exist unless the server is started with
`--snapshot-path`:

```bash
lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin \
  --restore-from  /var/lib/lora/db.bin
```

- `--snapshot-path <PATH>` mounts `POST /admin/snapshot/save` and
  `POST /admin/snapshot/load` against this file. Without the flag the
  routes return `404` — the admin surface is off by default.
- `--restore-from <PATH>` loads a snapshot at boot before the server
  accepts queries. A missing file is fine (empty graph, logged); a
  malformed file is fatal.

Once enabled, saving and restoring is a plain HTTP call:

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save
# => {"formatVersion":1,"nodeCount":1024,"relationshipCount":4096,"walLsn":null,"path":"/var/lib/lora/db.bin"}

curl -sX POST http://127.0.0.1:4747/admin/snapshot/load
```

Both endpoints accept an optional `{ "path": "…" }` body to override
the configured default for a single request — useful for ad-hoc
backups to a rotated filename:

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save \
  -H 'content-type: application/json' \
  -d '{"path": "/var/backups/lora/2026-04-24.bin"}'
```

`--restore-from` is independent of `--snapshot-path`. You can restore
from a read-only seed and save to a writable runtime path:

```bash
lora-server \
  --restore-from  /var/lib/lora/seed.bin \
  --snapshot-path /var/lib/lora/runtime.bin
```

:::caution Security

The admin endpoints have **no authentication**, and the optional
`path` body field is passed straight to the OS. Any client that can
reach the admin port can write files anywhere the server UID can
write, or swap the live graph by pointing `load` at an attacker-staged
file. Do not expose the admin surface on a network-reachable host
without authenticated ingress in front (a reverse proxy with auth, a
Unix socket, or simply not binding the port at all). Future releases
may add authentication; until then, the correct deployment is "admin
surface disabled by default, enabled only behind an auth boundary".

:::

## Why Snapshots Are Useful Even Without A WAL

A snapshot is not a replacement for continuous durability, but it
closes enough of the gap for the workloads LoraDB currently serves:

- **Seeded services.** Build the graph offline from a cheaper source
  (SQL exports, a scrape, an ETL job), snapshot it, and ship the
  snapshot alongside the deployment. Every restart boots in one
  file-read rather than a multi-minute replay.
- **Notebooks and research tooling.** Save the graph you've curated at
  the end of a session; reload it the next morning with one call.
- **Agents and LLM context stores.** Periodic snapshots of the working
  graph give you trivial "go back to yesterday's state" without the
  complexity of a full transactional store.
- **HTTP operator loop.** `ExecStop=curl … /admin/snapshot/save` on a
  systemd unit gives a graceful-shutdown save without any new
  tooling. Add a `--restore-from` on boot and you have a durable-
  enough deployment for a single-node service.
- **Scheduled backups.** A cron that calls `POST
  /admin/snapshot/save` every N minutes, optionally with a rotating
  `{"path": "…"}`, is a complete backup policy for small graphs.

The bright line is still the same: a crash between saves loses every
mutation in the window. The question to ask is whether that window is
narrow enough for your workload. For most of the shapes above, it is.

## What's Still Out Of Scope

Explicitly not in this release, so the feature stays honest about its
boundary:

- **No WAL / checkpoint loop.** The header reserves space for a WAL
  LSN, but the engine does not yet write one. A future release will
  turn checkpoints into "snapshots with a meaningful `walLsn`" — the
  reader already accepts the flag.
- **No automatic persistence.** Snapshots are always manual. Nothing
  runs them on a schedule unless you do.
- **No partial / incremental snapshots.** A save serializes the whole
  graph. For v0.3 the expected scale is graphs that fit in memory
  comfortably and dump in seconds.
- **Non-blocking save.** The store mutex is held for the full save.
  Concurrent queries block. Real per-mutation copy-on-write will come
  with the WAL work.
- **No multi-graph file format.** One file, one graph — same one-process
  model as the rest of the engine.
- **No auth on the HTTP admin surface.** Opt-in, off by default, and
  still not safe on a network-reachable host without an ingress.

Those are the things a future release will address. They are not
hidden in the implementation — every one of them is a place the docs
say so.

## Try It

Get the repo, build, and snapshot:

```bash
cargo run --bin lora-server -- \
  --snapshot-path /tmp/loradb.bin \
  --restore-from  /tmp/loradb.bin
```

Then from a second shell:

```bash
curl -sX POST http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"CREATE (:Person {name:\"Ada\"})"}' > /dev/null

curl -sX POST http://127.0.0.1:4747/admin/snapshot/save
# => {"formatVersion":1,"nodeCount":1,"relationshipCount":0,"walLsn":null,"path":"/tmp/loradb.bin"}
```

Stop the server, start it again with the same flags, and the graph is
still there.

The docs site has a dedicated page for snapshots — the file format,
atomicity guarantees, binding examples, and the full HTTP admin
surface:

- [Snapshots](/docs/snapshot)
- [HTTP server quickstart → Snapshots, WAL, and restore](/docs/getting-started/server#snapshots-wal-and-restore)
- [HTTP API → Admin endpoints (opt-in)](/docs/api/http#admin-endpoints-opt-in)

## What Comes Next

Three directions stand out after v0.3:

1. **A WAL.** The snapshot header already reserves the slot; the
   missing piece is the append-only log that checkpoints refer to.
   That unlocks continuous durability, which in turn unblocks multi-
   minute crash-recovery windows.
2. **A checkpoint loop.** Once there is a WAL, the engine can fold
   snapshots and the log together in the background — the operator
   stops having to decide when to save.
3. **Auth on the admin surface.** Token-based auth in front of
   `/admin/*` so the endpoints can be used on network-reachable hosts
   without an external reverse proxy.

If you try v0.3 with snapshots, the feedback that will shape those is
concrete:

- how large does your graph get, and how long does `save_snapshot`
  take at that size;
- what cadence did you end up running — seconds, minutes, on shutdown
  only;
- did the atomic-rename guarantee land cleanly on your filesystem
  (we've tested on Linux ext4/xfs and macOS APFS);
- what does your ingress look like for the admin endpoints;
- which binding did you use, and did the byte-based WASM surface fit
  your storage layer (IndexedDB, OPFS, a backend POST) without extra
  glue.

That is the feedback that will shape v0.4.
