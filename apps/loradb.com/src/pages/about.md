---
title: About LoraDB
description: What LoraDB is, who it's for, and where it fits — an embedded graph database for systems that reason over connected, evolving context.
---

# About LoraDB

LoraDB is an **in-memory graph database** with a **Cypher-like query
engine**, written in Rust. It's built for services, pipelines, and
agents that model richly connected data and need to query it in-process
without standing up a separate database.

## What it is

A small, embeddable core — not a server, not a cluster, not a product
suite. You pull it in as a crate or binding, open a database, and query
it.

- **Labeled property graph** — nodes with labels, relationships with
  types, properties on both. See
  [Graph Model](/docs/concepts/graph-model).
- **Cypher-like queries** — <CypherCode code="MATCH" />,
  <CypherCode code="CREATE" />, <CypherCode code="WHERE" />,
  <CypherCode code="WITH" />, <CypherCode code="OPTIONAL MATCH" />,
  <CypherCode code="MERGE" />, aggregation, shortest paths.
- **Four bindings, one engine** — Rust, Node/TypeScript, Python, WASM,
  plus an HTTP server.
- **Pure Rust** — no C dependencies, no external database process.

## Why it exists

Most data is relational by nature, but only a fraction of it deserves
a separate database service. Graph platforms are great when you need a
graph platform. They're disproportionate when you need a graph _data
structure_ that speaks Cypher and lives inside an application you
already have.

LoraDB is the missing option in that second direction — small enough
to read in an afternoon, predictable enough to trust inside a hot
path, expressive enough to model real domains.

## Who it's for

### Backend services

Services that already own their storage and want a graph view over it
— permissions, org charts, supply chains, lineage — without running a
second database tier.

### AI agents and LLM pipelines

Agents accumulate context: entities, relations, observations,
decisions, tool calls. Storing that as a graph gives you typed
traversal (`what does this agent know about entity X?`) instead of
ad-hoc JSON lookups. LoraDB's in-process model keeps the memory close
to the reasoning loop.

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

## What makes it fit modern workloads

Modern systems — agents, robots, real-time pipelines — share three
things:

- **Context is relational.** What matters is rarely a row; it's how
  rows connect. A graph model states that directly.
- **Context changes.** Schemas shift as the system learns. LoraDB is
  schema-free — new labels and properties come into existence the
  first time you write them.
- **Context must stay close.** Reasoning that crosses a network
  boundary is reasoning that's slower and less reliable. An embedded
  engine removes the boundary.

None of that replaces a durable, distributed graph platform. It covers
the gap underneath one.

## Start here

- [**What is LoraDB**](/docs) — the docs landing page.
- [**Graph Model**](/docs/concepts/graph-model) — the data model in
  four queries.
- [**Installation**](/docs/getting-started/installation) — pick a
  binding and run a query in a minute.
- [**Why LoraDB**](/why) — the longer-form case.
