---
slug: loradb-v0-12-vector-indexing
title: "LoraDB v0.12: Vectors, end to end"
description: "LoraDB v0.12 ships a complete vector indexing subsystem: flat and HNSW backends, four similarity metrics, hybrid pre-filters, int8 quantization, async populate, snapshot persistence, and a Cypher wizard for tuning it all."
authors: [loradb]
tags: [release-notes, announcement, vectors, indexing, hnsw, performance]
image: /img/blog/loradb-v0-12-vector-indexing-header.png
---

![LoraDB v0.12. Vectors, end to end.](/img/blog/loradb-v0-12-vector-indexing-header.png)

LoraDB v0.12 is a vector release.

v0.5 made the engine stream. v0.6 made persistence feel like a system.
v0.7 was a process release. v0.8 made plans and runtime metrics easier
to inspect. v0.9 gave the planner a schema catalog. v0.10 made the
function library a library. v0.11 put the engine behind a URL at
[play.loradb.com](https://play.loradb.com).

v0.12 turns the vector type into a real index. Until this release a
`VECTOR` value was a first-class property you could store, score, and
return, but `CREATE VECTOR INDEX` was a catalog entry with no backing
structure. Every k-NN query did a flat scan over every label-matching
node. v0.12 keeps that behaviour as a deliberate fallback and adds an
HNSW backend, hybrid pre-filters, four similarity metrics, int8
quantization, async populate, and snapshot persistence behind it. The
playground gets a tuning wizard so none of this requires reading the
catalog by hand.

<!-- truncate -->

## What ships

The work is structured as five phases. Each one is shippable on its
own. The release is the union.

### A real index, not just a catalog entry

`CREATE VECTOR INDEX` registers a backend that lives alongside the
property store. Writes flow through the same secondary-index
maintenance hook that already serves TEXT, POINT, and FULLTEXT, so a
`SET n.embedding = ...` updates the vector index in lockstep. Queries
go through `GraphStorage::vector_search`, not a per-call scan of the
node store.

The default backend is still flat scan, which scores every vector on
every query. It is correct, deterministic, and the right pick under
about ten thousand vectors. The phase-1 refactor by itself shaved
about 25% off the cost of `CALL db.index.vector.queryNodes` at
n=1,000, d=384 because the backend reads a pre-built map instead of
chasing label index then `Arc<NodeRecord>` then property lookup for
every entity.

### HNSW for sub-linear k-NN

```cypher
CREATE VECTOR INDEX movie_emb FOR (m:Movie) ON (m.embedding)
OPTIONS {indexConfig: {
  `vector.dimensions`: 384,
  `vector.similarity_function`: 'cosine',
  `vector.indexProvider`: 'hnsw'
}}
```

The HNSW backend is hand-rolled. No new dependency. The implementation
follows Malkov and Yashunin (2018) with the simple closest-M neighbour
selection. Defaults match the names used by Neo4j so existing configs
port without surprise:

- `vector.hnsw.m`: 16
- `vector.hnsw.ef_construction`: 200
- `vector.hnsw.ef_search`: 100

At n=10,000, d=384, k=10, cosine, the bench measured 4.38 ms per
query for flat and 1.19 ms for HNSW: a 3.7x speedup. The gap widens
with N because flat is O(N) and HNSW is roughly O(log N).

Per-index level assignment is seeded from the index name, so a
snapshot reload reproduces the same graph topology and the same
top-k. Recall against the flat oracle holds at 0.95 or higher on
uniform random embeddings at d=64 with default knobs.

### Four similarity metrics

`vector.similarity_function` accepts `cosine`, `euclidean`, `dot`,
and `manhattan`. Cosine and euclidean are unchanged from prior
releases. Dot product is added for normalised embeddings (one
reciprocal-sqrt cheaper per pair). Manhattan uses `1 / (1 + d_L1)`
for the same higher-is-better shape as euclidean.

The HNSW backend works with every metric for free. The algorithm
operates on `-similarity` internally and inherits whichever scoring
function the catalog records.

### Hybrid queries

The procedure grew an optional fourth argument:

```cypher
CALL db.index.vector.queryNodes(
  'movie_emb',
  10,
  $queryVec,
  {restrictTo: [1, 5, 12, 47]}
) YIELD node, score
RETURN node, score
```

The flat backend skips non-allowed ids during scoring. HNSW keeps
non-allowed ids as routing hops, because evicting them would
fragment the graph, but excludes them from the result heap. Under a
filter, internal `ef` auto-bumps to keep recall stable when the
filter is selective. Users facing very tight filters should still
raise `vector.hnsw.ef_search` at index creation.

Pre-computing the candidate set with a normal `MATCH` + `collect`
is currently a two-step pattern: the standalone-CALL router does
not yet thread through `WITH`. That integration is the next item on
the planner side.

### int8 quantization for HNSW

```cypher
OPTIONS {indexConfig: {
  ...
  `vector.hnsw.quantization`: 'int8'
}}
```

Each f32 coordinate is scaled by 127 and stored as `INTEGER8`. The
query vector is quantized the same way at search time. Storage drops
by 4x for the vector portion of an HNSW index.

The current implementation accepts `int8` only with cosine. Cosine
is scale-invariant, so the implicit ×127 scaling preserves ranking
exactly. Euclidean and manhattan are not, and would return a
degenerate score range, so the schema validator rejects the
combination at DDL time rather than silently mis-ranking.

### Async populate

```cypher
OPTIONS {indexConfig: {
  ...
  `vector.populate.async`: true
}}
```

When set, the index registers in `Populating` state and `CREATE`
returns immediately. The first query against the index triggers the
backfill inline, then flips the state to `Online`. Mutations between
`CREATE` and the first query already flow through the maintenance
hook, so nothing is dropped: the lazy phase only handles vectors
that existed before `CREATE`.

This trades initial-query latency for `CREATE` latency. It is the
right pick when you have a script that creates the index alongside
other work and does not query it for a while.

### Snapshot persistence

HNSW pays an O(n log n) cost to rebuild. v0.12 captures the entire
backend in the snapshot trailer: nodes, layered neighbour lists,
entry point, RNG state, and quantization config. On load, the
restore overlays the persisted topology after the catalog
re-registers the index. Older snapshots round-trip cleanly through
the existing rebuild path.

A round-trip test confirms a donor HNSW returns byte-identical
top-k ids in the same order after `save_snapshot_to_bytes` and
`load_snapshot_from_bytes`. That is the strongest signal that the
rebuild path was bypassed, because a rebuild would produce a
different graph topology under a different RNG seed.

The trailer is JSON-encoded for v0.12. The length-prefixed framing
is stable, so a future binary codec can drop in without bumping the
snapshot format version.

### A wizard for all of this

In the playground, the `Add index` flow now ships a `Tune` step
that appears only when the kind is `VECTOR`. It exposes every
option the engine accepts:

- a `NumberInput` for dimensions, clamped to 1..4096, with a hint
  pointing at common embedding widths (384 for MiniLM, 768 for
  BERT-base, 1536 for OpenAI text-embedding-3-small)
- a segmented control for similarity, with a one-line tradeoff
  blurb per choice
- a segmented control for provider (HNSW or flat)
- three marked sliders for M, efConstruction, efSearch
- an int8 quantization switch that auto-disables itself with a
  tooltip when combined with a non-cosine metric
- an async populate switch
- a `Quick read` panel that summarises the active tuning in plain
  language as the user changes knobs

Editing an existing vector index round-trips its tuning so users
never lose their config.

`SHOW INDEXES` now surfaces the full `OPTIONS` map as a column so
the same panel can list the active configuration in the index
inspector.

## Same engine, not a separate path

The vector index pipeline is not a sidecar. It uses:

- the same `LoraVector` value model that powered scoring and
  storage since v0.5,
- the same `IndexCatalog` that holds TEXT, POINT, FULLTEXT, and
  RANGE entries,
- the same `secondary_index_maintenance` hook that already keeps
  trigram and grid indexes in lockstep with the property store,
- the same `GraphStorage` trait surface used by every Cypher
  read path,
- the same snapshot codec used by the rest of the catalog.

The trait that adds `vector_search` has a default that returns an
empty vector, so backends without HNSW support degrade to "no
results" cleanly. The procedure parser, the schema validator, and
the playground wizard all share one source of truth for what the
options map can contain.

## What is deferred

A few things from the design plan are not in v0.12 and are flagged
as Phase 6 follow-ups in the issue tracker:

- Planner pushdown so `WITH ids CALL ...` can thread an id set into
  the procedure without a textual hack.
- Hand-rolled binary codec for the snapshot trailer (today it is
  JSON-encoded inside a length-prefixed bytes block).
- The HNSW heuristic neighbour selection from Algorithm 4 of the
  paper. Recall on uniform data is fine without it; clustered data
  may benefit.
- Quantization for euclidean and manhattan, which needs a different
  storage encoding than the scale-invariant cosine trick.

## Try it

```bash
cargo add lora-database
```

Or open [play.loradb.com](https://play.loradb.com) and click `Add
index` in the schema panel. Pick `Vector similarity`, fill in a
label and property, accept the defaults on the Tune step, and the
generated DDL appears in the preview before you commit it.

The full changelog and binaries are on the
[v0.12.0 release page](https://github.com/loradb/loradb/releases/tag/v0.12.0).
