# ADR-0003: Snapshot Format

## Status

Accepted, updated for the current `LORACOL1` columnar snapshot codec.

## Context

LoraDB needs a way to persist the full in-memory graph, move snapshots across
bindings, and pair point-in-time files with WAL checkpoint fences. The format
must reject torn/corrupt files, support atomic path-based saves, and leave room
for compression and encryption.

## Decision

Snapshots use the `lora-snapshot` codec and the current envelope magic
`LORACOL1`.

```text
[0..8)    magic         "LORACOL1"
[8..12)   format        u32 envelope format
[12..16)  manifest_len  u32
[16..24)  body_len      u64
[24..56)  checksum      BLAKE3(manifest || body)
[56..)    manifest      bincode-serialized manifest
[...]     body          columnar graph payload
```

The manifest stores:

- envelope format version
- optional `wal_lsn`
- node and relationship counts
- compression mode
- encryption metadata
- body length

The body stores graph payload data in the snapshot crate's current body format.
Default Rust options use gzip level 1 and no encryption. JSON option surfaces can
also request no compression, gzip, raw-key encryption, or password-based
ChaCha20-Poly1305 encryption.

Path saves write to `<path>.tmp`, fsync the file, rename over the target, and
best-effort fsync the parent directory. Loads decode into a fresh
`InMemoryGraph` and publish it by swapping the database's current `Arc`.

## Checkpoints

Pure snapshots have `wal_lsn = null`. A checkpoint snapshot records the WAL's
durable LSN, then recovery replays only committed records above that fence.
`Database::checkpoint_to`, managed snapshot checkpoints, and HTTP
`POST /admin/checkpoint` all use this field.

## Consequences

- Snapshot files are a whole-graph artifact, not an incremental storage engine.
- The current live graph is not partially replaced on a failed load.
- Corruption is caught by magic/version/length/checksum validation before decode.
- Changing the envelope or body layout requires an explicit compatibility plan in
  `crates/lora-snapshot/src/format.rs` and docs updates.
- Indexes are rebuilt from payload data on load; in-memory index layout changes
  are not automatically file-format changes unless the payload changes.

## See also

- [Snapshots](../operations/snapshots.md)
- [WAL](../operations/wal.md)
- [Change Management](../design/change-management.md#snapshot-format-compatibility)
- [Value Model](../internals/value-model.md#serialization-stability)
