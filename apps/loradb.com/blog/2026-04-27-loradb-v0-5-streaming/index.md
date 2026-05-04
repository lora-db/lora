---
slug: loradb-v0-5-streaming
title: "LoraDB v0.5: streaming queries, property indexes, and faster bindings"
description: "LoraDB v0.5 turns the engine from whole-result execution toward pull-based streaming, property indexes, owned result streams, and binding APIs that can handle larger graph workloads without forcing everything through one materialized response."
authors: [loradb]
tags: [release-notes, announcement, performance, cypher]
---

LoraDB v0.5 is the release where the engine starts to breathe under
larger result sets.

The first public releases were about making the model real: an in-memory
graph, Cypher-shaped queries, vectors, snapshots, and then a WAL. v0.5
moves a level deeper. It changes how rows move through the executor, how
common property lookups avoid scans, and how bindings expose results
without requiring every query to become one large JSON payload.

The product promise is still the same: keep the graph close to the
application, make the hot path fast, and keep the system small enough to
understand. v0.5 makes that promise more practical once the graph stops
being a demo-sized toy.

<!-- truncate -->

## What Changed

The short version:

- a pull-based streaming executor for query results;
- transactional query streams in the database layer;
- owned query streams that can cross binding boundaries safely;
- client stream APIs for Node, WebAssembly, Python, and Go;
- snapshot helper updates across bindings;
- graph storage property indexes;
- memory indexing improvements;
- stronger archive-backed WAL persistence;
- follow-up fixes across v0.5.1 through v0.5.6 for Node handles, WAL
  working directories, temporary paths, and persistence edge cases.

That patch-release tail matters. v0.5 was not one neat switch. It was a
weekend of making the new execution shape work across the surfaces people
actually install.

## Why Streaming Came Next

Before v0.5, the natural API shape was "run a query, get the result."
That is useful, simple, and exactly right for small graphs. It also has a
ceiling: every result has to be produced, stored, converted, and returned
before the caller can do anything useful with the first row.

Graph queries make that ceiling show up early. A traversal can discover a
large neighborhood even when the application only wants to page, filter,
or process rows incrementally. If the executor has to materialize the
whole thing at once, memory usage starts to reflect the worst moment of
the query instead of the shape the caller actually needs.

v0.5 starts moving the engine toward this model:

```cypher
MATCH (account:Account)-[:OWNS]->(ticket:Ticket)
WHERE account.id = $account_id
RETURN ticket.id, ticket.status, ticket.priority
ORDER BY ticket.priority DESC
```

The database should be able to produce rows as the plan is pulled, and
the binding should be able to hand those rows to the host runtime without
pretending the result has to be one giant object.

## What The Pull-Based Executor Means

The pull executor changes the flow from "plan runs to completion" to
"the consumer asks for the next row."

That matters for three reasons.

First, it makes memory behavior easier to reason about. A query can carry
the current row, the current traversal state, and the current operator
state instead of eagerly building the full output.

Second, it creates a better API boundary for bindings. Node, Python, Go,
and WASM all have different ideas about iteration, async work, and
backpressure. Owned streams give each runtime a safer handle to wrap.

Third, it is the shape future performance work wants. Once the executor
has a pull contract, improvements like paging, cancellation, and more
selective operators have a place to attach.

## Property Indexes Move The Store Closer To The Query

v0.5 also adds graph storage property indexes.

The early LoraDB store was deliberately simple. That was the right first
move: prove the model, keep the code readable, and only add shortcuts
where the query engine has shown the need. By v0.5, a few common query
shapes were loud enough:

```cypher
MATCH (u:User)
WHERE u.id = $id
RETURN u
```

```cypher
MATCH (d:Doc)
WHERE d.slug = $slug
RETURN d.title
```

Those should not feel like whole-graph scans. Property indexes make the
storage layer more graph-aware without changing the Cypher surface. The
developer still writes the same query; the engine gets a better access
path.

That is the LoraDB pattern in miniature: keep the public model steady,
then make the internals earn their place.

## Binding Streams

The binding work is part of the release, not an afterthought.

LoraDB is only developer-first if the non-Rust surfaces feel first-class.
v0.5 extends the stream story through:

- Node / TypeScript query streams;
- WebAssembly worker streams and snapshot helpers;
- Python sync and async stream surfaces;
- Go streams, transactions, and snapshot byte helpers;
- Rust owned query streams in the database crate.

The details differ by runtime, but the journey is the same: start with a
local graph, query it with Cypher, and process results in the shape your
application already understands.

## Persistence Fixes In The v0.5 Patch Train

v0.4 introduced WAL-backed persistence. v0.5 made that path more useful
and then spent several patches tightening the practical edges.

The patch train covered:

- archive WAL persistence improvements;
- Node multiple-database handle fixes;
- WAL working-directory fixes;
- temporary-path fixes;
- durability updates for archive-backed opens.

That is part of the story worth saying plainly. Persistence is not a
single checkbox. It becomes trustworthy through boring fixes: paths,
handles, flushes, recovery fences, and reopening the same graph the way
real applications reopen it.

## How v0.5 Fits The Journey

The first four releases answered "can I model, query, save, and recover
the graph?"

v0.5 starts answering "can I keep using it when the result set gets
large, the bindings matter, and common lookups need to stay cheap?"

It is a performance release, but not only in the benchmark sense. It is a
product-feel release. The graph can be closer to the application because
the application does not have to wait for every row before touching the
first one.

## Read Next

- [Performance](/docs/performance)
- [Node guide](/docs/getting-started/node)
- [Python guide](/docs/getting-started/python)
- [Go guide](/docs/getting-started/go)
- [Snapshots](/docs/snapshot)

v0.3 gave LoraDB a file. v0.4 gave it a log. v0.5 gives the query path a
stream.
