# Snapshots

LoraDB can durably persist its in-memory graph to a single file and restore
it later. Snapshots are deliberately simple: one file on disk, taken
on-demand, atomic on rename — a snapshot is a point-in-time dump of
everything the database knows.

For continuous durability between snapshots, the v0.3.x core also ships a
write-ahead log: see [WAL](wal.md). When the WAL is enabled, snapshots
double as **checkpoints** — they record the WAL position they cover so
recovery replays only the records past the fence.

## File format

```
[0..8)    magic         "LORASNAP"
[8..12)   format        u32 — currently 1
[12..16)  header_flags  u32 — bit 0 = has_wal_lsn
[16..24)  wal_lsn       u64 — 0 when has_wal_lsn is unset
[24..40)  reserved      16 zero bytes
[40..)    payload       bincode-serialized SnapshotPayload
last 4B   crc32         IEEE CRC over header + payload
```

The `wal_lsn` field marks a checkpoint produced by `Database::checkpoint_to`
(or HTTP `POST /admin/checkpoint`) — it carries the WAL's `durable_lsn` at
the time the snapshot was taken so recovery knows which records the
snapshot already covers. Pure (non-checkpoint) snapshots leave
`has_wal_lsn = 0`. Readers written against format v1 transparently load
both shapes.

## API surfaces

| Context | Entry point |
|---|---|
| Rust | `Database::save_snapshot_to(path)` / `load_snapshot_from(path)` / `in_memory_from_snapshot(path)` / `checkpoint_to(path)` |
| Python | `db.save_snapshot(path)` / `db.load_snapshot(path)` |
| Node.js | `db.saveSnapshot(path)` / `db.loadSnapshot(path)` |
| Ruby | `db.save_snapshot(path)` / `db.load_snapshot(path)` |
| WASM | `db.saveSnapshotToBytes()` / `db.loadSnapshotFromBytes(bytes)` — no filesystem |
| FFI | `lora_db_save_snapshot(path)` / `lora_db_load_snapshot(path)` |
| HTTP | `POST /admin/snapshot/save` / `POST /admin/snapshot/load` / `POST /admin/checkpoint` (opt-in) |

`checkpoint_to(path)` (Rust) and `POST /admin/checkpoint` (HTTP) are
**WAL-only** entry points — they require `Database::open_with_wal` /
`--wal-dir` because they stamp the WAL's `durable_lsn` into the
snapshot header. Bindings (Python/Node.js/Ruby/WASM/FFI) do not expose
WAL or checkpoint APIs in v0.3.x; they remain snapshot-only. See
[wal.md](wal.md) for the WAL surface.

All API surfaces return a `SnapshotMeta` describing the file:

```json
{
  "formatVersion": 1,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null
}
```

## Atomicity

`save_snapshot_to` writes to `<path>.tmp`, `fsync`s the file, then renames
over the target. A crashed / interrupted save can leave a `.tmp` file
behind but can never leave a half-written file at the target path. The
parent directory is also `fsync`ed on best-effort, so the rename itself is
durable on power loss.

`load_snapshot_from` holds the store write lock for the duration of the
restore. Concurrent queries block until the restore completes; normal
read-only queries can otherwise share the store read lock.

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
for that single request.

> ⚠️ **Security.** The admin surface has no authentication today, and the
> path override means any client that can reach the admin port can write
> files anywhere the server process can write. Expose the admin routes
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
- **Schema migration.** Snapshots are versioned (format v1 today), but
  that is the *file* format, not an application-level schema. The reader
  accepts any version in
  `[SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION..=SNAPSHOT_FORMAT_VERSION]` and
  migrates legacy payloads to the current shape in-memory, so a v1 file
  keeps loading after the next format bump. Support is dropped only when
  `SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION` is deliberately raised; at that
  point you need to export via Cypher and re-import, or restore with the
  last release that still accepted the old format.

## See also

- [Graph engine internals](../architecture/graph-engine.md)
- [Known risks](../design/known-risks.md)
