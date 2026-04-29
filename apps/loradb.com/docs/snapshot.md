---
title: Snapshots
sidebar_label: Snapshots
description: Manual point-in-time snapshots — save and restore the full in-memory LoraDB graph as a single file or byte payload, through every binding and the opt-in HTTP admin surface. Standalone for backups, or paired with WAL-backed recovery on every filesystem-backed surface.
---

# Snapshots

LoraDB can dump the in-memory graph to a single file and restore it
later. A snapshot is the full graph frozen at the moment the call
took the store mutex — taken on demand, atomic on rename, readable
from any binding.

Snapshots are shipped as of v0.3. They close the "no persistence at
all" gap for workloads that only need occasional save / restore
operations (seeded services, notebooks, graceful-shutdown saves,
scheduled backups). Continuous durability is now available through the
[WAL](./wal) on every filesystem-backed surface, but snapshots are
still the portable file primitive those surfaces checkpoint to.

## What a snapshot is

- A **single encoded payload** containing the full graph — every node,
  every relationship, every property — plus a short header describing
  the format. Filesystem bindings write that payload as one file; WASM
  returns bytes or web-native wrappers around those bytes.
- A **point-in-time dump.** The store mutex is held for the save, so
  every reader sees a consistent graph at the instant the save began.
- **Atomic on rename for path-based saves.** Writes land in
  `<path>.tmp`, are `fsync`'d, then renamed over the target; a
  crashed save can leave a stale `.tmp` file but never a half-written
  target.
- **Format-versioned and forward-compatible.** Files declare a format
  version; the reader dispatches by version so today's v1 files will
  keep loading after a future format bump until support is
  deliberately dropped.

## What snapshots are not

The bright line, stated explicitly so it cannot be missed:

- **Not continuous durability.** A crash between two saves loses every
  mutation in the window unless you pair snapshots with the WAL on a
  filesystem-backed surface.
- **Not a wall-clock checkpoint scheduler.** Manual snapshots run when
  you call them. Explicit WAL helpers can also write managed snapshots
  after N committed transactions, but there is no built-in timer.
- **Not a general persistent storage layer.** There is no alternative
  backend; a snapshot is a dump of the in-memory graph, not a format
  a different engine writes into.
- **Not non-blocking.** The store mutex is held for the full save and
  the full load. Concurrent queries block until the call finishes.
- **Not a multi-tenant boundary.** One process holds one graph; each
  process you run needs its own snapshot file.

## When to use snapshots

Good fits:

- **Seeded services and agents.** Build the graph offline from a
  cheaper source, snapshot it, and ship the file alongside the
  deployment. Every restart boots in one file-read.
- **Notebooks, demos, research tooling.** Save what you've curated;
  reload it tomorrow with one call.
- **Graceful shutdown.** A final `save_snapshot` (or `POST
  /admin/snapshot/save` from systemd `ExecStop`) preserves the graph
  across planned restarts.
- **Scheduled backups.** A cron that calls the admin endpoint with a
  rotating filename every N minutes is a complete backup policy for
  graphs that fit the save window.

Bad fits:

- **Hard-durability workloads.** If losing even a minute of mutations
  on crash is unacceptable, snapshots alone are not enough — use one
  of the [WAL-enabled surfaces](./wal).
- **Very large graphs where save time exceeds your query window.** The
  mutex is held for the save; latency-sensitive reads stall.

## Metadata

Path-based saves, load calls, and HTTP admin calls return a small
metadata record. Byte-output save APIs such as WASM `saveSnapshot()` or
Node `saveSnapshot()` return the snapshot bytes directly instead.

```json
{
  "formatVersion": 1,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null
}
```

| Field | Type | Meaning |
|---|---|---|
| `formatVersion` | integer | On-disk file format the payload is written in. Currently `1`. |
| `nodeCount` | integer | Nodes in the saved / restored graph. |
| `relationshipCount` | integer | Relationships in the saved / restored graph. |
| `walLsn` | integer or null | `null` for a pure snapshot; non-`null` for a checkpoint snapshot written with WAL enabled. |

Whenever metadata is returned, every binding uses the same four
fields; the spelling of the field names matches the wire shape
(camelCase).

## Compression and encryption

Embedded bindings can save snapshots with codec options:

| Option | Supported values |
|---|---|
| `compression` | `"none"`, `"gzip"`, or `{ format: "gzip", level: 0..9 }` |
| `encryption` | Password / passphrase encryption, or a raw 32-byte key |

Password encryption is the most portable shape:

```ts
const encryption = {
  type: 'password',
  keyId: 'backup-key',
  password: process.env.LORA_SNAPSHOT_PASSWORD!,
};

await db.saveSnapshot('graph.lorasnap', {
  compression: { format: 'gzip', level: 1 },
  encryption,
});

await db.loadSnapshot('graph.lorasnap', { credentials: encryption });
```

Node, Python, WASM, and Ruby accept this JSON-style option shape; Go
mirrors it with `SnapshotOptions`, `SnapshotCompression`, and
`SnapshotEncryption` structs; Rust uses the typed `SnapshotOptions`,
`SnapshotPassword`, and `EncryptionKey` structs. Load calls accept the
credentials either under `credentials` or `encryption`, so the same
object used to save can usually be reused to load. HTTP admin snapshot
routes do not accept per-call codec options today; use an in-process
binding when you need custom compression or encryption.

## Binding examples

Snapshots are exposed on every binding that exposes the engine. The
shape is always "save produces a file or byte payload, load consumes a
source, and metadata is returned whenever the operation has a metadata
surface".

### Rust

The reference surface. Every other binding wraps these two methods.

```rust
use lora_database::Database;

let db = Database::in_memory();
db.execute("CREATE (:Person {name: 'Ada'})", None)?;

// Dump the graph to a file. Atomic on rename.
let meta = db.save_snapshot_to("graph.bin")?;
println!("saved {} nodes, {} relationships",
    meta.node_count, meta.relationship_count);

// Boot a fresh Database directly from the snapshot.
let db2 = Database::in_memory_from_snapshot("graph.bin")?;

// Or overlay a snapshot onto an existing handle.
db.load_snapshot_from("graph.bin")?;
```

`SnapshotMeta` is re-exported from `lora_database`:

```rust
use lora_database::SnapshotMeta;
fn log_meta(m: SnapshotMeta) {
    tracing::info!(
        format = m.format_version,
        nodes  = m.node_count,
        rels   = m.relationship_count,
        wal    = ?m.wal_lsn,
    );
}
```

### Python

Synchronous:

```python
from lora_python import Database

db = Database.create()
db.execute("CREATE (:Person {name: 'Ada'})")

meta = db.save_snapshot("graph.bin")
print(meta["nodeCount"], meta["relationshipCount"])

db2 = Database.create()
db2.load_snapshot("graph.bin")
```

Async — same methods as coroutines on `AsyncDatabase`:

```python
import asyncio
from lora_python import AsyncDatabase

async def main():
    db = await AsyncDatabase.create()
    await db.execute("CREATE (:Person {name: 'Ada'})")
    await db.save_snapshot("graph.bin")

    db2 = await AsyncDatabase.create()
    await db2.load_snapshot("graph.bin")

asyncio.run(main())
```

Both the sync and async forms run with the GIL released (sync) / on a
worker thread (async) so other Python threads / coroutines make
progress during the call. A large save still blocks anything that
needs the underlying store mutex.

### Node.js / TypeScript

```ts
import { createDatabase, type SnapshotMeta } from '@loradb/lora-node';

const db = await createDatabase(); // in-memory by default
// const db = await createDatabase('app', { databaseDir: './data' }); // archive-backed
await db.execute("CREATE (:Person {name: 'Ada'})");

const meta: SnapshotMeta = await db.saveSnapshot('graph.bin');
console.log(meta.nodeCount, meta.relationshipCount);

const db2 = await createDatabase();
await db2.loadSnapshot('graph.bin');
```

`saveSnapshot(path)` writes atomically and resolves to `SnapshotMeta`.
Calling `saveSnapshot()` with no path returns a Node `Buffer`; you can
also request `uint8Array`, `arrayBuffer`, `base64`, or a Node stream.
`loadSnapshot` accepts a string / `URL` path, `Buffer`, `Uint8Array`,
`ArrayBuffer`, Node `Readable`, Web `ReadableStream`, or async
iterable. The native engine work still holds the store mutex for the
duration of the save or load.

### WebAssembly

WASM has no filesystem path API. The snapshot API is byte/source based:
the caller chooses where to persist the bytes, and `loadSnapshot` never
accepts a string path.

```ts
import { createDatabase } from '@loradb/lora-wasm';

const db = await createDatabase();
await db.execute("CREATE (:Person {name: 'Ada'})");

// Serialize the graph to a Uint8Array.
const bytes: Uint8Array = await db.saveSnapshot();

// Persist however your app already stores state:
// IndexedDB, localStorage, OPFS, a POST to your backend,
// `fs.writeFileSync` in Node — all work.

// Later (same or a new session), restore from bytes.
const db2 = await createDatabase();
await db2.loadSnapshot(bytes);
```

`saveSnapshot` can also return `{ format: "arrayBuffer" }`,
`"blob"`, `"response"`, `"stream"`, or `"url"`. `loadSnapshot`
accepts `URL`, `Uint8Array`, `ArrayBuffer`, `Blob`, `Response`, or a
`ReadableStream<Uint8Array | ArrayBuffer>`.

The Worker-backed surface (`createWorkerDatabase`) exposes the same
`saveSnapshot` / `loadSnapshot` methods and moves the snapshot work to
the worker thread.

#### Persist across reloads with IndexedDB

```ts
const DB = 'loradb-snapshots', STORE = 'graph', KEY = 'main';

async function idb(): Promise<IDBDatabase> {
  return await new Promise((ok, err) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore(STORE);
    r.onsuccess = () => ok(r.result);
    r.onerror   = () => err(r.error);
  });
}

async function saveToIdb(db: Database) {
  const bytes = await db.saveSnapshot();
  const idbDb = await idb();
  await new Promise<void>((ok, err) => {
    const tx = idbDb.transaction(STORE, 'readwrite');
    tx.objectStore(STORE).put(bytes, KEY);
    tx.oncomplete = () => ok();
    tx.onerror    = () => err(tx.error);
  });
}

async function loadFromIdb(db: Database) {
  const idbDb = await idb();
  const bytes = await new Promise<Uint8Array | undefined>((ok, err) => {
    const tx = idbDb.transaction(STORE, 'readonly');
    const r  = tx.objectStore(STORE).get(KEY);
    r.onsuccess = () => ok(r.result);
    r.onerror   = () => err(r.error);
  });
  if (bytes) await db.loadSnapshot(bytes);
}
```

### Go

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

`SnapshotMeta.WalLsn` is a `*uint64`; it is `nil` for pure snapshots
and non-`nil` when you load or save a checkpoint snapshot stamped by a
WAL-enabled surface.

### Ruby

```ruby
require "lora_ruby"

db = LoraRuby::Database.create
db.execute("CREATE (:Person {name: 'Ada'})")

meta = db.save_snapshot("graph.bin")
puts "#{meta['nodeCount']} nodes, #{meta['relationshipCount']} relationships"

db2 = LoraRuby::Database.create
db2.load_snapshot("graph.bin")
```

## HTTP admin surface

`lora-server` exposes snapshot save and load as two HTTP endpoints.
Both are **opt-in**: they are mounted only when the server is started
with `--snapshot-path`.

### Enabling the endpoints

```bash
lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin
```

Without the flag, the routes return `404`. This is deliberate — the
admin surface has no authentication, and an unauthenticated admin
endpoint on a network-reachable port is a footgun. Off by default
means "never exposed by accident".

The same path can also be provided via the `LORA_SERVER_SNAPSHOT_PATH`
environment variable.

### Saving and loading

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save
# => {"formatVersion":1,"nodeCount":1024,"relationshipCount":4096,"walLsn":null,"path":"/var/lib/lora/db.bin"}

curl -sX POST http://127.0.0.1:4747/admin/snapshot/load
```

Both endpoints accept an optional `{ "path": "…" }` body that
overrides the configured default for one request — useful for ad-hoc
backups to a rotated filename:

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save \
  -H 'content-type: application/json' \
  -d '{"path": "/var/backups/lora/2026-04-24.bin"}'
```

The response includes the same four metadata fields as every other
binding, plus the `path` that was actually used.

When WAL is enabled, `POST /admin/checkpoint` writes the same snapshot
format but stamps `walLsn` with the durable WAL fence. See
[WAL and checkpoints](./wal).

### Restoring at boot

`--restore-from <PATH>` loads a snapshot once, at startup, before the
server begins accepting queries:

```bash
lora-server \
  --restore-from  /var/lib/lora/seed.bin \
  --snapshot-path /var/lib/lora/runtime.bin
```

- A missing file at boot is fine: the server logs a message and starts
  with an empty graph.
- A malformed file at boot is fatal.
- `--restore-from` is **independent** of `--snapshot-path`. You can
  restore from a read-only seed and snapshot to a writable runtime
  path, or pass the same path to both for the "boot from the last save
  every time" pattern:

```bash
lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin \
  --restore-from  /var/lib/lora/db.bin
```

### Security warning

:::caution

The admin endpoints have **no authentication**, and the optional
`path` body field is passed straight to the OS. Any client that can
reach the admin port can:

- write files anywhere the server UID can write, or
- swap the live graph by pointing `load` at an attacker-staged file.

Do not expose the admin surface on a network-reachable host without
authenticated ingress in front of it. A reverse proxy with auth, a
Unix socket, or simply not binding the port at all are all acceptable
answers. Future releases may add authentication; until then, the
correct deployment is "admin surface disabled by default, enabled only
behind an auth boundary".

See [Limitations → HTTP server](./limitations#http-server) and
[HTTP API → Admin endpoints (opt-in)](./api/http#admin-endpoints-opt-in)
for the detailed security profile.

:::

## File format (reference)

The current database snapshot format is column-oriented and will
remain readable after future format bumps until support is deliberately
dropped. Older legacy snapshots with the previous `LORASNAP` magic are
still recognized by the database loader when that legacy format is
supported.

```
[0..8)     magic         "LORACOL1"
[8..12)    format        u32 — envelope format, currently 1
[12..16)   manifest_len  u32
[16..24)   body_len      u64
[24..56)   checksum      BLAKE3(manifest || body)
[56..)     manifest      bincode-serialized manifest
[...end)   body          column-oriented graph payload
```

The manifest carries `walLsn`, node and relationship counts,
compression, encryption metadata, and the body length. The body stores
nodes, labels, relationships, relationship types, and properties in
separate columns, then applies compression and authenticated
encryption if requested.

Readers validate the magic bytes, the format version, total length,
and BLAKE3 checksum before decoding the payload. A file that fails any
of those checks is rejected — the graph in memory is never touched
until the load succeeds.

## Limitations

Worth restating, because the failure modes are where snapshots bite:

- **Manual save and restore only.** Manual snapshots run when you call
  them. Explicit WAL helpers can also write managed snapshots after N
  committed transactions, but there is no wall-clock scheduler.
- **Snapshots alone are not continuous durability.** A crash between
  saves loses every mutation in the window unless you pair snapshots
  with the [WAL](./wal).
- **Blocking.** Both save and load hold the store mutex for the full
  call; concurrent queries wait.
- **One process, one graph.** Each process you run needs its own
  snapshot file.
- **No partial or incremental snapshots.** Every save serializes the
  whole graph.
- **Admin surface is unauthenticated.** Opt-in is the only safety
  control today; put an authenticated ingress in front of it on any
  host that isn't exclusively localhost.

For the underlying engine internals (wire format, mutation-event
surface, forward-compatibility rules, atomicity guarantees on the
parent-dir fsync), see the internal
[Snapshots operator doc](https://github.com/lora-db/lora/blob/main/docs/operations/snapshots.md).

## See also

- [**Rust guide → Persisting your graph**](./getting-started/rust#persisting-your-graph)
- [**Python guide → Persisting your graph**](./getting-started/python#persisting-your-graph)
- [**Node guide → Persisting your graph**](./getting-started/node#persisting-your-graph)
- [**WASM guide → Persisting your graph**](./getting-started/wasm#persisting-your-graph)
- [**Go guide → Persisting your graph**](./getting-started/go#persisting-your-graph)
- [**HTTP server → Snapshots, WAL, and restore**](./getting-started/server#snapshots-wal-and-restore)
- [**HTTP API → Admin endpoints (opt-in)**](./api/http#admin-endpoints-opt-in)
- [**WAL and checkpoints**](./wal)
- [**Cookbook → Backup and restore**](./cookbook#backup-and-restore)
- [**Limitations → Storage**](./limitations#storage)
- [**Troubleshooting → Snapshots**](./troubleshooting#snapshots)
