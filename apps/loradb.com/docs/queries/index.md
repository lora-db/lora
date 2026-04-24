---
title: Querying LoraDB with Cypher
sidebar_label: Overview
description: An index of every Cypher clause LoraDB supports — MATCH, WHERE, RETURN, WITH, CREATE, MERGE, SET, DELETE, UNWIND, paths, and aggregation — with links to each clause reference.
---

# Querying LoraDB with Cypher

LoraDB speaks a pragmatic subset of Cypher. Queries are strings that
chain _clauses_ — see the [clause reference](#clause-reference) table
below, or jump into the [**Ten-Minute Tour**](../getting-started/tutorial)
for a guided run-through.

```cypher
MATCH  (p:Person)-[:WORKS_AT]->(c:Company)
WHERE  p.active = true
RETURN p.name, c.name
ORDER  BY p.name
```

Each clause reads the rows emitted by the previous one and passes rows
forward. [`RETURN`](./return-with) ends the pipeline.

## Anatomy of a query

```text
MATCH  — find patterns                (produces rows)
 ↓
WHERE  — filter rows                  (drops rows)
 ↓
WITH   — project + optionally group   (reshapes rows)
 ↓
WHERE  — filter rows post-aggregate   (HAVING-style)
 ↓
RETURN — project + sort + paginate    (final shape)
```

Not every query uses every stage. The important invariant: each clause
sees the rows produced by the previous one.

## Clause reference

| Clause | Purpose |
|---|---|
| [**MATCH**](./match) | Find patterns of nodes and relationships |
| [**CREATE**](./create) | Create nodes and relationships |
| [**WHERE**](./where) | Filter rows |
| [**RETURN / WITH**](./return-with) | Project, rename, order, and page results |
| [**ORDER BY / SKIP / LIMIT**](./ordering) | Sort and paginate |
| [**SET / REMOVE / DELETE**](./set-delete) | Mutate existing entities |
| [**UNWIND / MERGE**](./unwind-merge) | Iterate over lists; create-or-match |
| [**Aggregation**](./aggregation) | `count`, `collect`, `avg`, and group-by |
| [**Paths**](./paths) | Variable-length traversals and shortest paths |

For copy-paste examples covering every clause, see
[**Query Examples**](./examples). For a single-page terse reference,
see the [**Cheat sheet**](./cheat-sheet).

## Where common tasks live

| Task | Page |
|---|---|
| Look up by label + property | [MATCH](./match#inline-property-filter) |
| Write new nodes/edges | [CREATE](./create) |
| Upsert | [MERGE](./unwind-merge#merge) |
| Bulk import | [UNWIND + CREATE](./unwind-merge#bulk-load-from-parameter) |
| Patch a property map | [<CypherCode code="SET +=" />](./set-delete#merge-properties-) |
| Replace all properties | [<CypherCode code="SET =" />](./set-delete#replace-all-properties-) |
| Remove a property | [<CypherCode code="REMOVE" /> / <CypherCode code="SET n.p = null" />](./set-delete#remove) |
| Delete with edges | [<CypherCode code="DETACH DELETE" />](./set-delete#detach-delete) |
| Top-N | [<CypherCode code="ORDER BY + LIMIT" />](./ordering#top-n) |
| Stable pagination | [Keyset pagination](./ordering#stable-pagination) |
| Group and aggregate | [Aggregation walkthrough](./aggregation#a-five-step-walkthrough) |
| HAVING-style filter | [<CypherCode code="WITH … WHERE" />](./return-with#having-style-filtering-with) |
| Anti-join | [<CypherCode code="NOT EXISTS" />](./where#pattern-existence) |
| Shortest path | [<CypherCode code="shortestPath" />](./paths#shortest-paths) |
| Inline related list | [Pattern comprehension](../functions/list#pattern-comprehension) |
| Per-row conditional value | [CASE expressions](./return-with#case-expressions) |
| Count rows matching a condition | [<CypherCode code="count(CASE WHEN … THEN 1 END)" />](./examples#conditional-count-case-inside-count) |

## Execution model

- Queries execute **atomically** per call. There is no explicit
  transaction boundary.
- Reads and writes share a single mutex — queries run one at a time.
  This keeps the model simple and removes classes of concurrency bugs,
  but it means concurrent reads don't parallelise. See
  [Limitations → Concurrency](../limitations#concurrency).
- Names (labels, relationship types, property keys) are validated
  against the live graph for [`MATCH`](./match); any name is accepted
  by [`CREATE`](./create), [`MERGE`](./unwind-merge#merge), and
  [`SET`](./set-delete).
- Unknown function names are rejected at analysis time — see
  [**Functions**](../functions/overview).

## Parameters

Any value that isn't a constant should use a parameter. The short
version follows; [**Parameters**](./parameters) has the full reference.

```cypher
MATCH (p:Person) WHERE p.name = $name RETURN p
```

Parameters are bound at call time from the host language:

- [Rust](../getting-started/rust#parameterised-query) — `BTreeMap<String, LoraValue>`
- [Node.js](../getting-started/node#parameterised-query) — plain object
- [Python](../getting-started/python#parameterised-query) — `dict`
- [WASM](../getting-started/wasm#parameterised-query) — plain object
- [HTTP server](../getting-started/server#post-query) — **not yet**
  supported, see [Limitations → Parameters](../limitations#parameters)

Missing parameters resolve to `null`, which can silently produce empty
results — set them or validate inputs before executing.

### Parameters vs inline literals

```cypher
-- Safe (parameterised)
MATCH (u:User) WHERE u.id = $id RETURN u

-- Unsafe if $id came from untrusted input and was inlined
MATCH (u:User) WHERE u.id = 42 RETURN u
```

Parameters are the only supported way to mix untrusted input into a
query. They also let the query planner cache plans across invocations.

### Parameter types

| Host value | LoraDB type |
|---|---|
| `null` / `None` / `undefined` | [`Null`](../data-types/scalars#null) |
| `bool` | [`Boolean`](../data-types/scalars#boolean) |
| `int` (Python) / `number` (JS, integer) / `i64` (Rust) | [`Integer`](../data-types/scalars#integer) |
| `float` (Python) / `number` (JS, non-integer) / `f64` (Rust) | [`Float`](../data-types/scalars#float) |
| `str` / `String` | [`String`](../data-types/scalars#string) |
| list / array / `Vec` | [`List`](../data-types/lists-and-maps#lists) |
| dict / object / `BTreeMap` | [`Map`](../data-types/lists-and-maps#maps) |
| helpers (`date()`, `wgs84()`, …) | [`Date`](../data-types/temporal), [`Point`](../data-types/spatial), etc. |

## What's not supported

See [**Limitations**](../limitations) for the full list. The short
version: no `CALL`, no `FOREACH`, no `LOAD CSV`, no DDL
(`CREATE INDEX`, constraints), no multi-database (`USE`).

## See also

- [**Ten-Minute Tour**](../getting-started/tutorial) — guided walkthrough.
- [**Cheat sheet**](./cheat-sheet) — single-page quick reference.
- [**Parameters**](./parameters) — typed parameter binding.
- [**Query Examples**](./examples) — copy-paste recipes by shape.
- [**Cookbook**](../cookbook) — scenario-driven recipes.
- [**Functions**](../functions/overview) — every built-in.
- [**Data types**](../data-types/overview) — value shapes for parameters and properties.
- [**Graph model**](../concepts/graph-model) — the underlying data model.
- [**Result formats**](../concepts/result-formats) — how results come back over the wire.
