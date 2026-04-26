# ADR-0003: Snapshot Format

## Status

Accepted (shipped in commit `ed26637`, "feat(snapshots): load and save snapshot support")

## Context

LoraDB needed a way to survive process restarts without blocking on a full write-ahead log, recovery engine, and replication story. The bar was:

1. No dependency on a third-party durability crate (sled, redb, sqlite, …).
2. Atomic against crashes — a killed save can never produce a half-written target file.
3. Compatible with the existing single-lock concurrency model of `InMemoryGraph`.
4. Forward-compatible with a future WAL / checkpoint hybrid without a second file-format migration.
5. Cheap enough to run as a cron-driven admin call against a live server.

Decisions needed:

1. Point-in-time snapshots vs continuous durability (WAL).
2. Wire format — hand-rolled schema vs a structured serializer.
3. Integrity checking on read.
4. How to leave room for a future WAL without breaking v1 readers.
5. How mutations become observable to a future durability layer.

## Decision

### Single-file point-in-time snapshots

`Database::save_snapshot_to(path)` writes the full in-memory graph — nodes, relationships, ID counters — to a single file on disk. `load_snapshot_from(path)` and `in_memory_from_snapshot(path)` restore it. There is no WAL, no incremental persistence, no background snapshot loop in the open-source core today.

### Wire format

Every snapshot file is laid out as:

```
[0..8)    magic         "LORASNAP"
[8..12)   format        u32 — currently SNAPSHOT_FORMAT_VERSION (1)
[12..16)  header_flags  u32 — bit 0 = has_wal_lsn
[16..24)  wal_lsn       u64 — 0 when has_wal_lsn is unset
[24..40)  reserved      16 zero bytes
[40..)    payload       bincode-serialized SnapshotPayload
last 4B   crc32         IEEE CRC over header + payload
```

Indexes (label, relationship-type, adjacency) are **not** serialized — they are fully derivable from the records and are rebuilt on load.

### Bincode payload

`SnapshotPayload` is bincode-serialized. It contains `Vec<NodeRecord>`, `Vec<RelationshipRecord>`, the two ID counters, and whatever auxiliary state the store needs to re-derive its indexes. Every `PropertyValue` variant — including temporal, spatial, and vector types — flows through the same bincode path.

### Atomic write protocol

`save_snapshot_to` writes to `<path>.tmp`, `fsync`s the file, renames over the target, and best-effort `fsync`s the parent directory. A crashed save can leave a `.tmp` file behind but can never leave a half-written file at the target path.

### Reader tolerance

The reader accepts any format version in `[SNAPSHOT_MIN_SUPPORTED_FORMAT_VERSION..=SNAPSHOT_FORMAT_VERSION]`. Legacy payloads are migrated to the current in-memory shape on load. The constants live in `crates/lora-store/src/snapshot.rs`.

### WAL seam

The `wal_lsn: u64` field and `header_flags` bit 0 (`has_wal_lsn`) are reserved for a future WAL / checkpoint hybrid. A checkpoint is simply a snapshot with `has_wal_lsn = 1`. v1 readers tolerate checkpoints produced by future code by design.

### Mutation events as a sibling surface

Alongside the snapshot surface, `lora-store` defines a `MutationEvent` enum — one variant per `GraphStorageMut` method — and a `MutationRecorder` trait. Stores hold an optional recorder (`None` by default); when set, every mutation fires an event that carries exactly the information needed to replay it against another store. This is the vocabulary a future WAL will append to a log file. The no-recorder hot path is a single null-pointer check — no event construction, no clone.

## Rationale

- **Point-in-time vs WAL.** Shipping snapshots alone avoids committing to a durable log, replay engine, recovery protocol, and compaction strategy on day one. Admin-surface snapshots plus a cron cover the 80% case immediately. Adding WAL later is a strict superset — the reserved `wal_lsn` field and the `MutationEvent` surface are the seams.

- **Bincode over a hand-rolled schema.** `lora-store` already defines `NodeRecord`, `RelationshipRecord`, `PropertyValue`, and the temporal / spatial / vector types. Bincode is a one-line derive that matches those shapes exactly, so there is no parallel on-disk schema to keep in sync. The trade-off is that the payload format is coupled to the Rust struct layout; mitigated by the explicit format-version contract.

- **CRC32 trailer.** Cheap to compute, catches truncation and bit-flips, and surfaces corruption as a loud failure at load rather than a partial / silent load. Snapshots are not large enough for CRC32 collision probability to matter.

- **Reserved `wal_lsn` + `header_flags` bit 0.** Forward-compat seam. Writers today always set bit 0 = 0; future WAL-aware writers produce files that today's v1 readers still open and display as "not a checkpoint". Adding more header-flag bits is a safe forward-compatible change provided unknown bits are ignored on read.

- **Rebuilding indexes on load.** Indexes are fully derivable from records, so not serializing them (a) saves disk bytes, and (b) means a later change to the in-memory index layout does not require a format-version bump. The load-time cost is the same `O(n + r)` we already pay at graph insertion.

- **Mutation events as a sibling surface, not a mode.** Keeping WAL / CDC / audit / replication on one vocabulary (`MutationEvent`) avoids the pattern where each durability feature re-invents its own event shape. Making the recorder optional (null-pointer check) means workloads that do not care pay zero cost.

## Consequences

- A crash between saves loses every mutation since the last save. Operators must either snapshot frequently enough to tolerate the gap or (when it ships) enable the WAL.
- `save_snapshot_to` acquires the store read lock while bincode serializes the payload; `load_snapshot_from` holds the write lock for the full deserialize + index-rebuild window. Loads block other queries, and long saves can delay writers. See [performance/notes.md](../performance/notes.md) for the practical implications.
- Any change to `PropertyValue`, the record types, or the temporal / spatial / vector layouts is a wire-format change. The format-version / compatibility policy lives in [../design/change-management.md](../design/change-management.md#snapshot-format-compatibility), and the value-level stability rules in [../internals/value-model.md](../internals/value-model.md#serialization-stability).
- The HTTP admin surface (`POST /admin/snapshot/{save,load}`) ships with no authentication today. The canonical warning lives in [../operations/security.md](../operations/security.md#admin-surface); see also [../operations/snapshots.md](../operations/snapshots.md#the-http-admin-surface).
- Adding a new `GraphStorageMut` method requires adding a matching `MutationEvent` variant or the durability layer silently drops the mutation. Checklisted in [../internals/cypher-development.md](../internals/cypher-development.md).

## Alternatives considered

- **Embedded KV store (sled / redb / sqlite).** Rejected: adds a heavyweight dependency, introduces a second write model to reason about, and forces a durability-aware query path before we know what a production workload looks like. The current design can still migrate to one of these later if needed — bincode is only the *payload* format; the file-level framing is independent.

- **Hand-rolled on-disk schema per record type.** Rejected: large upfront engineering cost, and still needs its own migration story. Bincode with a format-version constant achieves the same outcome with less code.

- **WAL on day one.** Rejected: defers any form of persistence by weeks and constrains the storage API before we know the production shape. The reserved `wal_lsn` seam keeps WAL as a strictly additive future change.

- **Log-structured merge format.** Rejected as out-of-scope for an in-memory engine whose working-set assumption is "it fits in RAM".

- **Protobuf / FlatBuffers / Cap'n Proto payload.** Rejected: schema files would duplicate types already defined in Rust, and none of those serializers has meaningfully different properties for this workload. Bincode matches the existing value model one-to-one.

- **No CRC, rely on the filesystem.** Rejected: the atomic rename protects against torn writes but not against bit-rot, media errors on read, or partial reads from network-backed volumes.

## Related

- [Snapshots (operator doc)](../operations/snapshots.md) — file format table, admin surface, atomicity guarantees.
- [Graph engine → Durability](../architecture/graph-engine.md#durability) — where the trait lives in the store crate.
- [Change management → Snapshot format compatibility](../design/change-management.md#snapshot-format-compatibility) — rules for bumping `SNAPSHOT_FORMAT_VERSION`.
- [Value model → Serialization stability](../internals/value-model.md#serialization-stability) — which type changes are wire-incompatible.
- [ADR-0001: Graph Architecture](0001-graph-architecture.md) — the BTreeMap-backed in-memory store this ADR extends.
