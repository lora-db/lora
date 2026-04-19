---
title: What's Not Supported Yet
sidebar_label: Limitations
---

# What's Not Supported Yet

What's **not** supported today. Everything below produces a clear error
(parse error, `SemanticError::UnsupportedFeature`, or `UnknownFunction`)
— nothing silently misbehaves.

For the full, machine-checkable feature list see the
[Cypher support matrix](https://github.com/loradb/loradb/blob/main/docs/reference/cypher-support-matrix.md)
in the internal documentation.

## At a glance

| Theme | Biggest gaps |
|---|---|
| Storage | No persistence, no indexes, no constraints |
| Concurrency | Single global lock, no timeouts |
| Clauses | No `CALL`, `FOREACH`, `LOAD CSV`, DDL |
| Patterns | No quantified path patterns |
| Operators | No `BETWEEN`; cross-type comparisons return `null` |
| Aggregates | No `GROUP BY` / `HAVING` keywords |
| Functions | No APOC; ASCII-only case ops |
| Parameters | No HTTP-level params; no parse-time type check |
| Spatial | No WKT I/O, no CRS transforms, no bbox predicate |

## Clauses

| Feature | Status |
|---|---|
| `CALL` (standalone) | Parsed; analyzer rejects with `UnsupportedFeature` |
| `CALL … YIELD` | Parsed; analyzer rejects with `UnsupportedFeature` |
| `FOREACH` | Not in grammar |
| `CREATE INDEX` / `DROP INDEX` | Not in grammar |
| `CREATE CONSTRAINT` / `DROP CONSTRAINT` | Not in grammar |
| `LOAD CSV` | Not in grammar |
| `USE <graph>` (multi-database) | Not in grammar |
| `PROFILE` | Not in grammar (`EXPLAIN` is supported) |

## Patterns

| Feature | Status |
|---|---|
| Quantified path patterns `((:X)-[:R]->(:Y)){1,3}` | Not in grammar |
| Inline `WHERE` inside variable-length relationships | Parsed, not evaluated |

## Operators and expressions

| Feature | Status |
|---|---|
| `BETWEEN a AND b` | Not supported — use `x >= a AND x <= b` |
| Type-mismatch detection in comparisons | Not implemented — cross-type `<` / `>` returns `null` rather than erroring |
| Aggregates inside [`WHERE`](./queries/where) | Rejected — use [`WITH … WHERE`](./queries/return-with#with) instead |

## Aggregates

| Feature | Status |
|---|---|
| `DISTINCT` on `stdev`, `stdevp`, `percentileCont`, `percentileDisc` | Not supported — use `collect(DISTINCT x)` and aggregate the unwound list |
| `GROUP BY` keyword | Not available — non-aggregated columns are the implicit group key |
| `HAVING` keyword | Not available — filter post-aggregate through `WITH … WHERE` |

## Functions

| Feature | Status |
|---|---|
| APOC-style utilities (`apoc.*`) | No compatibility layer |
| User-defined functions | No registration surface |
| [`date.truncate`](./functions/temporal#truncation) units | Only `"year"` and `"month"` |
| [`datetime.truncate`](./functions/temporal#truncation) units | Only `"day"`, `"hour"`, and `"month"` |
| [`toLower` / `toUpper`](./functions/string#tolower--toupper) | ASCII-only — non-ASCII letters pass through unchanged |
| [`normalize(str)`](./functions/string#normalize) | Placeholder — does not apply Unicode NFC |

## Data types

| Feature | Status |
|---|---|
| Binary / byte arrays | Not a type — store base64 strings |
| Fixed-precision decimals | Not a type — use scaled integers or strings |
| User-defined types | Not supported |
| Numeric overflow guarding | Not guarded — Rust panics in debug, wraps in release |

## Spatial

| Feature | Status |
|---|---|
| WGS-84 3D [`distance`](./functions/spatial#distance) honouring `height` | Not implemented — computes surface great-circle only |
| `point.withinBBox()` | Not implemented |
| `point.fromWKT()` / WKT output | Not implemented |
| CRS transformation between SRIDs | Not implemented — cross-SRID `distance` returns `null` |
| Custom SRIDs | Not supported — only `7203`, `9157`, `4326`, `4979` |

## Parameters

| Feature | Status |
|---|---|
| Parameter as a label or relationship type | Not supported |
| Parameter type checking at parse time | Not implemented |
| Parameters over HTTP | The `/query` body does not accept `params` — Rust API only |

## Storage

| Gap | Impact |
|---|---|
| **No persistence** | All data is lost on process exit — LoraDB is in-memory only today |
| No uniqueness constraints | Duplicates can be created silently — enforce in application code or by matching before creating |
| No property indexes | Property filters without a label are `O(n)` full scans |
| No explicit transactions | Each query is atomic; no multi-query transaction boundary |
| IDs are never reused | Deleting an entity doesn't free its `u64` id |

## Concurrency

- A single global lock serialises all queries. Concurrent **reads** do
  not parallelise.
- No query timeout — a pathological query can hold the lock indefinitely.
- No rate limiting on the HTTP server.

## HTTP server

- No authentication. No TLS. Bind `127.0.0.1` only in production until
  this changes — see the security notes in the
  [Contact](/contact#security) page.
- `POST /query` does not accept a `params` body field (see Parameters
  above).
- One server process serves exactly one in-memory graph. Run multiple
  processes for isolation.

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

See [Why LoraDB](/why) for the project's intended direction.

## See also

- [**Troubleshooting**](./troubleshooting) — what to do when a query errors.
- [**Queries → Overview**](./queries/) — the supported subset.
- [**Functions → Overview**](./functions/overview) — supported functions.
- [**Concepts → Graph Model**](./concepts/graph-model) — the underlying data model.
