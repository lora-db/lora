---
title: What is LoraDB
sidebar_label: What is LoraDB
slug: /
description: LoraDB is a local-first, in-memory property-graph engine in Rust that speaks a pragmatic subset of Cypher, with Node.js / Python / WASM / Go / Ruby bindings, snapshots on every binding, and optional WAL-backed durability on every filesystem-backed surface.
---

# What is LoraDB

LoraDB is a **local-first, in-memory property-graph engine** written
in Rust that speaks a pragmatic subset of Cypher. It runs in-process
inside your service, pipeline, or agent — no separate database tier
— and reaches you through a Rust crate, five bindings, or an HTTP
server.

It is:

- **A query engine.** Parser, analyzer, planner, optimizer, and
  executor all in one crate.
- **An in-process graph store.** Nodes, relationships, and properties
  held in RAM.
- **A set of bindings over one shared core.** Node, Python, WASM, Go
  (via a shared C ABI), and Ruby, plus an Axum-based HTTP server.

It is **not**:

- A drop-in replacement for other graph databases. The Cypher surface
  is a scoped subset — see [Limitations](./limitations) for what's
  in and out.
- A product suite. It's a crate you embed, not a service you operate.
- A durable, clustered database tier — local WAL-backed durability
  exists on some surfaces, but the engine is still single-process and
  intentionally small. See [the engine's boundaries](#the-engines-boundaries)
  below.

For the longer-form positioning — why an embedded graph at all, and
how LoraDB compares against managed graph DBs, SQL, and document
stores — see [**Why LoraDB**](./why).

## Who it's for

| Workload | Why LoraDB fits |
|---|---|
| **Backend services** | A graph view over already-owned storage — permissions, org charts, supply chains, lineage — without a second database tier. |
| **AI agents and LLM pipelines** | Entities, observations, tool calls, and decisions as typed traversals rather than ad-hoc JSON. [`VECTOR`](./data-types/vectors) is a first-class value, so embeddings live on the same node as labels and edges — similarity and traversal share one query. |
| **Robotics and stateful systems** | Scenes, maps, tasks, and dependencies as a graph. Running in the controller's process avoids cross-service latency on the control loop. |
| **Event-driven / real-time pipelines** | Entity resolution, relationship inference, and path queries over streams — in-memory, alongside the handler. |
| **Notebooks, CLIs, tests, research tooling** | A Cypher-capable graph you open in one line of code. No Docker, no auth, no network hop. |

## Why it fits modern workloads

Agents, robots, and streaming pipelines all end up building the same
structure by accident: entities with typed keys, evolving relations,
accessed in-process. Three properties make an in-memory graph a good
fit for that structure:

- **Context is relational.** What matters is rarely a row; it's how
  rows connect. A graph model states that directly.
- **Context changes.** Schemas shift as the system learns. LoraDB is
  schema-free — new labels and properties come into existence the
  first time you write them.
- **Context must stay close.** Reasoning that crosses a network
  boundary is slower and less reliable. Running in-process removes
  the boundary.

## From zero to first query

Four steps. Pick your host language on step 2; everything else is
identical.

### 1. Install

| Host | Command |
|---|---|
| [Node / TypeScript](./getting-started/node) | `npm install @loradb/lora-node` |
| [Python](./getting-started/python) | `pip install lora-python` |
| [Browser / WASM](./getting-started/wasm) | `npm install @loradb/lora-wasm` |
| [Go](./getting-started/go) | `go get github.com/lora-db/lora/crates/lora-go` |
| [Ruby](./getting-started/ruby) | `gem install lora-ruby` |
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
| Build a Go service or CLI (cgo) | [Go binding](./getting-started/go) |
| Ship a Ruby app or Rails service | [Ruby binding](./getting-started/ruby) |
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
| [**Data types**](./data-types/overview) | Scalars, lists, maps, temporals, spatial points, [vectors](./data-types/vectors) — how each round-trips. |
| [**HTTP API**](./api/http) | Endpoint reference for `lora-server`. |
| [**Cookbook**](./cookbook) | Scenario-driven recipes: social graphs, e-commerce, events, geospatial, [backup and restore](./cookbook#backup-and-restore). |
| [**Snapshots**](./snapshot) | Save / load the full graph as a single file — every binding, plus the opt-in HTTP admin surface. |
| [**WAL & checkpoints**](./wal) | Continuous durability on Rust, Node, Python, Go, Ruby, and `lora-server` — plus full operator controls on Rust and the server. |
| [**Limitations**](./limitations) | What's not supported — binding-level WAL-control asymmetry, no indexes, no `CALL`, etc. |
| [**Troubleshooting**](./troubleshooting) | Common errors and the shortest path out. |

## The engine's boundaries

Every item below is a deliberate trade-off, not an oversight:

- **Durability depends on the surface.** Every binding can
  [save / load snapshots](./snapshot). Every filesystem-backed
  surface can also attach a [WAL](./wal) for continuous durability
  between checkpoints. WASM remains snapshot-only. The engine is
  still an in-memory, single-process system — not a separate
  persistent storage tier.
- **No property indexes.** `MATCH (n {prop: v})` without a label is `O(n)`.
- **No uniqueness constraints.** Use [`MERGE`](./queries/unwind-merge#merge)
  on a key, or enforce in application code.
- **Global mutex.** Queries serialise — concurrent reads don't parallelise.
- **No HTTP auth / TLS.** Bind the server to localhost or put it behind
  a reverse proxy. The opt-in admin snapshot and WAL endpoints also ship
  without auth — see [Limitations → HTTP server](./limitations#http-server).
- **No HTTP-level parameters yet.** Bind via the in-process bindings;
  see [Parameters](./queries/parameters#http-api-doesnt-forward-params).

Full list in [**Limitations**](./limitations).

## Help and community

- [**Troubleshooting**](./troubleshooting) — first stop when something
  breaks.
- [**GitHub**](https://github.com/lora-db/lora) — source, issues,
  discussions.
- [**Discord**](https://discord.gg/vUgKb6C8Af) — ask a question or
  lurk on updates.
