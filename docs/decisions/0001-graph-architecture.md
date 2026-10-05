# ADR-0001: Graph Architecture

## Status

Accepted, updated to match the current implementation.

## Context

LoraDB needs a graph storage engine for a Cypher-like query engine. The current
core is intentionally in-memory and single-process, while durability is layered
around the store through snapshots, WAL, and named containers.

Key forces:

1. Fast point lookup by `NodeId` / `RelationshipId`.
2. Deterministic catalog ordering for labels, relationship types, and map keys.
3. Cheap read snapshots for concurrent read-only queries.
4. A mutation vocabulary that can feed WAL, recovery, container mirrors, and future
   CDC-style consumers.
5. A backend trait surface that does not force every implementation to expose
   borrowed records.

## Decision

### Slot-indexed in-memory storage

`InMemoryGraph` stores primary records in slot vectors:

- `ChunkedVec<Option<Arc<NodeRecord>>>` for nodes
- `ChunkedVec<Option<Arc<RelationshipRecord>>>` for relationships
- `ChunkedVec<SmallVec<RelationshipId, 2>>` for outgoing and incoming adjacency
- `BTreeMap<String, ChunkedVec<NodeId>>` for labels
- `BTreeMap<String, ChunkedVec<RelationshipId>>` for relationship types
- lazy exact-match property indexes for indexable property values
- an explicit index catalog plus RANGE/TEXT/POINT/FULLTEXT/VECTOR backing
  registries for declared secondary indexes, and a constraint catalog

`ChunkedVec` is a `Vec` split into `Arc`-shared chunks, so a write copies only
the chunks it touches (added 2026-09-29, `d004ba4e`, together with
copy-on-write secondary indexes). The chunks hang off a two-level persistent
radix tree (512-entry leaves, 128 leaves per interior node, one shared root), so
a clone is one refcount bump instead of one per chunk (2026-10-05).

IDs are monotonic `u64`s and are never reused. Deletes leave tombstones in the
slot vectors. Records are held behind `Arc` so database snapshots and staged
writes can share unchanged records; mutations use copy-on-write for touched
records.

### Database-level snapshot publication

`lora-database` publishes the current store through a `RwLock<Arc<S>>`
(`live_store.rs`; an earlier `ArcSwap` was replaced because its internal `Arc`
defeated `Arc::make_mut`). Read-only auto-commit queries clone the
`Arc<InMemoryGraph>` under a brief read lock and execute without holding a
store lock. Mutating auto-commit queries take the writer mutex, stage a clone
(or, for plans that cannot fail midway, mutate the live graph in place),
buffer mutation events, append WAL records when configured, and publish the
new `Arc`.

Explicit read-only transactions pin a snapshot. Explicit read-write
transactions hold the writer mutex until commit or rollback.

### Storage trait layering

The storage API is split into:

- `GraphStorage` for reads, scans, expansion, and default helpers.
- `GraphCatalog` for narrow count/name/property-key checks. (The analyzer
  originally validated labels, types and keys through it; since 2026-09-29,
  `d004ba4e`, it no longer reads the graph at all.)
- `BorrowedGraphStorage` for backends that can expose borrowed records.
- `GraphStorageMut` for primitive writes, deletes, property/label helpers, and
  `clear`.

### Mutation events

Every primitive write emits a `MutationEvent` when a `MutationRecorder` is
installed. The recorder is optional and absent by default. The WAL uses this
vocabulary for committed mutation batches; the same event stream is suitable for
audit, CDC, and replication work later.

## Consequences

- Read-only auto-commit queries can overlap on immutable snapshots.
- Write commits and explicit read-write transactions still serialize.
- Point lookup by ID is direct, but tombstones mean slot vectors can grow after
  heavy delete/create workloads.
- Exact-match property lookups on indexable values can use internal lazy
  indexes, and declared RANGE/TEXT/POINT/LOOKUP indexes can guide the
  optimizer for scoped predicates. Constraint DDL, VECTOR indexes, and
  FULLTEXT indexes now exist. Vector indexes default to an exact flat scan;
  approximate (HNSW) search shipped in v0.12.0 and is opt-in per index via
  `vector.indexProvider: 'hnsw'`.
- Bulk compatibility APIs that return owned records still allocate; executor hot
  paths use borrowed closure hooks where possible.

## See also

- [Graph Engine](../architecture/graph-engine.md)
- [Data Flow](../architecture/data-flow.md)
- [Value Model](../internals/value-model.md)
- [WAL](../operations/wal.md)
