---
title: Querying LoraDB with Cypher
sidebar_label: Overview
description: An index of every Cypher clause LoraDB supports — MATCH, WHERE, RETURN, WITH, CREATE, MERGE, SET, DELETE, UNWIND, paths, and aggregation — with links to each clause reference.
---

# Querying LoraDB with Cypher

LoraDB speaks a pragmatic subset of Cypher. Queries are strings that
chain _clauses_ — see the [clause reference](#clause-reference)
below, or jump into the [**Ten-Minute Tour**](/docs/getting-started/tutorial)
for a guided run-through.

<QueryCodeBlock code={String.raw`MATCH  (p:Person)-[:WORKS_AT]->(c:Company)
WHERE  p.active = true
RETURN p.name, c.name
ORDER  BY p.name`} />

Each clause reads the rows emitted by the previous one and passes rows
forward. [`RETURN`](/docs/queries/return-with) ends the pipeline.

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
| [**MATCH**](/docs/queries/match) | Find patterns of nodes and relationships |
| [**CREATE**](/docs/queries/create) | Create nodes and relationships |
| [**WHERE**](/docs/queries/where) | Filter rows |
| [**Indexes**](/docs/queries/indexes) | Declare, inspect, query, and drop secondary indexes |
| [**Constraints**](/docs/queries/constraints) | Add uniqueness, existence, key, and type checks |
| [**RETURN / WITH**](/docs/queries/return-with) | Project, rename, order, and page results |
| [**ORDER BY / SKIP / LIMIT**](/docs/queries/ordering) | Sort and paginate |
| [**SET / REMOVE / DELETE**](/docs/queries/set-delete) | Mutate existing entities |
| [**UNWIND / MERGE**](/docs/queries/unwind-merge) | Iterate over lists; create-or-match |
| [**Aggregation**](/docs/queries/aggregation) | `count`, `collect`, `avg`, and group-by |
| [**Paths**](/docs/queries/paths) | Variable-length traversals and shortest paths |

For copy-paste examples covering every clause, see
[**Query Examples**](/docs/queries/examples). For a single-page terse reference,
see the [**Cheat sheet**](/docs/queries/cheat-sheet).

## Where common tasks live

| Task | Page |
|---|---|
| Look up by label + property | [MATCH](/docs/queries/match#inline-property-filter) |
| Write new nodes/edges | [CREATE](/docs/queries/create) |
| Upsert | [MERGE](/docs/queries/unwind-merge#merge) |
| Bulk import | [UNWIND + CREATE](/docs/queries/unwind-merge#bulk-load-from-parameter) |
| Patch a property map | [<CypherCode code="SET +=" />](/docs/queries/set-delete#merge-properties-) |
| Replace all properties | [<CypherCode code="SET =" />](/docs/queries/set-delete#replace-all-properties-) |
| Remove a property | [<CypherCode code="REMOVE" /> / <CypherCode code="SET n.p = null" />](/docs/queries/set-delete#remove) |
| Delete with edges | [<CypherCode code="DETACH DELETE" />](/docs/queries/set-delete#detach-delete) |
| Top-N | [<CypherCode code="ORDER BY + LIMIT" />](/docs/queries/ordering#top-n) |
| Stable pagination | [Keyset pagination](/docs/queries/ordering#stable-pagination) |
| Group and aggregate | [Aggregation walkthrough](/docs/queries/aggregation#a-five-step-walkthrough) |
| HAVING-style filter | [<CypherCode code="WITH … WHERE" />](/docs/queries/return-with#having-style-filtering-with) |
| Anti-join | [<CypherCode code="NOT EXISTS" />](/docs/queries/where#pattern-existence) |
| Speed up common predicates | [<CypherCode code="CREATE INDEX" />](/docs/queries/indexes) |
| Enforce uniqueness or property shape | [<CypherCode code="CREATE CONSTRAINT" />](/docs/queries/constraints) |
| Shortest path | [<CypherCode code="shortestPath" />](/docs/queries/paths#shortest-paths) |
| Inline related list | [Pattern comprehension](/docs/functions/list#pattern-comprehension) |
| Per-row conditional value | [CASE expressions](/docs/queries/return-with#case-expressions) |
| Count rows matching a condition | [<CypherCode code="count(CASE WHEN … THEN 1 END)" />](/docs/queries/examples#conditional-count-case-inside-count) |

## Execution model

- Queries execute **atomically** per auto-commit call. Rust and in-process
  bindings also expose explicit transactions; HTTP does not.
- Auto-commit reads can overlap on Arc snapshots. Write commits and explicit
  read-write transactions serialize. See
  [Limitations → Concurrency](/docs/limitations#concurrency).
- Names (labels, relationship types, property keys) are validated
  against the live graph for [`MATCH`](/docs/queries/match); any name is accepted
  by [`CREATE`](/docs/queries/create), [`MERGE`](/docs/queries/unwind-merge#merge), and
  [`SET`](/docs/queries/set-delete).
- Unknown function names are rejected at analysis time — see
  [**Functions**](/docs/functions/overview).

## Parameters

Any value that isn't a constant should use a parameter. The short
version follows; [**Parameters**](/docs/queries/parameters) has the full reference.

<QueryCodeBlock code={String.raw`MATCH (p:Person) WHERE p.name = $name RETURN p`} />

Parameters are bound at call time from the host language:

- [Rust](/docs/getting-started/rust#parameterised-query) — `BTreeMap<String, LoraValue>`
- [Node.js](/docs/getting-started/node#parameterised-query) — plain object
- [Python](/docs/getting-started/python#parameterised-query) — `dict`
- [WASM](/docs/getting-started/wasm#parameterised-query) — plain object
- [Go](/docs/getting-started/go#parameterised-query) — `lora.Params`
- [Ruby](/docs/getting-started/ruby#parameterised-query) — `Hash`
- [HTTP server](/docs/api/http#post-query) — JSON `params` object

Missing parameters resolve to `null`, which can silently produce empty
results — set them or validate inputs before executing.

### Parameters vs inline literals

<QueryCodeBlock code={String.raw`// Safe (parameterised)
MATCH (u:User) WHERE u.id = $id RETURN u

;// Unsafe if $id came from untrusted input and was inlined
MATCH (u:User) WHERE u.id = 42 RETURN u`} />

Parameters are the only supported way to mix untrusted input into a
query. They also let the query planner cache plans across invocations.

### Parameter types

| Host value | LoraDB type |
|---|---|
| `null` / `None` / `undefined` | [`Null`](/docs/data-types/scalars#null) |
| `bool` | [`Boolean`](/docs/data-types/scalars#boolean) |
| `int` (Python) / `number` (JS, integer) / `i64` (Rust) | [`Integer`](/docs/data-types/scalars#integer) |
| `float` (Python) / `number` (JS, non-integer) / `f64` (Rust) | [`Float`](/docs/data-types/scalars#float) |
| `str` / `String` | [`String`](/docs/data-types/scalars#string) |
| list / array / `Vec` | [`List`](/docs/data-types/lists-and-maps#lists) |
| dict / object / `BTreeMap` | [`Map`](/docs/data-types/lists-and-maps#maps) |
| host helpers (`date()`, `wgs84()`, …) | [`Date`](/docs/data-types/temporal), [`Point`](/docs/data-types/spatial), etc. |

Host helpers are binding APIs. In query text, use cast syntax for
typed construction, for example `'2026-05-01'::DATE` or
`{x: 1, y: 2}::POINT`.

## What's not supported

See [**Limitations**](/docs/limitations) for the full list. Short
version: no general-purpose `CALL`, no `LOAD CSV`, and no
multi-database (`USE`). Index and constraint DDL are supported for scoped
catalog entries; see [Indexes](/docs/queries/indexes) and [Constraints](/docs/queries/constraints).

## See also

- [**Ten-Minute Tour**](/docs/getting-started/tutorial) — guided walkthrough.
- [**Cheat sheet**](/docs/queries/cheat-sheet) — single-page quick reference.
- [**Parameters**](/docs/queries/parameters) — typed parameter binding.
- [**Query Examples**](/docs/queries/examples) — copy-paste recipes by shape.
- [**Cookbook**](/docs/cookbook) — scenario-driven recipes.
- [**Functions**](/docs/functions/overview) — every built-in.
- [**Data types**](/docs/data-types/overview) — value shapes for parameters and properties.
- [**Graph model**](/docs/concepts/graph-model) — the underlying data model.
- [**Result formats**](/docs/concepts/result-formats) — how results come back over the wire.
