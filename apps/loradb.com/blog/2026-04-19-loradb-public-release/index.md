---
slug: loradb-public-release
title: "LoraDB public release: a fast in-memory graph database in Rust"
description: "The first public release of LoraDB, what is included, how to try it, the license model, and where the project goes next."
authors: [joost]
tags: [release-notes, announcement, architecture, cypher]
---

LoraDB is now public.

It is a fast in-memory graph database written in Rust, with a Cypher-shaped
query engine, an HTTP API, and bindings for Node.js, WebAssembly, and Python.
It is built for developers who need relationship queries close to their
application without adopting a large graph database stack on day one.

This release is the beginning of the public journey: source-available core,
developer-first adoption, and a path toward a hosted platform for teams that
want managed operations later.

<!-- truncate -->

## What LoraDB Is

LoraDB is an in-memory property graph database.

It stores:

- nodes with labels and properties;
- relationships with a type, direction, endpoints, and properties;
- scalar values, lists, maps, temporal values, and spatial points;
- query results in row, graph, row-array, and combined formats.

It speaks a Cypher-like query language because Cypher is still one of the best
ways to express graph patterns:

```cypher
MATCH (person:Person)-[:KNOWS]->(friend:Person)
WHERE person.name = $name
RETURN friend.name, friend.age
ORDER BY friend.age DESC
LIMIT 10
```

Under the hood, LoraDB is split into small Rust crates:

- `lora-ast` for query syntax structures;
- `lora-parser` for parsing;
- `lora-analyzer` for semantic analysis;
- `lora-compiler` for logical and physical planning;
- `lora-executor` for running plans;
- `lora-store` for graph storage;
- `lora-database` for the database entry point;
- `lora-server` for the HTTP server;
- `lora-node`, `lora-wasm`, and `lora-python` for language bindings.

The goal is not to hide the database behind a giant internal system. The goal
is to make the engine readable enough that developers can understand how a
query moves from text to result.

## Why It Exists

LoraDB exists because many graph workloads need to be fast, local, and
efficient before they need to be distributed.

Existing graph databases like Neo4j are powerful, but for the workloads that
started this project, they felt too heavy. I wanted a database that could live
in the application loop, load quickly, run in memory, and make storage costs
easy to reason about.

That means LoraDB is optimized for a specific first experience:

1. Clone the repo.
2. Run the server.
3. Load a graph.
4. Write a Cypher query.
5. Understand the result and the code path behind it.

The hosted platform will come later. The core has to earn developer trust
first.

## What You Can Do Today

### Run the server

```bash
cargo run --bin lora-server
```

By default, the server listens on `127.0.0.1:4747`.

Send a query:

```bash
curl -X POST http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"CREATE (:Person {name: \"Ada\"}) RETURN 1 AS ok"}'
```

Check health:

```bash
curl http://127.0.0.1:4747/health
```

### Use the Rust API

The Rust API is the most direct way to embed LoraDB in another Rust program:

```rust
use lora_database::Database;

let db = Database::new();
let rows = db.execute("MATCH (n) RETURN n LIMIT 10")?;
```

### Use language bindings

This repository includes package work for:

- Node.js / TypeScript through `crates/lora-node`;
- WebAssembly through `crates/lora-wasm`;
- Python through `crates/lora-python`.

The bindings are part of the same public release because the customer journey
should not stop at Rust. Graph workloads show up in web apps, notebooks,
automation tools, agent runtimes, and backend services.

## Query Support

This release includes a substantial Cypher-shaped query surface:

- `MATCH` and `OPTIONAL MATCH`;
- `WHERE`;
- `RETURN`;
- `WITH`;
- `ORDER BY`, `SKIP`, `LIMIT`, and `DISTINCT`;
- `UNWIND`;
- `UNION` and `UNION ALL`;
- `CREATE`;
- `SET`;
- `DELETE` and `DETACH DELETE`;
- `REMOVE`;
- `MERGE` with `ON CREATE` and `ON MATCH`;
- variable-length paths;
- `shortestPath()` and `allShortestPaths()`;
- aggregation functions;
- list, string, math, temporal, spatial, conversion, and entity functions.

It also supports parameter binding through the Rust API and typed values for
common graph workloads.

## Storage And Execution

The storage layer is intentionally in-memory. That is the point of this first
release.

LoraDB is designed around:

- cheap local iteration;
- predictable graph traversal;
- explicit query stages;
- small intermediate representations;
- clear Rust ownership boundaries;
- enough structure to evolve toward persistence without hiding current costs.

The planner and executor are still young, but they already handle meaningful
graph patterns, projection, filtering, aggregation, updates, and paths. The
project is written so performance work can happen in the open, with the storage
and execution model visible to contributors.

## Documentation

The documentation site includes:

- getting started guides;
- query language pages;
- function references;
- data type references;
- architecture notes;
- performance notes;
- troubleshooting;
- known limitations.

The root repository also includes architecture, testing, operations, and
release documentation for contributors who want to understand or improve the
engine.

## License

The LoraDB core is licensed under the Business Source License 1.1.

The license allows:

- development use;
- non-production use;
- internal business use;
- internal production systems;
- reading, modifying, and distributing the source under the BSL terms.

The license does not allow using the core to offer LoraDB as
database-as-a-service, a hosted API for third parties, a competing managed
database platform, or a hosted resale product.

Each covered release converts to Apache License 2.0 on the Change Date listed
in the root `LICENSE` file. For this release policy, that date is April 19,
2029.

The documentation website under `apps/loradb.com` is separately MIT licensed.

The goal is simple: developers should be able to adopt and trust the core,
while the hosted platform business remains sustainable.

## What Is Not Included Yet

This release is intentionally honest about what it is not.

LoraDB does not yet include:

- durable disk persistence;
- WAL or snapshots;
- clustering;
- replication;
- authentication;
- TLS termination;
- transactions across concurrent clients;
- property indexes for every workload;
- a managed cloud service.

The HTTP server is useful for local development, internal experiments, and
controlled environments. It is not yet a hardened internet-facing database
server.

Those limitations are not hidden. They are the roadmap.

## Who Should Try It

Try LoraDB if you are:

- building an internal tool with relationship-heavy data;
- experimenting with graph memory for agents;
- prototyping a knowledge graph;
- looking for a Rust graph database engine you can read;
- evaluating whether Cypher-shaped queries fit your product;
- tired of starting with a large graph stack before you know the model works.

Do not choose LoraDB yet if you need mature distributed operations, long-term
durability, multi-region replication, or hardened public database hosting
today.

## What Comes Next

The next phase has three tracks.

### 1. Core database maturity

More planner work, better indexing, tighter memory usage, clearer error
messages, and deeper test coverage for Cypher behavior.

### 2. Persistence

The in-memory engine is the foundation. The next durable layer should preserve
the simplicity of the current store while adding snapshots, recovery, and a
path toward production workloads that outlive a process.

### 3. Hosted platform

The long-term product is a hosted LoraDB platform for teams that want the graph
model without operating the database themselves. That means managed projects,
backups, metrics, auth, scaling, and support.

The public core and the hosted product are not in conflict. The core creates
developer adoption. The hosted platform turns that adoption into a sustainable
business.

## Closing

LoraDB started from a practical frustration: I needed a graph database that was
fast enough to live in memory, efficient enough to trust, and small enough to
understand.

This public release is the first serious step toward that goal.

If you try it, the most useful feedback is concrete:

- what graph did you load;
- which query did you expect to be easy;
- where did performance surprise you;
- which limitation blocked you;
- which docs page did you wish existed.

That feedback will shape the next release.

Welcome to LoraDB.
