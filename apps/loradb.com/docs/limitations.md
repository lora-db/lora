---
title: Limitations
sidebar_label: Limitations
description: Every Cypher feature and operational capability LoraDB does not support today — persistence, indexes, constraints, procedures, clustering — and what to reach for instead.
---

# Limitations

A single page for every Cypher feature and operational capability
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
| Storage | WAL-backed open exists on Rust, Node, Python, Go, Ruby, and `lora-server`; WASM stays snapshot-only; no indexes; no constraints |
| Concurrency | Single global lock, no timeouts |
| Clauses | No `CALL`, `FOREACH`, `LOAD CSV`, DDL |
| Patterns | No quantified path patterns |
| Operators | No `BETWEEN`; cross-type comparisons return `null` |
| Aggregates | No `GROUP BY` / `HAVING` keywords |
| Functions | No APOC; ASCII-only case ops |
| Parameters | No HTTP-level params; no parse-time type check |
| Spatial | No WKT I/O, no CRS transforms, no bbox predicate |
| Vectors | No indexes / ANN; no embedding generation; no list-of-vectors properties |

## Clauses

| Feature | Status |
|---|---|
| `CALL` (standalone) | Not supported — parses, analyzer rejects with `UnsupportedFeature` |
| `CALL … YIELD` | Not supported — parses, analyzer rejects with `UnsupportedFeature` |
| `FOREACH` | Not supported |
| `CREATE INDEX` / `DROP INDEX` | Not supported |
| `CREATE CONSTRAINT` / `DROP CONSTRAINT` | Not supported |
| `LOAD CSV` | Not supported |
| `USE <graph>` (multi-database) | Not supported |
| `PROFILE` | Not supported (`EXPLAIN` is supported) |

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
| APOC-style utilities (`apoc.*`) | Not supported — no compatibility layer |
| User-defined functions | Not supported — no registration surface |
| [`date.truncate`](./functions/temporal#truncation) units | Only `"year"` and `"month"`; `"quarter"` / `"week"` / `"day"` not yet supported |
| [`datetime.truncate`](./functions/temporal#truncation) units | Only `"day"`, `"hour"`, and `"month"`; sub-hour units not yet supported |
| [`toLower` / `toUpper`](./functions/string#tolower--toupper) | ASCII-only — Unicode case folding not yet supported |
| [`normalize(str)`](./functions/string#normalize) | Not yet implemented — placeholder returns its input unchanged |

## Data types

| Feature | Status |
|---|---|
| Binary / byte arrays | Not supported — store base64 strings |
| Fixed-precision decimals | Not supported — use scaled integers or strings |
| User-defined types | Not supported |
| Numeric overflow guarding | Not supported — Rust panics in debug, wraps in release |

## Spatial

| Feature | Status |
|---|---|
| WGS-84 3D [`distance`](./functions/spatial#distance) honouring `height` | Not yet supported — computes surface great-circle only |
| `point.withinBBox()` | Not yet supported |
| `point.fromWKT()` / WKT output | Not yet supported |
| CRS transformation between SRIDs | Not yet supported — cross-SRID `distance` returns `null` |
| Custom SRIDs | Not supported — only `7203`, `9157`, `4326`, `4979` |

## Vectors

| Feature | Status |
|---|---|
| Vector indexes / approximate nearest neighbour | Not yet supported — every similarity / distance call is a linear scan over matched candidates |
| Built-in embedding generation | Not supported — no plugin surface; generate embeddings in host code |
| [List-of-vectors as a property](./data-types/vectors#restriction-no-list-of-vectors-as-a-property) | Not supported — rejected at write time; hang many embeddings off separate nodes |
| Dimension > 4096 | Not supported — rejected at construction time |
| `ORDER BY` on a `VECTOR` column | Not implemented — runs without panicking, but ordering is unspecified; order by a scalar score instead |
| Metric extensions (e.g. Minkowski, Chebyshev) | Not yet supported — current metrics are listed in [Vectors → Signed distance metrics](./data-types/vectors#signed-distance-metrics) |
| Passing a `VECTOR` parameter over HTTP | Blocked by the HTTP parameters limitation below — build the vector with `vector(...)` in the query string or use an in-process binding |

## Parameters

| Feature | Status |
|---|---|
| Parameter as a label or relationship type | Not supported |
| Parameter type checking at parse time | Not yet supported |
| Parameters over HTTP | Not yet supported — the `/query` body ignores `params`; use an in-process binding |

## Storage

| Gap | Impact |
|---|---|
| WAL controls are not uniform across bindings | Rust and `lora-server` expose the full [WAL](./wal) surface. Node, Python, Go, and Ruby expose simple WAL-backed initialization only. WASM remains snapshot-only. |
| Automatic checkpoint loop — not yet supported | Checkpoints are explicit (`checkpoint_to(...)`, `POST /admin/checkpoint`, or host-driven snapshot saves). Nothing schedules them in the background for you. |
| Uniqueness constraints — not supported | Duplicates can be created silently; enforce in application code or match before creating |
| Property indexes — not yet supported | Property filters without a label are `O(n)` full scans |
| Explicit transactions — not supported | Each query is atomic; no multi-query transaction boundary |
| ID reuse — not supported | Deleting an entity does not free its `u64` id |

## Concurrency

- A single global lock serialises every query. Concurrent **reads**
  do not parallelise.
- Query timeouts — not supported; a pathological query can hold the
  lock indefinitely.
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

## Workarounds cheatsheet

| Instead of | Use |
|---|---|
| `BETWEEN a AND b` | `x >= a AND x <= b` |
| `HAVING` | [`WITH … WHERE`](./queries/return-with#having-style-filtering-with) |
| `GROUP BY cols` | Non-aggregated columns in `RETURN` / `WITH` |
| `CREATE INDEX ON :L(prop)` | Scope to a label in `MATCH (n:L {…})` |
| `CONSTRAINT UNIQUE` | [`MERGE`](./queries/unwind-merge#merge) on the key + `SET` |
| `LOAD CSV` | Parse on host, pass as `$rows`, [`UNWIND $rows`](./queries/unwind-merge#unwind) |
| `CALL apoc.…` | Re-implement in the host language |
| `point.withinBBox()` | Explicit `lat/lon` `>=` / `<=` |
| `point.fromWKT()` | Parse host-side, pass as a [point param](./functions/spatial#parameters) |
| `GROUP BY year` | `RETURN e.at.year AS year, count(*)` |
| `IF/THEN/ELSE` expressions | [`CASE … WHEN … THEN … END`](./queries/return-with#case-expressions) |
| Window function `row_number()` per group | `ORDER BY … collect(…)[..N]` pipeline |
| `COUNT(*) FILTER (WHERE …)` | [`count(CASE WHEN … THEN 1 END)`](./queries/aggregation#conditional-count-count-if) |

## Out of scope (for now)

These are part of standard Cypher but **not** on the short-term
roadmap:

- Stored procedures (`CALL` family)
- `LOAD CSV`-based ingestion
- Schema constraints / indexes at the DDL level
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
