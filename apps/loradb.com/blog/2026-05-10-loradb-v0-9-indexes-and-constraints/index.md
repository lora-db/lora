---
slug: loradb-v0-9-indexes-and-constraints
title: "LoraDB v0.9: indexes, constraints, and a real schema catalog"
description: "LoraDB v0.9 adds a first-class index and constraint catalog: RANGE/TEXT/POINT/LOOKUP/VECTOR/FULLTEXT indexes, node and relationship constraints, catalog-backed scans, and full-text and vector query procedures that survive WAL recovery and snapshot reload."
authors: [loradb]
tags: [release-notes, announcement, performance, architecture, cypher]
image: /img/blog/loradb-v0-9-indexes-and-constraints-header.png
---

![LoraDB v0.9 — indexes, constraints, and a real schema catalog.](/img/blog/loradb-v0-9-indexes-and-constraints-header.png)

LoraDB v0.9 is a schema-catalog release.

v0.5 made the engine stream. v0.6 made persistence feel like a system.
v0.7 was a process release. v0.8 made the planner and executor
observable. v0.9 closes the next gap: the planner now has a real schema
catalog to plan against, and the engine has real constraints to enforce.

The result is a single coherent surface — index DDL, constraint DDL,
catalog-backed scans, full-text and vector query procedures — wired
through the parser, store, optimizer, executor, WAL, and snapshots.

<!-- truncate -->

## Not a shift away from schema-free graphs

LoraDB is still a schema-free property graph. Labels, relationship
types, and properties continue to appear when you write them. v0.9
does not introduce a mandatory schema.

What it introduces is a way for developers to be explicit about two
things:

- _Keep a secondary structure for this hot predicate_ — that is an
  index.
- _Reject writes that violate this invariant_ — that is a constraint.

Both are opt-in. A graph that never issues a `CREATE INDEX` or
`CREATE CONSTRAINT` keeps the v0.8 behavior unchanged.

## Index DDL

Index management is now part of the Cypher surface:

```cypher
CREATE INDEX user_email FOR (u:User) ON (u.email);
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location);
CREATE VECTOR INDEX movie_embedding FOR (m:Movie) ON (m.embedding)
OPTIONS {indexConfig: {`vector.dimensions`: 384, `vector.similarity_function`: 'cosine'}};
CREATE FULLTEXT INDEX article_search FOR (n:Article|Note) ON EACH [n.title, n.body];
CREATE INDEX rel_since FOR ()-[r:FOLLOWS]-() ON (r.since);

SHOW INDEXES;
SHOW VECTOR INDEXES;
DROP INDEX user_email IF EXISTS;
```

The supported catalog kinds are RANGE, TEXT, POINT, LOOKUP, VECTOR, and
FULLTEXT. RANGE is the default for `CREATE INDEX`; TEXT and POINT
activate dedicated string and spatial candidate registries; LOOKUP
records label/type token indexes in the catalog; VECTOR records a kNN
configuration; FULLTEXT builds an inverted index over one or more
properties.

`IF NOT EXISTS` is idempotent across both duplicate names and equivalent
index schemas. Duplicate names surface as `22N71`, equivalent schemas
as `22N70`, and dropping a missing index without `IF EXISTS` returns
`42N51`.

## Constraint DDL

Schema constraints share the same surface:

```cypher
CREATE CONSTRAINT book_isbn FOR (b:Book) REQUIRE b.isbn IS UNIQUE;
CREATE CONSTRAINT author_name FOR (a:Author) REQUIRE a.name IS NOT NULL;
CREATE CONSTRAINT actor_fullname FOR (a:Actor)
REQUIRE (a.first, a.last) IS NODE KEY;
CREATE CONSTRAINT movie_title FOR (m:Movie)
REQUIRE m.title IS :: STRING | LIST<STRING NOT NULL>;

SHOW CONSTRAINTS;
DROP CONSTRAINT book_isbn IF EXISTS;
```

The supported constraint family covers node and relationship
uniqueness, existence, node keys, relationship keys, and property type
constraints — including fixed-dimension VECTOR property types.
Creating a constraint validates existing data before committing the
catalog change; later writes are checked at mutation time.

## Faster scan shapes

The optimizer now uses graph statistics and catalog state when it
compiles a query. Eligible scan-and-filter patterns can lower to
specialized operators:

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
WHERE geo.within_bbox(v.location, $southwest, $northeast)
RETURN v;
MATCH ()-[r:FOLLOWS]->() WHERE r.since > 2020 RETURN r;
```

The executor still refilters conservative candidate sets. TEXT indexes
use a trigram candidate path, and POINT indexes use spatial buckets, so
correctness does not depend on the index being exact.

Run the same query through `profile()` (shipped in v0.8) before and
after a `CREATE INDEX` to see the plan rewrite for itself.

## Search procedures

Two catalog-backed procedure families are now exposed through the
limited built-in procedure dispatcher.

Full-text indexes support node and relationship scopes, multiple labels
or relationship types, and multiple indexed properties:

```cypher
CREATE FULLTEXT INDEX article_search
FOR (n:Article|Note) ON EACH [n.title, n.body]
OPTIONS {`fulltext.analyzer`: 'standard'};

CALL db.index.fulltext.queryNodes('article_search', 'graph powerful')
YIELD node, score;
```

The standard analyzer lowercases text, splits on non-alphanumeric
boundaries, uses AND semantics across query terms, and scores by summed
term frequency. It returns rows in descending score order. It is
deliberately small: analyzer choice is currently limited to `standard`
and `simple`, both on the same synchronous maintenance path.

Vector indexes support node and relationship scopes with explicit
dimensions and either `cosine` or `euclidean` similarity:

```cypher
CREATE VECTOR INDEX movie_embedding FOR (m:Movie) ON (m.embedding)
OPTIONS {indexConfig: {`vector.dimensions`: 3, `vector.similarity_function`: 'cosine'}};

CALL db.index.vector.queryNodes('movie_embedding', 5, [1.0, 0.0, 0.0])
YIELD node, score;
```

This is not an ANN engine yet. VECTOR indexes are catalog entries that
validate configuration and scope the query procedure; the current
implementation still uses a flat scan over matching entities and
returns the top `k` rows by score. The shape is stable; the speedup is
a later release.

## Durability and snapshots

Index and constraint DDL travel through the same write path as graph
mutations. WAL payloads now encode `CreateIndex`, `DropIndex`,
`CreateConstraint`, and `DropConstraint` events, and recovery replays
them into the in-memory catalog before queries run.

Snapshots also carry both catalogs. The `LORACOL1` envelope remains
format version 2, and the snapshot body is now version 4: version 3
added the index-catalog trailer, and version 4 adds the
constraint-catalog trailer. Readers still accept older body formats,
loading older snapshots with empty index or constraint lists as
needed.

The snapshot and WAL codecs now use a small store-owned binary codec
for nested property values and catalog records. That keeps WAL and
snapshots aligned on the same catalog wire shape, including
VECTOR/FULLTEXT index definitions and constraint property-type
records.

## Developer-facing improvements

Planner APIs now accept `GraphStats`, and the plan cache fingerprints
catalog and cardinality state so adding or dropping an index
invalidates stale plans. That means a query explained before
`CREATE INDEX` can recompile into an indexed scan immediately after the
catalog changes.

Read-only materialization also gained an optional native `parallel`
feature through Rayon. It is default-on for native builds and disabled
for WASM-style consumers through `default-features = false`.

`SHOW INDEXES` now accepts type filters such as `SHOW RANGE INDEXES`,
`SHOW FULLTEXT INDEXES`, and `SHOW VECTOR INDEXES`. `SHOW INDEXES` and
`SHOW CONSTRAINTS` also accept a YIELD-anchored projection tail:

```cypher
SHOW INDEXES
YIELD name, type, entityType
WHERE type = 'FULLTEXT'
RETURN name
ORDER BY name;
```

Benchmark coverage was reshaped around intent:

- `query_implementations` mirrors integration-test feature areas;
- `index_acceleration` compares indexed and unindexed RANGE/TEXT
  scenarios;
- existing `scale`, `realistic`, `wal`, `concurrent`, and
  `concurrency_guard` suites remain workload-specific tools.

## Breaking changes and migration notes

For Rust callers of `lora-compiler`, `Compiler::compile` now requires a
`&GraphStats` argument:

```rust
let compiled = Compiler::compile(&resolved, &store.graph_stats());
```

Use `GraphStats::default()` when compiling outside a store-backed
runtime.

Snapshot writers now emit the newer `LORACOL1` envelope/body
combination. Current readers accept the previous body version, but
older binaries should not be expected to read snapshots written after
the index and constraint catalog trailers landed.

There are no data-model migrations for normal users. Existing graphs
remain schema-free; add indexes where predicates are hot enough to
justify the extra memory, and add constraints only for invariants you
want enforced on every matching write.

## Notable fixes

- Plan-cache invalidation now accounts for catalog/cardinality changes.
- Index and constraint catalog DDL survives WAL crash recovery.
- TEXT and POINT indexes track property updates and label/type
  membership changes.
- FULLTEXT indexes backfill existing data and track property
  updates/removals.
- Constraint-owned backing indexes cannot be dropped directly; use
  `DROP CONSTRAINT` so the catalogs stay in sync.
- Indexed scans preserve already-bound node and relationship variables
  instead of rebinding them.
- Regex predicates cache compiled patterns per thread, avoiding
  repeated compilation for row-by-row `=~` filters.
- Numeric conversion helpers now reject non-finite or out-of-range
  float-to-int conversions instead of silently casting.
- Ruby binding calls release the GVL more defensively and surface
  engine panics as query errors.

## How v0.9 fits the journey

v0.5 made the engine stream. v0.6 made persistence feel like a system.
v0.7 was a process release. v0.8 made the planner and executor
observable.

v0.9 closes the last gap before the planner stops being a black box:
the catalog. Indexes were already inferable from the code; constraints
were not enforceable at all. v0.9 gives both a stable surface — Cypher
DDL, WAL events, snapshot trailers, and `SHOW` introspection — that
every binding inherits without per-language plumbing.

## Still open

This work does not add ANN structures for vector search, custom
full-text analyzers, a full-text query language, general
`CALL`/`RETURN` procedure pipelines, or sorted-index `ORDER BY`
planning. Composite RANGE indexes are accepted and visible in
`SHOW INDEXES`, but current optimizer rewrites target single-property
predicates. Those are the natural extensions of the v0.9 shape, not
prerequisites for it.

## Read next

- [Indexes — Cypher reference](/docs/queries/indexes)
- [Constraints — Cypher reference](/docs/queries/constraints)
- [Vector values and similarity functions](/docs/data-types/vectors)
- [Limitations](/docs/limitations)

v0.9 is the release where LoraDB's planner stops guessing at structure
that already exists in the application's head.
