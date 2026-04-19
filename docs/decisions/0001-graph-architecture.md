# ADR-0001: Graph Architecture

## Status

Accepted (inferred from implementation)

## Context

The project needed a graph storage engine to back a Cypher query engine. The key decisions were:

1. In-memory vs persistent storage
2. Data structure choices for the graph
3. Storage trait design

## Decision

### In-memory BTreeMap-based storage

The graph is stored entirely in memory using `BTreeMap` collections:

- `BTreeMap<NodeId, NodeRecord>` for nodes
- `BTreeMap<RelationshipId, RelationshipRecord>` for relationships
- `BTreeMap<NodeId, BTreeSet<RelationshipId>>` for outgoing and incoming adjacency
- `BTreeMap<String, BTreeSet<NodeId>>` for label indexes
- `BTreeMap<String, BTreeSet<RelationshipId>>` for relationship type indexes

### Trait-based abstraction

Storage is abstracted behind two traits:

- `GraphStorage` -- read-only operations (scan, lookup, expand, introspection)
- `GraphStorageMut` -- extends `GraphStorage` with create, update, delete

This separation allows the analyzer and read-only executor to work with `&dyn GraphStorage` while the mutable executor requires `&mut dyn GraphStorageMut`.

### Monotonic ID allocation

Node and relationship IDs are sequential `u64` values that are never reused.

## Rationale

- **BTreeMap** provides deterministic iteration order, which simplifies testing and debugging. The performance trade-off (O(log n) vs O(1)) is acceptable for an in-memory engine where the constant factors are small.
- **Trait abstraction** enables future alternative implementations (e.g., persistent storage, sharded storage) without changing the query engine.
- **In-memory** avoids complexity of persistence, WAL, recovery, and crash safety. Appropriate for a project focused on the Cypher language implementation.
- **Monotonic IDs** are simple, fast, and avoid stale reference bugs. The trade-off is that IDs are not reused after deletion, but `u64` overflow is not a practical concern.

## Consequences

- Data is lost on process exit
- No concurrent reads (could be mitigated with `RwLock`)
- Clone-heavy API (methods return `Vec<Record>` rather than iterators)
- No property indexes (property-based queries require scans)
- `BTreeMap` is slightly slower than `HashMap` for point lookups but provides ordered iteration

## Alternatives considered (inferred)

- **HashMap** -- faster lookups but non-deterministic iteration; could be considered for performance optimization
- **SlotMap / Arena** -- more cache-friendly than BTreeMap for entity storage; would require a different deletion strategy
- **Adjacency list on nodes** -- storing outgoing/incoming relationships directly on `NodeRecord` instead of separate index maps; rejected in favor of decoupled indexes
