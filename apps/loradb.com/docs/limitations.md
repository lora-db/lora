---
title: Limitations
sidebar_label: Limitations
description: Every query feature and operational capability LoraDB does not support today — including procedures, clustering, browser playground boundaries, and remaining storage limitations — plus what to reach for instead.
---

# Limitations

A single page for every query feature and operational capability
LoraDB does not support today, so you can decide whether LoraDB fits
your use case and know what to reach for instead. Every unsupported
feature below raises a clear error (a parse error,
`SemanticError::UnsupportedFeature`, or `UnknownFunction`) —
nothing silently misbehaves.

Wording convention used below:

- **Not supported** — hard absence with no near-term plan.
- **Not yet supported** — absence the docs intentionally signal as
  future capability.
- **Not implemented** — the grammar accepts it but the analyzer or
  executor rejects it today.

For the machine-checkable feature list see the
[Cypher support matrix](https://github.com/lora-db/lora/blob/main/docs/reference/cypher-support-matrix.md)
in the internal documentation.

## At a glance

| Theme | Biggest gaps |
|---|---|
| Storage | WAL-backed open exists on Rust, Node, Python, Go, Ruby, and `lora-server`; WASM stays snapshot-only; explicit RANGE/TEXT/POINT/LOOKUP/VECTOR/FULLTEXT indexes and schema constraints exist; no ANN structure |
| Concurrency | Snapshot reads can overlap; write commits and explicit read-write transactions serialize; timeout coverage is API-dependent |
| Clauses | No general-purpose `CALL`, `FOREACH`, `LOAD CSV` |
| Patterns | No quantified path patterns |
| Operators | No `BETWEEN`; cross-type comparisons return `null` |
| Aggregates | No `GROUP BY` / `HAVING` keywords |
| Functions | No external utility compatibility layer; case conversion is Unicode-aware but not locale-specific |
| Parameters | No HTTP-level params; no parse-time type check |
| Spatial | No WKT I/O, no CRS transforms; `geo.within_bbox` exists for same-SRID boxes |
| Vectors | VECTOR indexes are cataloged and queryable through flat-scan procedures; no ANN structure; no embedding generation; no list-of-vectors properties |
| Browser playground | No parameter drawer, no shared hosted database, no true query abort; state is local to the browser origin |

## Clauses

| Feature | Status |
|---|---|
| General-purpose `CALL` | Not supported — analyzer rejects ordinary procedures such as `CALL db.labels()` |
| Index procedure `CALL` | Supported for `db.index.vector.queryNodes`, `db.index.vector.queryRelationships`, `db.index.fulltext.queryNodes`, and `db.index.fulltext.queryRelationships` |
| `FOREACH` | Not supported |
| `CREATE INDEX` / `DROP INDEX` / `SHOW INDEXES` | Supported for RANGE, TEXT, POINT, LOOKUP, VECTOR, and FULLTEXT indexes; see [Indexes](./queries/indexes) |
| `CREATE CONSTRAINT` / `DROP CONSTRAINT` / `SHOW CONSTRAINTS` | Supported for uniqueness, existence, node key, relationship key, and property type constraints; see [Constraints](./queries/constraints) |
| `LOAD CSV` | Not supported |
| `USE <graph>` (multi-database) | Not supported |
| `EXPLAIN` / `PROFILE` Cypher keywords | Not supported — use the explicit binding methods (`db.explain`, `db.profile`, `Explain`, `Profile`) or HTTP `/explain` and `/profile` endpoints |

## Patterns

| Feature | Status |
|---|---|
| Quantified path patterns `((:X)-[:R]->(:Y)){1,3}` | Not supported |
| Inline `WHERE` inside variable-length relationships | Not implemented — parses but is not evaluated |

## Operators and expressions

| Feature | Status |
|---|---|
| `BETWEEN a AND b` | Not supported — use `x >= a AND x <= b` |
| Type-mismatch detection in comparisons | Not implemented — cross-type `<` / `>` returns `null` instead of erroring |
| Aggregates inside [`WHERE`](./queries/where) | Not supported — use [`WITH … WHERE`](./queries/return-with#with) instead |

## Aggregates

| Feature | Status |
|---|---|
| `DISTINCT` on `stdev`, `stdevp`, `percentileCont`, `percentileDisc` | Not supported — use `collect(DISTINCT x)` and aggregate the unwound list |
| `GROUP BY` keyword | Not supported — non-aggregated columns are the implicit group key |
| `HAVING` keyword | Not supported — filter post-aggregate through `WITH … WHERE` |

## Functions

| Feature | Status |
|---|---|
| External utility compatibility layer | Not supported |
| User-defined functions | Not supported — no registration surface |
| [`temporal.truncate`](./functions/temporal#truncation) units | `DATE` supports `"year"`, `"month"`, and `"day"`; `DATETIME` supports `"year"`, `"month"`, `"day"`, and `"hour"`. `"quarter"`, `"week"`, and sub-hour truncation are not yet supported |
| [`string.lower` / `string.upper`](./functions/string#tolower--toupper) | Unicode case mapping is supported; locale-specific case folding is not |
| [`string.normalize(str[, form])`](./functions/string#normalize) | Unicode NFC/NFD/NFKC/NFKD normalization is supported; locale-specific transliteration is not |

## Data types

| Feature | Status |
|---|---|
| Binary / byte arrays | Supported as a byte-string property value through binding wire formats; there is no Cypher byte literal |
| Fixed-precision decimals | Not supported — use scaled integers or strings |
| User-defined types | Not supported |
| Numeric overflow guarding | Not supported — Rust panics in debug, wraps in release |

## Spatial

| Feature | Status |
|---|---|
| WGS-84 3D [`geo.distance`](./functions/spatial#geodistance) honouring `height` | Not yet supported — computes surface great-circle only |
| `geo.within_bbox()` | Supported for same-SRID 2D/3D bounding boxes; mixed dimensionality returns `null` |
| `point.fromWKT()` / WKT output | Not yet supported |
| CRS transformation between SRIDs | Not yet supported — cross-SRID `geo.distance` returns `null` |
| Custom SRIDs | Not supported — only `7203`, `9157`, `4326`, `4979` |

## Vectors

| Feature | Status |
|---|---|
| Vector ANN structure | Not yet supported — `db.index.vector.*` procedures use the cataloged index scope but still perform a flat scan over matching entities |
| Built-in embedding generation | Not supported — no plugin surface; generate embeddings in host code |
| [List-of-vectors as a property](./data-types/vectors#restriction-no-list-of-vectors-as-a-property) | Not supported — rejected at write time; hang many embeddings off separate nodes |
| Dimension > 4096 | Not supported — rejected at construction time |
| `ORDER BY` on a `VECTOR` column | Deterministic but implementation-defined; order by a scalar score instead |
| Metric extensions (e.g. Minkowski, Chebyshev) | Not yet supported — current metrics are listed in [Vectors → Signed distance metrics](./data-types/vectors#signed-distance-metrics) |
| Passing a `VECTOR` parameter over HTTP | Blocked by the HTTP parameters limitation below — build the vector with `[...]::VECTOR<COORD>(DIM)` in the query string or use an in-process binding |

## Parameters

| Feature | Status |
|---|---|
| Parameter as a label or relationship type | Not supported |
| Parameter type checking at parse time | Not yet supported |
| Parameters over HTTP | Not yet supported — the `/query` body ignores `params`; use an in-process binding |

## Storage

| Gap | Impact |
|---|---|
| WAL controls are not uniform across bindings | Rust and `lora-server` expose the full [WAL](./wal) surface. Node exposes archive/raw WAL opens plus sync-mode control. Python, Go, and Ruby expose archive/raw WAL opens with managed snapshot options. WASM remains snapshot-only. |
| Time-based checkpoint scheduler — not yet supported | Explicit WAL helpers can write managed snapshots after N committed transactions, and Rust / `lora-server` expose explicit checkpoints. Nothing schedules checkpoints by wall-clock time in the background for you. |
| Constraint coverage is scoped | Uniqueness, existence, node key, relationship key, and property type constraints exist for a label or relationship type. There is no database-wide uniqueness constraint, and existence constraints are single-property only. |
| Index coverage is scoped | `CREATE INDEX`, `CREATE TEXT INDEX`, `CREATE POINT INDEX`, `CREATE VECTOR INDEX`, `CREATE FULLTEXT INDEX`, `DROP INDEX`, and `SHOW INDEXES` exist. Composite RANGE indexes are cataloged, but current planner rewrites are single-property. Vector procedures are flat scans today, not ANN. |
| Explicit transactions are surface-dependent | Rust and in-process bindings expose transaction APIs; HTTP has no multi-query transaction endpoint |
| ID reuse — not supported | Deleting an entity does not free its `u64` id |

## Concurrency

- Auto-commit read-only queries load Arc snapshots and can overlap.
- Write commits serialize, and explicit read-write transactions hold the
  writer slot until commit or rollback.
- Query timeouts are cooperative and not exposed uniformly across every
  binding or HTTP endpoint.
- HTTP rate limiting — not supported.

## HTTP server

- Authentication — not supported. TLS — not supported. Bind to
  `127.0.0.1` only in production until this changes; see the
  security notes on the [Contact](/contact#security) page.
- Admin endpoints ([`POST /admin/snapshot/save`](./api/http#admin-endpoints-opt-in)
  and `/admin/snapshot/load`, plus `/admin/checkpoint`,
  `/admin/wal/status`, and `/admin/wal/truncate` when WAL is enabled)
  are opt-in via `--snapshot-path` / `--wal-dir` and have **no
  authentication**. The optional `path` body field is passed straight
  to the OS. Do not enable them on a network-reachable host without
  authenticated ingress in front.
- Parameters over HTTP — not yet supported (see Parameters above).
- Multi-database — not supported. One process serves exactly one
  in-memory graph; run multiple processes for isolation.

## Browser playground

- Host-side parameter input — not yet supported. The editor detects
  `$name`-style placeholders, but the UI does not yet provide a params
  drawer. Use trusted inline literals for playground-only examples and
  parameters in application bindings.
- Shared hosted database — not supported. The hosted app runs against
  a browser-origin database; saved queries, snapshots, settings,
  history, and auto-restored graph state are local browser data.
- True query abort — not yet supported. The Cancel button drops the
  pending result from the UI, but the current WASM call still runs until
  it returns.
- Multi-database selector — not supported. Use a different browser
  profile/origin or clear site data when you need a separate local
  scratch graph.
- Remote import — not supported. Import accepts local snapshot files;
  the app does not fetch remote URLs to seed a graph.

## Workarounds cheatsheet

| Instead of | Use |
|---|---|
| `BETWEEN a AND b` | `x >= a AND x <= b` |
| `HAVING` | [`WITH … WHERE`](./queries/return-with#having-style-filtering-with) |
| `GROUP BY cols` | Non-aggregated columns in `RETURN` / `WITH` |
| `CREATE INDEX ON :L(prop)` | Use `CREATE INDEX name FOR (n:L) ON (n.prop)` |
| `CONSTRAINT UNIQUE` shorthand | `CREATE CONSTRAINT name FOR (n:Label) REQUIRE n.key IS UNIQUE` |
| `LOAD CSV` | Parse on host, pass as `$rows`, [`UNWIND $rows`](./queries/unwind-merge#unwind) |
| External utility procedures | Re-implement in the host language |
| `geo.within_bbox()` | Use `geo.within_bbox(p, lowerLeft, upperRight)` with matching SRIDs |
| `point.fromWKT()` | Parse host-side, pass as a [point param](./functions/spatial#parameters) |
| `GROUP BY year` | `RETURN e.at.year AS year, count(*)` |
| `IF/THEN/ELSE` expressions | [`CASE … WHEN … THEN … END`](./queries/return-with#case-expressions) |
| Window function `row_number()` per group | `ORDER BY … collect(…)[..N]` pipeline |
| `COUNT(*) FILTER (WHERE …)` | [`count(CASE WHEN … THEN 1 END)`](./queries/aggregation#conditional-count-count-if) |

## Out of scope (for now)

These are part of standard Cypher but **not** on the short-term
roadmap:

- General-purpose stored procedures (`CALL` family), except the supported
  index query procedures
- `LOAD CSV`-based ingestion
- Multi-database `USE`

See [Why LoraDB](./why) for the project's intended direction.

## See also

- [**Troubleshooting**](./troubleshooting) — what to do when a query errors.
- [**HTTP API → Admin endpoints (opt-in)**](./api/http#admin-endpoints-opt-in) — how snapshot and WAL admin routes are exposed over HTTP.
- [**WAL and checkpoints**](./wal) — recovery, sync modes, and admin semantics.
- [**Queries → Overview**](./queries/) — the supported subset.
- [**Cheat sheet**](./queries/cheat-sheet) — one-page quick reference.
- [**Parameters**](./queries/parameters) — typed parameter binding (Rust, Node, Python, WASM, Go, and Ruby bindings; HTTP does not yet forward params).
- [**Schema-free**](./concepts/schema-free) — strict reads, permissive writes.
- [**Functions → Overview**](./functions/overview) — supported functions.
- [**Concepts → Graph Model**](./concepts/graph-model) — the underlying data model.
