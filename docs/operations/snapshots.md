# Snapshots

LoraDB can durably persist its in-memory graph to a single file and restore
it later. Snapshots are deliberately simple: one file on disk, taken
on-demand, atomic on rename — a snapshot is a point-in-time dump of
everything the database knows.

For continuous durability between snapshots, the core also ships a
write-ahead log: see [WAL](wal.md). When the WAL is enabled, snapshots
double as **checkpoints** — they record the WAL position they cover so
recovery replays only the records past the fence.

## File format

```
[0..8)    magic         "LORACOL1"
[8..12)   format        u32: envelope format, 3 (framed body) or 2 (columnar body)
[12..16)  manifest_len  u32
[16..24)  body_len      u64
[24..56)  checksum      BLAKE3 over manifest + body
[56..)    manifest      explicit binary manifest
[...]     body          framed (format 3) or columnar (format 2) body
```

The manifest carries the snapshot format version, optional `wal_lsn`, node and
relationship counts, compression mode, encryption metadata, and body length.

### Format 3: framed body

Every save, checkpoint and `sync()` through `Database` writes format 3. The body starts with the id counters and the
graph's dictionaries (labels, relationship types, property keys, and the short
string values the graph stores by number), then holds the records in frames of
4,096 ids each: node frames, then relationship frames, then one catalog frame
(index definitions, constraint definitions, vector index trailer), then an end
marker. Each frame is compressed on its own (zstd, raw deflate for gzip, or
stored as is), so frames are built and decoded on every core:

- **Writing** (`sync()`, checkpoints, `save_snapshot_to*` and
  `save_snapshot_to_bytes*`, so every binding's `saveSnapshot`) encodes and compresses
  a round of frames on the thread pool while the calling thread writes the
  round before.
- **Reading** (opening a named or snapshot-backed database, `recover`,
  `load_snapshot_from*` and `in_memory_from_snapshot`, so every binding's
  `loadSnapshot` and the admin restore) decodes a round of
  frames into the store's record encoding on the thread pool while the calling
  thread files the round before into the graph and reads the round after. The
  catalogs are then registered, and every index population and constraint scan
  runs at once, each also split over the cores (see
  [Restart cost](#restart-cost)).

A frame records its record count, raw length and stored length; the reader
checks every length, that ids ascend within a frame, that frames come in order,
and the envelope checksum over the whole body before the graph is used.

A snapshot can come from an upload, so no length in it sizes an allocation:
frames inflate as a stream into a buffer that grows only as bytes really
inflate, and a frame that inflates to more or less than its raw length, or has
bytes after its compressed stream, is refused (by the native and the wasm zstd
decoder alike). The writer starts a new frame once one holds 64 MiB of raw
records, and the reader refuses a frame claiming more than 1 GiB; a round reads
ahead at most 256 MiB of raw frames, so a crafted file cannot make a reader hold
much more than a gigabyte before its records are checked. A single record
larger than 1 GiB cannot be snapshotted and fails the save. A node listing one
label twice is refused, in either format.
Encryption seals the whole body, as in format 2. The layout is documented in
`crates/lora-snapshot/src/chunked.rs`.

Builds without threads (wasm, or `default-features = false`) write and read the
same format on one thread.

### Format 2: columnar body

Snapshots written before format 3 hold one column-major body, compressed
whole. They still load, on one thread. A named database opened from one
checkpoints before the open returns, so its first open after an upgrade takes
one `sync()` longer and the opens after it read format 3. That rewrite is an
optimisation: if it fails (a full disk, a read-only mount, a container at
`max_database_bytes`) the open logs a warning and succeeds, and
`compact_on_open_wal_bytes = u64::MAX` turns it off with open compaction. Once
rewritten, a database no longer opens in a release before 0.34, so back up the
`.loradb` file before upgrading if you may need to roll back. Managed snapshot
directories are rewritten as format 3 by their next checkpoint. The columnar body has its own version, tracked in
`crates/lora-snapshot/src/format.rs`; each version appends a trailer:

| Body version | Adds |
|---|---|
| v2 | Base columnar body (no trailers) |
| v3 | Index catalog trailer, so snapshots preserve `CREATE INDEX` / `DROP INDEX` state |
| v4 | Constraint catalog trailer |
| v5 | Vector-index backend trailer: HNSW graph state, JSON-encoded |

The reader accepts v2 through v5; a missing trailer loads as empty (no indexes,
no constraints, vector backends rebuilt from the property store). Only the
payload encoder in `lora-snapshot` (`encode_snapshot` and `write_snapshot`,
given a `SnapshotPayload` rather than a graph) still writes format 2, for gzip
or no compression. `decode_snapshot` reads both formats into a payload; the
`Database` loads read both straight into a graph.

### Restart cost

Opening a database reads its snapshot and replays the WAL past the snapshot's
fence. With format 3 the snapshot part scales with the cores: on a 10-core
machine a named database of 3M nodes and 3M relationships with a uniqueness
constraint (630 MB in memory) opened in about 0.75 s, against about 7.7 s for
the same graph from a format 2 snapshot, and `sync()` took about 0.35 s instead
of 4.7 s. Measure your own with
`cargo run --release -p lora-database --example open_probe -- <dir>`.

Workers decode frames into whole leaves of the record store, which the
loading thread files by pointer, and link a round's relationships into the
adjacency lists leaf by leaf on every core. Each declared index is built split
over the cores, and so is a constraint's scan of the existing data; a
constraint on one property (other than a type constraint) reads each value in
place. What a restart still does on one thread: reading the file and checking
its checksum, and replaying the WAL tail. Replay applies about 85 MB of WAL a
second; a named database checkpoints once its WAL reaches 64 MB or a quarter
of the snapshot's size, whichever is larger, so a restart replays at most
about a quarter snapshot of WAL (`open_probe <dir> wal <MB>` measures it).
Peak memory over the graph is a few rounds of frames (a few megabytes per core)
and, while a uniqueness constraint is checked, 16 bytes per constrained entity.

A named database's container (`.loradb`) stores a format 3 snapshot as is: its
frames are compressed already, and compressing the container frame again made
every open inflate the whole snapshot on one thread.

### Compression

Snapshots are compressed with zstd level 3 by default: in the comparison below
a quarter smaller than gzip level 1 and as fast to write and read. Gzip (levels
0 to 9) and no compression remain available; zstd levels go from 1 to 22. Wasm
builds read zstd snapshots (with a pure-Rust decoder) but write gzip level 1,
their default; a snapshot written by a native build therefore loads in a
browser. zstd compresses format 3 snapshots only: the payload encoder
(`encode_snapshot`) writes format 3 when asked for zstd, which its default
options are on native builds. Byte and file saves (`saveSnapshot`) are
uncompressed unless asked otherwise, so they stay quick to write; managed
snapshot directories and named databases use zstd in every binding.

The 3M-node database above, per codec, 4,096 ids a frame:

| Codec | Size | Write | Load |
|---|---|---|---|
| none | 250.6 MiB | 0.49 s | 1.34 s |
| gzip 1 | 65.5 MiB | 0.34 s | 1.27 s |
| gzip 6 | 56.2 MiB | 1.16 s | 1.23 s |
| zstd 1 | 57.7 MiB | 0.30 s | 1.21 s |
| **zstd 3** (default) | **50.6 MiB** | **0.30 s** | **1.22 s** |
| zstd 6 | 49.2 MiB | 0.50 s | 1.22 s |
| zstd 9 | 49.1 MiB | 0.60 s | 1.23 s |

(Measured before the later restore work; the codec's share of a load is the
same.) Compare codecs on your own database with
`cargo run --release -p lora-database --example codec_probe -- <file.loradb> <work dir>`,
which copies the file and never opens the original.

`wal_lsn` is the WAL fence: the WAL's durable LSN at the time the snapshot was
taken, so recovery knows which records the snapshot already covers. Every
snapshot of a WAL-backed database carries it: checkpoints
(`Database::checkpoint_to`, managed snapshot checkpointing, HTTP
`POST /admin/checkpoint`) and plain saves (`save_snapshot_to`,
`save_snapshot_to_bytes` and their `_with_options` forms, every binding's
`saveSnapshot`, HTTP `POST /admin/snapshot/save`). A save fsyncs the WAL and
reads its durable LSN under the writer lock, so the graph and the fence match:
`Database::recover(snapshot, wal)` replays exactly the writes made after the
save. Writers wait for that fsync (and for an open write transaction to
finish), not for the snapshot encode. Unlike a checkpoint, a save appends no
`Checkpoint` marker and truncates no WAL segments.

A database without a WAL writes `wal_lsn = null` (a pure snapshot). So does a
WAL-backed database whose WAL is poisoned, with a warning in the log: its graph
may no longer match the log, so that snapshot is a seed for a fresh WAL
directory, not something to recover over the old WAL. Recovery treats
`wal_lsn = null` as fence 0 and replays the whole WAL on top of it, which is
right for a seed over a new WAL directory (`lora-server --restore-from seed.bin
--wal-dir <empty dir>`) and fails with `WalError::Gap` over a WAL whose early
segments a checkpoint removed.

Default Rust snapshot options use zstd level 3 (gzip level 1 on wasm) with no
encryption. The JSON options surfaces used by bindings/admin helpers accept no
compression, `"gzip"` (level 0 to 9, default 1) or `"zstd"` (level 1 to 22,
default 3), plus raw 32-byte key or password-based ChaCha20-Poly1305
encryption.

## API surfaces

| Context | Entry point |
|---|---|
| Rust | `Database::save_snapshot_to(path)` / `save_snapshot_to_with_options(path, options)` / `load_snapshot_from(path)` / `in_memory_from_snapshot(path)` / `checkpoint_to(path)` / managed snapshot constructors |
| Python | `db.save_snapshot(path \| "binary" \| "base64" \| writer)` / `db.load_snapshot(source)` where `source` can be a path, bytes-like object, base64 string, or binary reader |
| Node.js | `db.saveSnapshot(path \| "binary" \| "base64" \| options)` / `db.loadSnapshot(source)` where `source` can be a path, `file:`/HTTP(S)/`data:` URL, `Buffer`, `Uint8Array`, `ArrayBuffer`, Node `Readable`, web `ReadableStream`, or async iterable of byte chunks |
| Ruby | `db.save_snapshot(path)` / `db.load_snapshot(path)` |
| WASM | `db.saveSnapshot("binary" \| "base64" \| "blob" \| "download" \| options)` / `db.loadSnapshot(source)` where `source` can be `Uint8Array`, `ArrayBuffer`, `Blob`/`File`, `Response`, web `ReadableStream`, string URL, or `URL` — no filesystem or Node `Buffer` |
| FFI | `lora_db_save_snapshot(path)` / `lora_db_save_snapshot_to_bytes()` / `lora_db_load_snapshot(path)` / `lora_db_load_snapshot_from_bytes()` |
| HTTP | `POST /admin/snapshot/save` / `POST /admin/snapshot/load` / `POST /admin/checkpoint` (opt-in) |

`checkpoint_to(path)` (Rust) and `POST /admin/checkpoint` (HTTP) are
**WAL-only** entry points — they require `Database::open_with_wal` /
`--wal-dir` because they stamp the WAL's durable LSN into the snapshot
manifest. Node, Python, Go, and Ruby can open WAL-backed databases and managed
snapshot directories; WASM remains snapshot-only and pathless. See
[wal.md](wal.md) for the WAL surface.

Loading a snapshot into a WAL-backed database replaces the graph without
writing WAL records, so the load is made durable as a checkpoint before it
becomes visible: into the managed snapshot directory, or into the container
of a named `.loradb` database. A WAL-backed database with neither (plain
`open_with_wal` / `--wal-dir`) refuses the load with `LORA_CONFIG`, because a
restart would replay the old WAL instead of the loaded graph. Restore such a
database with `Database::recover` / `--restore-from` at startup.

All API surfaces return a `SnapshotMeta` describing the file:

```json
{
  "formatVersion": 3,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null
}
```

The TypeScript bindings expose two deliberately different source types:
`NodeSnapshotSource` for `lora-node`, which includes filesystem paths,
Node `Buffer`, Node streams, web streams, and async iterables, and
`WasmSnapshotSource` for `lora-wasm`, which accepts web binary objects,
web streams, and fetchable URLs but does not include `Buffer`.

The save side is typed separately as well. Node's `saveSnapshot("binary")`
returns a `Buffer`, `saveSnapshot("base64")` returns a string, and path saves
return `SnapshotMeta`. WASM's `saveSnapshot()`/`"binary"` returns
`Uint8Array`, `"base64"` returns a string, `"blob"` returns a `Blob`, and
`"download"` creates a browser download when a document is available.
Python mirrors the shape with `bytes`, base64 `str`, and binary file-like
objects. Go exposes explicit helpers: `SaveSnapshotBytes`,
`SaveSnapshotBase64`, `SaveSnapshotTo`, `LoadSnapshotBytes`,
`LoadSnapshotBase64`, and `LoadSnapshotFrom`.

## Atomicity

`save_snapshot_to` writes to `<path>.tmp`, `fsync`s the file, then renames
over the target. A crashed / interrupted save can leave a `.tmp` file
behind but can never leave a half-written file at the target path. The
parent directory is also `fsync`ed on best-effort, so the rename itself is
durable on power loss.

`load_snapshot_from` decodes the file into a fresh `InMemoryGraph` and then
publishes it by swapping the database's current `Arc`. Readers that already
loaded the previous `Arc` keep seeing that previous snapshot until they finish;
new queries see the restored graph.

## The HTTP admin surface

`/admin/snapshot/{save,load}` are **opt-in** — they do not exist unless the
server is started with `--snapshot-path <PATH>` (or
`LORA_SERVER_SNAPSHOT_PATH`). With no snapshot path configured, the
router simply does not mount the admin routes, and requests to them return
404.

Once enabled, both endpoints accept an optional JSON body:

```json
POST /admin/snapshot/save
Content-Type: application/json

{ "path": "/custom/backup/today.bin" }
```

When the body is omitted (or `path` is missing), the server uses the path
from `--snapshot-path`. When `path` is supplied, it overrides the default
for that single request. Overrides are rejected unless the server runs
with `--admin-path-root <DIR>`, and must then stay inside `DIR`.

> ⚠️ **Security.** The admin surface has no authentication today, and the
> admin routes let any client that can reach the admin port overwrite the
> snapshot, replace the live graph, and (with `--admin-path-root`) write
> files inside that directory. Expose the admin routes
> behind an authenticated ingress (or a Unix socket, or nothing at all on a
> network-reachable host). Future releases may add authentication; until
> then, the correct deployment is "admin surface disabled by default, and
> operators opt in only behind an auth boundary".

## Restoring at boot

Pass `--restore-from <PATH>` to the server to have it load the snapshot at
that path before accepting queries. A missing file is fine — the server
starts with an empty graph and logs a message. A malformed file is fatal.

`--restore-from` is independent of `--snapshot-path`: operators can restore
from a read-only location (`/var/lib/lora/seed.bin`) and snapshot to a
writable one (`/var/lib/lora/runtime.bin`). When the same path is passed
to both, the server boots from the last-saved snapshot every time.

## Mutation events

Alongside snapshots, `lora-store` emits per-mutation events to an optional
[`MutationRecorder`] observer. Each event variant mirrors one method on
`GraphStorageMut` and carries exactly the information needed to replay the
mutation against another store:

```rust
pub enum MutationEvent {
    CreateNode { id, labels, properties },
    CreateRelationship { id, src, dst, rel_type, properties },
    SetNodeProperty { node_id, key, value },
    // ...one variant per GraphStorageMut method...
    Clear,
}
```

This is the vocabulary the [WAL](wal.md) appends to its segment files.
Today the recorder is `None` by default and the emit path is one
null-pointer check per mutation (no event construction, no clone) —
operators who don't enable the WAL pay nothing for it. Other use cases
that benefit from the same hook (audit streams, change-data-capture,
replication) install a recorder via
`InMemoryGraph::set_mutation_recorder`.

## What snapshots do not solve

- **Continuous durability.** A crash between snapshot saves loses every
  mutation in the window. Enable the [WAL](wal.md) (`--wal-dir`) for
  that — the WAL fills the gap between snapshots, and `checkpoint_to`
  / `POST /admin/checkpoint` pairs the two together.
- **Multi-tenant isolation.** Each server process holds one graph; if you
  run several, each has its own snapshot file.
- **Schema migration.** Snapshots are versioned, but that is the snapshot
  envelope/body format, not an application-level schema. The current reader
  accepts `LORACOL1` envelope formats 2 and 3 and rejects other format
  versions. Treat format changes as compatibility events that need a reader
  path or an export/import migration.

## See also

- [Graph engine internals](../architecture/graph-engine.md)
- [Known risks](../design/known-risks.md)
