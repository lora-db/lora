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

- `Vec<Option<Arc<NodeRecord>>>` for nodes
- `Vec<Option<Arc<RelationshipRecord>>>` for relationships
- `Vec<Vec<RelationshipId>>` for outgoing and incoming adjacency
- `BTreeMap<String, Vec<NodeId>>` for labels
- `BTreeMap<String, Vec<RelationshipId>>` for relationship types
- lazy exact-match property indexes for indexable property values
- an explicit index catalog plus RANGE/TEXT/POINT backing registries for
  declared secondary indexes

IDs are monotonic `u64`s and are never reused. Deletes leave tombstones in the
slot vectors. Records are held behind `Arc` so database snapshots and staged
writes can share unchanged records; mutations use copy-on-write for touched
records.

### Database-level snapshot publication

`lora-database` publishes the current store through `ArcSwap`. Read-only
auto-commit queries load an `Arc<InMemoryGraph>` and execute without holding a
store lock. Mutating auto-commit queries stage a clone, buffer mutation events,
serialize commit publication through the writer mutex, append WAL records when
configured, and swap in the new `Arc`.

Explicit read-only transactions pin a snapshot. Explicit read-write
transactions hold the writer mutex until commit or rollback.

### Storage trait layering

The storage API is split into:

- `GraphStorage` for reads, scans, expansion, and default helpers.
- `GraphCatalog` for the analyzer's narrow count/name/property-key checks.
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
  FULLTEXT indexes now exist; vector procedures still use flat scans rather
  than ANN execution.
- Bulk compatibility APIs that return owned records still allocate; executor hot
  paths use borrowed closure hooks where possible.

## See also

- [Graph Engine](../architecture/graph-engine.md)
- [Data Flow](../architecture/data-flow.md)
- [Value Model](../internals/value-model.md)
- [WAL](../operations/wal.md)
