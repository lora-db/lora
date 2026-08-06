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
  executor live in separate Rust crates with one shared pipeline.
- **An in-process graph store.** Nodes, relationships, and properties
  held in RAM.
- **A set of bindings over one shared core.** Node, Python, WASM, Go
  (via a shared C ABI), and Ruby, plus an Axum-based HTTP server.

It is **not**:

- A drop-in replacement for other graph databases. The Cypher surface
  is a scoped subset — see [Limitations](/docs/limitations) for what's
  in and out.
- A product suite. The source-available core is something you embed or run
  locally, not a managed service you operate as-is.
- A durable, clustered database tier — local WAL-backed durability
  exists on filesystem-backed surfaces, but the engine is still
  single-process and intentionally small. See
  [the engine's boundaries](#the-engines-boundaries) below.

For the longer-form positioning — why an embedded graph at all, and
how LoraDB compares against managed graph DBs, SQL, and document
stores — see [**Why LoraDB**](/docs/why).

## Who it's for

| Workload | Why LoraDB fits |
|---|---|
| **Backend services** | A graph view over already-owned storage — permissions, org charts, supply chains, lineage — without a second database tier. |
| **AI agents and LLM pipelines** | Entities, observations, tool calls, and decisions as typed traversals rather than ad-hoc JSON. [`VECTOR`](/docs/data-types/vectors) is a first-class value, so embeddings live on the same node as labels and edges — similarity and traversal share one query. |
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
identical. To skip step 1, open the
[**playground**](/docs/getting-started/playground) — it runs LoraDB compiled
to WASM in your browser, no install, no account.

If this is your first run, stay in memory until the query shape feels
right. Plain in-memory handles start empty on every process run; move
to a named `.loradb` archive, snapshots, or WAL when the graph should
survive restarts.

### 1. Install

| Host | Command |
|---|---|
| [Node / TypeScript](/docs/getting-started/node) | `npm install @loradb/lora-node` |
| [Python](/docs/getting-started/python) | `pip install lora-python` |
| [Browser / WASM](/docs/getting-started/wasm) | `npm install @loradb/lora-wasm` |
| [Go](/docs/getting-started/go) | `go get github.com/lora-db/lora/crates/bindings/lora-go` |
| [Ruby](/docs/getting-started/ruby) | `gem install lora-ruby` |
| [Rust (embedded)](/docs/getting-started/rust) | `cargo add lora-database` |
| [HTTP server](/docs/getting-started/server) | `cargo install lora-server` |

### 2. Create data

<QueryCodeBlock code={String.raw`CREATE (ada:Person   {name: 'Ada',   born: 1815})
CREATE (grace:Person {name: 'Grace', born: 1906})
CREATE (ada)-[:INFLUENCED {year: 1843}]->(grace)`} />

One node per `CREATE (…)`. Relationships have a type, direction, and
their own properties. See [Graph model](/docs/concepts/graph-model).

### 3. Query

<QueryCodeBlock code={String.raw`MATCH (a:Person)-[:INFLUENCED]->(b:Person)
WHERE a.born < 1900
RETURN a.name AS influencer, b.name AS influenced`} />

Clauses stream rows: `MATCH` finds patterns, `WHERE` filters, `RETURN`
projects. See [Queries → Overview](/docs/queries) or jump into the
[**Cheat sheet**](/docs/queries/cheat-sheet) for a single-page reference.

### 4. Choose an API

| If you… | Use |
|---|---|
| Ship Node / TS code | [Node binding](/docs/getting-started/node) |
| Write Python (sync or asyncio) | [Python binding](/docs/getting-started/python) |
| Run in a browser / Web Worker / edge | [WASM binding](/docs/getting-started/wasm) |
| Build a Go service or CLI (cgo) | [Go binding](/docs/getting-started/go) |
| Ship a Ruby app or Rails service | [Ruby binding](/docs/getting-started/ruby) |
| Embed inline in a Rust binary | [Rust crate](/docs/getting-started/rust) |
| Want a polyglot HTTP service | [HTTP server](/docs/getting-started/server) + [HTTP API reference](/docs/api/http) |

All bindings share the same query language and result shapes — see
[Result formats](/docs/concepts/result-formats) for the four response
shapes (`rows`, `rowArrays`, `graph`, `combined`).

## First-time decisions

Most confusion comes from choosing the wrong surface for the job, or
expecting an in-memory graph to persist automatically. Use this table
before wiring LoraDB into an app:

| Need | Pick | Why |
|---|---|---|
| Learn the query language | [Playground](/docs/getting-started/playground) | No install, browser-local state, good for copy-paste experiments. |
| Build an app in one language | The matching [binding](/docs/getting-started/installation) | Real parameters, typed values, snapshots, and host-language error handling. |
| Share one local graph across languages | [HTTP server](/docs/getting-started/server) | Simple `POST /query` API from any client. |
| Accept untrusted user input | Parameters on any binding or HTTP `/query` | Never interpolate raw input into query text. |
| Survive process restarts | Named `.loradb` archive, snapshots, or [WAL](/docs/wal) | Plain in-memory databases intentionally start empty. |
| Expose over a network | Put `lora-server` behind authenticated ingress | Built-in HTTP auth, TLS, and rate limiting are not included. |

## What you'll read next

| Section | What's inside |
|---|---|
| [**Playground**](/docs/getting-started/playground) | Run LoraDB queries in your browser, inspect graph/table/JSON/analysis views, share query URLs, and export snapshots. |
| [**Tutorial**](/docs/getting-started/tutorial) | A ten-minute guided tour — create, match, filter, aggregate, paths, CASE. |
| [**Concepts**](/docs/concepts/graph-model) | Graph model, nodes, relationships, properties, [schema-free](/docs/concepts/schema-free), [result formats](/docs/concepts/result-formats). |
| [**Queries**](/docs/queries) | Clause reference, [parameters](/docs/queries/parameters), [cheat sheet](/docs/queries/cheat-sheet). |
| [**Functions**](/docs/functions/overview) | String, math, list, aggregation, temporal, spatial, vector, type, and cast helpers. |
| [**Data types**](/docs/data-types/overview) | Scalars, lists, maps, temporals, spatial points, [vectors](/docs/data-types/vectors) — how each round-trips. |
| [**HTTP API**](/docs/api/http) | Endpoint reference for `lora-server`. |
| [**Cookbook**](/docs/cookbook) | Scenario-driven recipes: social graphs, e-commerce, events, geospatial, [backup and restore](/docs/cookbook#backup-and-restore). |
| [**Snapshots**](/docs/snapshot) | Save / load the full graph as a file or byte payload — every binding, plus the opt-in HTTP admin surface. |
| [**WAL & checkpoints**](/docs/wal) | Continuous durability on Rust, Node, Python, Go, Ruby, and `lora-server` — with full operator controls on Rust and the server. |
| [**Performance**](/docs/performance) | Benchmark tables, CI `benchmark-summary.json`, and how to read regression signals. |
| [**Errors**](/docs/errors) | Error codes, diagnostic shape, and how to read parser / analyzer / runtime failures. |
| [**Limitations**](/docs/limitations) | What's not supported - binding-level WAL-control asymmetry, limited `CALL`, no ANN vector index structure, etc. |
| [**Troubleshooting**](/docs/troubleshooting) | Common errors and the shortest path out. |

## The engine's boundaries

Every item below is a deliberate trade-off, not an oversight:

- **Durability depends on the surface.** Every binding can
  [save / load snapshots](/docs/snapshot). Filesystem-backed surfaces can
  also attach a [WAL](/docs/wal) for continuous durability between
  checkpoints or managed commit-count snapshots. WASM remains
  snapshot-only and pathless. The engine is still an in-memory,
  single-process system — not a separate persistent storage tier.
- **Indexes are explicit but scoped.** `CREATE INDEX`, `CREATE TEXT INDEX`,
  `CREATE POINT INDEX`, `CREATE VECTOR INDEX`, `CREATE FULLTEXT INDEX`,
  `DROP INDEX`, and `SHOW INDEXES` exist for node and relationship scopes.
  Vector search currently uses flat scan execution from the indexed scope;
  a dedicated ANN structure is still future work.
- **Constraints are optional.** Use
  [`CREATE CONSTRAINT`](/docs/queries/constraints) for uniqueness, existence,
  node keys, relationship keys, and property type checks when a label or
  relationship type needs stronger guarantees.
- **Single-process concurrency.** Auto-commit reads can overlap on Arc snapshots;
  write commits and explicit read-write transactions serialize.
- **No HTTP auth / TLS.** Bind the server to localhost or put it behind
  a reverse proxy. The opt-in admin snapshot and WAL endpoints also ship
  without auth — see [Limitations → HTTP server](/docs/limitations#http-server).
- **HTTP is intentionally small.** `/query`, `/explain`, and
  `/profile` accept JSON parameters, but HTTP has no multi-query
  transaction handles and no built-in auth / TLS. Use an in-process
  binding for direct embedding and host-language typed helpers.

Full list in [**Limitations**](/docs/limitations).

## Help and community

- [**Troubleshooting**](/docs/troubleshooting) — first stop when something
  breaks.
- [**GitHub**](https://github.com/lora-db/lora) — source, issues,
  discussions.
- [**Discord**](https://discord.gg/vUgKb6C8Af) — ask a question or
  lurk on updates.
