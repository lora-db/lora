---
slug: index-catalog-and-accelerated-scans
title: "Index catalog, accelerated scans, and durable schema DDL"
description: "A developer-facing changelog for the latest LoraDB engine work: index DDL, catalog-backed RANGE/TEXT/POINT scans, snapshot/WAL durability, planner stats, and benchmark coverage."
authors: [loradb]
tags: [release-notes, performance, architecture, cypher]
---

The latest LoraDB engine work lands a real index catalog and wires it through
the parser, store, optimizer, executor, snapshots, WAL, and benchmarks.

This is not a shift away from the schema-free graph model. Labels,
relationship types, and properties still appear when you write them. The new
DDL gives developers an explicit way to say: keep a secondary structure for
this hot predicate.

<!-- truncate -->

## What landed

Index management is now part of the Cypher surface:

```cypher
CREATE INDEX user_email FOR (u:User) ON (u.email);
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location);
CREATE INDEX rel_since FOR ()-[r:FOLLOWS]-() ON (r.since);

SHOW INDEXES;
DROP INDEX user_email IF EXISTS;
```

The supported catalog kinds are RANGE, TEXT, POINT, and LOOKUP. RANGE is the
default for `CREATE INDEX`; TEXT and POINT activate dedicated string and spatial
candidate registries; LOOKUP records label/type token indexes in the catalog.

`IF NOT EXISTS` is idempotent across both duplicate names and equivalent index
schemas. Duplicate names surface as `22N71`, equivalent schemas as `22N70`, and
dropping a missing index without `IF EXISTS` returns `42N51`.

## Faster scan shapes

The optimizer now uses graph statistics and catalog state when it compiles a
query. Eligible scan-and-filter patterns can lower to specialized operators:

- `NodeByPropertyScan` and `NodeByPropertyRangeScan`
- `NodeByTextScan`
- `NodeByPointScan`
- `RelByPropertyRangeScan`
- `RelByTextScan`
- `RelByPointScan`

That covers node and relationship predicates such as:

```cypher
MATCH (u:User) WHERE u.age >= 18 AND u.age < 65 RETURN u;
MATCH (u:User) WHERE u.name STARTS WITH 'Al' RETURN u;
MATCH (v:Venue)
WHERE point.withinBBox(v.location, $southwest, $northeast)
RETURN v;
MATCH ()-[r:FOLLOWS]->() WHERE r.since > 2020 RETURN r;
```

The executor still refilters conservative candidate sets. TEXT indexes use a
trigram candidate path, and POINT indexes use spatial buckets, so correctness
does not depend on the index being exact.

## Durability and snapshots

Index DDL travels through the same write path as graph mutations. WAL payloads
now encode `CreateIndex` and `DropIndex` events, and recovery replays them into
the in-memory catalog before queries run.

Snapshots also carry the catalog. The `LORACOL1` envelope is now format version
2, and the snapshot body is version 3 with an index-catalog trailer. Readers
still accept the previous body format, loading older snapshots with an empty
index list.

The snapshot and WAL codecs now use a small store-owned binary codec for nested
property values and catalog records. That removes the old `bincode` dependency
from the store/snapshot path and keeps WAL and snapshots aligned on the same
catalog wire shape.

## Developer-facing improvements

Planner APIs now accept `GraphStats`, and the plan cache fingerprints catalog
and cardinality state so adding or dropping an index invalidates stale plans.
That means a query explained before `CREATE INDEX` can recompile into an
indexed scan immediately after the catalog changes.

Read-only materialization also gained an optional native `parallel` feature
through Rayon. It is default-on for native builds and disabled for WASM-style
consumers through `default-features = false`.

Benchmark coverage was reshaped around intent:

- `query_implementations` mirrors integration-test feature areas;
- `index_acceleration` compares indexed and unindexed RANGE/TEXT scenarios;
- existing `scale`, `realistic`, `wal`, `concurrent`, and
  `concurrency_guard` suites remain workload-specific tools.

## Breaking changes and migration notes

For Rust callers of `lora-compiler`, `Compiler::compile` now requires a
`&GraphStats` argument:

```rust
let compiled = Compiler::compile(&resolved, &store.graph_stats());
```

Use `GraphStats::default()` when compiling outside a store-backed runtime.

Snapshot writers now emit the newer `LORACOL1` envelope/body combination.
Current readers accept the previous body version, but older binaries should not
be expected to read snapshots written after the catalog trailer landed.

There are no data-model migrations for normal users. Existing graphs remain
schema-free; add indexes only where predicates are hot enough to justify the
extra memory.

## Notable fixes

- Plan-cache invalidation now accounts for catalog/cardinality changes.
- Catalog DDL survives WAL crash recovery.
- TEXT and POINT indexes track property updates and label/type membership
  changes.
- Indexed scans preserve already-bound node and relationship variables instead
  of rebinding them.
- Regex predicates cache compiled patterns per thread, avoiding repeated
  compilation for row-by-row `=~` filters.

## Still open

This work does not add uniqueness constraints, vector/ANN indexes, full-text
ranking, or sorted-index `ORDER BY` planning. Composite RANGE indexes are
accepted and visible in `SHOW INDEXES`, but current optimizer rewrites target
single-property predicates.
