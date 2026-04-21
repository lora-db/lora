---
title: What is LoraDB
sidebar_label: What is LoraDB
slug: /
---

# What is LoraDB

LoraDB is a **local-first, in-memory property-graph engine** written
in Rust. You embed it in your Rust / Node / Python / WASM host, or
talk to it over HTTP, and query it with a pragmatic subset of
**Cypher**.

It is:

- **A query engine** — parser, analyzer, planner, optimizer, and
  executor all in one crate.
- **An in-process graph store** — nodes, relationships, properties,
  all in RAM.
- **A set of bindings** over one shared Rust core — Node, Python,
  WebAssembly, plus an Axum-based HTTP server.

It is **not**:

- A durable, clustered production database. There's no persistence
  today — state is lost on restart.
- A multi-tenant server with auth or TLS. Bind to `127.0.0.1` or put
  it behind a proxy.
- A drop-in replacement for Neo4j. LoraDB speaks Cypher, but a
  scoped subset — see [Limitations](./limitations) for the exact
  shape.

## Who it's for

- Developers who want a graph database **embedded** in their process
  rather than running one as a service.
- Engineers prototyping or shipping a **local-first** app that
  benefits from graph queries (Electron, browser worker, CLI, edge).
- Teams evaluating graph data models who want a fast, zero-ops
  starting point — write Cypher, read rows, keep moving.

## From zero to first query

Four steps. Pick your host language on step 2; everything else is
identical.

### 1. Install

| Host | Command |
|---|---|
| [Node / TypeScript](./getting-started/node) | `npm install lora-node` |
| [Python](./getting-started/python) | `pip install lora-python` |
| [Browser / WASM](./getting-started/wasm) | `npm install lora-wasm` |
| [Rust (embedded)](./getting-started/rust) | `cargo add lora-database` |
| [HTTP server](./getting-started/server) | `cargo install --path crates/lora-server` |

:::note

Pre-release: packages aren't yet on npm / PyPI / crates.io. Each
platform guide includes repo-local build steps.

:::

### 2. Create data

```cypher
CREATE (ada:Person   {name: 'Ada',   born: 1815})
CREATE (grace:Person {name: 'Grace', born: 1906})
CREATE (ada)-[:INFLUENCED {year: 1843}]->(grace)
```

One node per `CREATE (…)`. Relationships have a type, direction, and
their own properties. See [Graph model](./concepts/graph-model).

### 3. Query

```cypher
MATCH (a:Person)-[:INFLUENCED]->(b:Person)
WHERE a.born < 1900
RETURN a.name AS influencer, b.name AS influenced
```

Clauses stream rows: `MATCH` finds patterns, `WHERE` filters, `RETURN`
projects. See [Queries → Overview](./queries/) or jump into the
[**Cheat sheet**](./queries/cheat-sheet) for a single-page reference.

### 4. Choose an API

| If you… | Use |
|---|---|
| Ship Node / TS code | [Node binding](./getting-started/node) |
| Write Python (sync or asyncio) | [Python binding](./getting-started/python) |
| Run in a browser / Web Worker / edge | [WASM binding](./getting-started/wasm) |
| Embed inline in a Rust binary | [Rust crate](./getting-started/rust) |
| Want a polyglot HTTP service | [HTTP server](./getting-started/server) + [HTTP API reference](./api/http) |

All bindings share the same query language and result shapes — see
[Result formats](./concepts/result-formats) for the four response
shapes (`rows`, `rowArrays`, `graph`, `combined`).

## What you'll read next

| Section | What's inside |
|---|---|
| [**Tutorial**](./getting-started/tutorial) | A ten-minute guided tour — create, match, filter, aggregate, paths, CASE. |
| [**Concepts**](./concepts/graph-model) | Graph model, nodes, relationships, properties, [schema-free](./concepts/schema-free), [result formats](./concepts/result-formats). |
| [**Queries**](./queries/) | Clause reference, [parameters](./queries/parameters), [cheat sheet](./queries/cheat-sheet). |
| [**Functions**](./functions/overview) | String, math, list, temporal, spatial, aggregation. |
| [**Data types**](./data-types/overview) | Scalars, lists, maps, temporals, spatial points — how each round-trips. |
| [**HTTP API**](./api/http) | Endpoint reference for `lora-server`. |
| [**Cookbook**](./cookbook) | Scenario-driven recipes: social graphs, e-commerce, events, geospatial. |
| [**Limitations**](./limitations) | What isn't supported — no persistence, no indexes, no `CALL`, etc. |
| [**Troubleshooting**](./troubleshooting) | Common errors and the shortest path out. |

## The engine's boundaries

Honest up front:

- **No persistence.** All state is in-memory; `kill -9` loses it.
- **No property indexes.** `MATCH (n {prop: v})` without a label is
  `O(n)`.
- **No uniqueness constraints.** Use [`MERGE`](./queries/unwind-merge#merge)
  on a key, or enforce in application code.
- **Global mutex.** Queries serialise — concurrent reads don't
  parallelise.
- **No HTTP auth / TLS.** Run the server on localhost or behind a
  reverse proxy.
- **No HTTP-level parameters yet.** Bind via the embedded bindings;
  see [Parameters](./queries/parameters#http-api-doesnt-forward-params).

None of these are accidents — see [Why LoraDB](/why) for the
positioning.

## Help and community

- [**Troubleshooting**](./troubleshooting) — first stop when something
  breaks.
- [**GitHub**](https://github.com/lora-db/lora) — source, issues,
  discussions.
- [**Discord**](https://discord.gg/vUgKb6C8Af) — ask a question or
  lurk on updates.
