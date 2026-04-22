---
title: What is LoraDB
sidebar_label: What is LoraDB
slug: /
---

# What is LoraDB

LoraDB is a **local-first, in-memory property-graph engine** written
in Rust, built for services, pipelines, and agents that model richly
connected data and need to query it in-process without standing up a
separate database. You embed it in your Rust, Node, Python, WASM, Go,
or Ruby host — or talk to it over HTTP — and query it with a pragmatic
subset of **Cypher**.

It is:

- **A query engine** — parser, analyzer, planner, optimizer, and
  executor all in one crate.
- **An in-process graph store** — nodes, relationships, properties,
  all in RAM.
- **A set of bindings** over one shared Rust core — Node, Python,
  WebAssembly, Go (via a shared C ABI), and Ruby, plus an Axum-based
  HTTP server.

It is **not**:

- A drop-in replacement for Neo4j. LoraDB speaks Cypher, but a
  scoped subset — see [Limitations](./limitations) for the exact
  shape.
- A product suite. It's a crate you embed, not a service you operate.
- A durable, clustered database tier — see
  [the engine's boundaries](#the-engines-boundaries) below for the
  technical limits.

For the longer-form positioning — why an embedded graph at all, and
how LoraDB compares against managed graph DBs, SQL, and document
stores — see [**Why LoraDB**](./why).

## Who it's for

### Backend services

Services that already own their storage and want a graph view over it
— permissions, org charts, supply chains, lineage — without running a
second database tier.

### AI agents and LLM pipelines

Agents accumulate context: entities, relations, observations,
decisions, tool calls. Storing that as a graph gives you typed
traversal (`what does this agent know about entity X?`) instead of
ad-hoc JSON lookups. The in-process model keeps the memory close to
the reasoning loop.

### Robotics and stateful systems

Scenes, maps, tasks, and their dependencies change constantly. A graph
captures the structure; Cypher captures the question. Running in the
same process as the controller avoids cross-service latency on the
control loop.

### Event-driven and real-time pipelines

Entity resolution, relationship inference, and path queries over
streams — done in memory, alongside the code that produces the events.

### Notebooks, CLIs, tests, research tooling

A Cypher-capable graph you can open in one line of code. No Docker, no
auth, no network hop. Useful any time reaching for Neo4j or Memgraph
would be disproportionate to the task.

## Why it fits modern workloads

Agents, robots, and real-time pipelines all end up building an
in-memory structure of entities and relations with typed keys and an
evolving shape. Three properties make that structure a good fit for
an embedded graph:

- **Context is relational.** What matters is rarely a row; it's how
  rows connect. A graph model states that directly.
- **Context changes.** Schemas shift as the system learns. LoraDB is
  schema-free — new labels and properties come into existence the
  first time you write them.
- **Context must stay close.** Reasoning that crosses a network
  boundary is slower and less reliable. An embedded engine removes
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
| [**Data types**](./data-types/overview) | Scalars, lists, maps, temporals, spatial points — how each round-trips. |
| [**HTTP API**](./api/http) | Endpoint reference for `lora-server`. |
| [**Cookbook**](./cookbook) | Scenario-driven recipes: social graphs, e-commerce, events, geospatial. |
| [**Limitations**](./limitations) | What isn't supported — no persistence, no indexes, no `CALL`, etc. |
| [**Troubleshooting**](./troubleshooting) | Common errors and the shortest path out. |

## The engine's boundaries

Every item below is a deliberate trade-off, not an oversight — worth
knowing up front:

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

## Help and community

- [**Troubleshooting**](./troubleshooting) — first stop when something
  breaks.
- [**GitHub**](https://github.com/lora-db/lora) — source, issues,
  discussions.
- [**Discord**](https://discord.gg/vUgKb6C8Af) — ask a question or
  lurk on updates.
