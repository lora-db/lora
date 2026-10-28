---
title: Snapshots
sidebar_label: Snapshots
description: Manual point-in-time snapshots — save and restore the full in-memory LoraDB graph as a single file or byte payload, through every binding and the opt-in HTTP admin surface. Standalone for backups, or paired with WAL-backed recovery on every filesystem-backed surface.
---

# Snapshots

LoraDB can dump the in-memory graph to a single file and restore it
later. A snapshot is the full graph frozen from the database's current
Arc snapshot — taken on demand, atomic on rename, readable from any
binding.

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
- A **point-in-time dump.** The save loads the current Arc snapshot and
  encodes that graph consistently; readers already using an older Arc
  continue normally.
- **Atomic on rename for path-based saves.** Writes land in
  `<path>.tmp`, are `fsync`'d, then renamed over the target; a
  crashed save can leave a stale `.tmp` file but never a half-written
  target.
- **Format-versioned.** Files declare an envelope/body format version; the
  reader accepts supported versions and rejects unsupported ones rather than
  partially loading unknown data.

## What snapshots are not

The bright line, stated explicitly so it cannot be missed:

- **Not continuous durability.** A crash between two saves loses every
  mutation in the window unless you pair snapshots with the WAL on a
  filesystem-backed surface.
- **Not a wall-clock checkpoint scheduler.** Manual snapshots run when
  you call them. Explicit WAL helpers can also write managed snapshots
  after N committed transactions, but there is no built-in timer.
- **Not a WAL write.** Loading a snapshot into a WAL-backed database
  checkpoints the loaded graph into its managed snapshot directory or
  `.loradb` file before the load returns. A WAL-backed database with
  neither refuses the load with `LORA_CONFIG`; restore it at startup with
  `--restore-from` instead.
- **Not a general persistent storage layer.** There is no alternative
  backend; a snapshot is a dump of the in-memory graph, not a format
  a different engine writes into.
- **Not free.** Saves and loads are whole-graph operations. Existing
  readers can keep using their pinned Arc snapshot, but the encode/decode work
  is still proportional to graph size.
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
- **Very large graphs where save time exceeds your query window.** Save work is
  proportional to graph size and can add meaningful CPU/I/O pressure.

## Metadata

Path-based saves, load calls, and HTTP admin calls return a small
metadata record. Byte-output save APIs such as WASM `saveSnapshot()` or
Node `saveSnapshot()` return the snapshot bytes directly instead.

```json
{
  "formatVersion": 3,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null
}
```

| Field | Type | Meaning |
|---|---|---|
| `formatVersion` | integer | Envelope format of the snapshot file. `3` for every snapshot this release writes; `2` when loading one an older release wrote. |
| `nodeCount` | integer | Nodes in the saved / restored graph. |
| `relationshipCount` | integer | Relationships in the saved / restored graph. |
| `walLsn` | integer or null | The WAL fence on a WAL-backed database, for saves and checkpoints alike; `null` for a database without a WAL. |

Whenever metadata is returned, every binding uses the same four
fields; the spelling of the field names matches the wire shape
(camelCase).

## Compression and encryption

Embedded bindings can save snapshots with codec options:

| Option | Supported values |
|---|---|
| `compression` | `"none"`, `"gzip"`, `"zstd"`, `{ format: "gzip", level: 0..9 }` or `{ format: "zstd", level: 1..22 }` (zstd: native builds; wasm reads it but cannot write it) |
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
progress during the call. A large save still consumes native engine work and
filesystem I/O proportional to graph size.

### Node.js / TypeScript

```ts
import { createDatabase, type SnapshotMeta } from '@loradb/lora-node';

const db = await createDatabase(); // in-memory by default
// const db = await createDatabase('app', { databaseDir: './data' }); // container-backed
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
iterable. The native engine work still encodes or decodes the whole graph for
the duration of the save or load.

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
import lora "github.com/lora-db/lora/crates/bindings/lora-go"

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

`SnapshotMeta.WalLsn` is a `*uint64`; it is `nil` for snapshots saved
without a WAL and non-`nil` when you load or save a snapshot stamped by a
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
# => {"formatVersion":3,"nodeCount":1024,"relationshipCount":4096,"walLsn":null,"path":"/var/lib/lora/db.bin"}

curl -sX POST http://127.0.0.1:4747/admin/snapshot/load
```

Both endpoints accept an optional `{ "path": "…" }` body that
overrides the configured default for one request — useful for ad-hoc
backups to a rotated filename. Overrides are only accepted when the
server runs with `--admin-path-root <DIR>`, and must stay inside `DIR`:

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save \
  -H 'content-type: application/json' \
  -d '{"path": "/var/backups/lora/2026-04-24.bin"}'
```

The response includes the same four metadata fields as every other
binding, plus the `path` that was actually used.

When WAL is enabled, every save stamps `walLsn` with the durable WAL
fence, so the file can be recovered over the same WAL directory without
replaying writes it already holds. `POST /admin/checkpoint` writes the
same format and also marks the WAL and drops the segments the snapshot
covers; a plain save leaves the WAL alone. See
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

Snapshots are written in envelope format 3: framed records, each frame
compressed on its own so snapshots are built and read on every core. Files
declare their envelope format version and are rejected when the reader does not
support it. Format 2 (one column-oriented body) still loads; the `LORASNAP`
legacy format has been retired.

```
[0..8)     magic         "LORACOL1"
[8..12)    format        u32 — envelope format, currently 3
[12..16)   manifest_len  u32
[16..24)   body_len      u64
[24..56)   checksum      BLAKE3(manifest || body)
[56..)     manifest      explicit binary manifest
[...end)   body          framed records and catalogs (format 3), or a column-oriented payload (format 2)
```

The manifest carries `walLsn`, node and relationship counts,
compression, encryption metadata, and the body length. A format 3 body holds
the id counters and the graph's dictionaries, then nodes and relationships in
frames of 4,096 ids, then one catalog frame with the index, constraint and
vector index definitions, so `SHOW INDEXES` and `SHOW CONSTRAINTS` state
survives snapshot save/load. Each frame is compressed on its own (zstd by
default, gzip or none); encryption seals the whole body. A format 2 body stores
the same graph in separate columns (body versions 2 to 5); older catalog-free
bodies still load with empty index and constraint lists.

Readers validate the magic bytes, the format version, every length and
the BLAKE3 checksum, and build the loaded graph beside the current one.
A file that fails any check is rejected, and the graph in memory is not
touched until the load succeeds. Snapshots may come from uploads, so a
reader never allocates from a length the file states: a frame claiming
more than 1 GiB, inflating to a different size than it declares, or
holding bytes after its compressed data is refused.

## Limitations

Worth restating, because the failure modes are where snapshots bite:

- **Manual save and restore only.** Manual snapshots run when you call
  them. Explicit WAL helpers can also write managed snapshots after N
  committed transactions, but there is no wall-clock scheduler.
- **Snapshots alone are not continuous durability.** A crash between
  saves loses every mutation in the window unless you pair snapshots
  with the [WAL](./wal).
- **Whole-graph work.** Save and load encode/decode the entire graph. Existing
  readers can continue on already-loaded Arc snapshots, but new queries see the
  restored graph only after load publishes it.
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
