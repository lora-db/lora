---
title: GraphQL Limits and Scaling
sidebar_label: Limits and scaling
description: What @loradb/lora-graphql does not do, every bound it enforces with its default, and how a server built on it scales, covering one writer, the Node worker pool, memory, durability and what has not been load tested.
keywords: [graphql limits, scaling, uv_threadpool_size, single writer, capacity planning]
---

# Limits and scaling

This page is the honest list: what the library does not do, every bound it
enforces, and how a server built on it behaves under load. Read it before
you plan a production deployment.

## Not supported

- **Offset pagination.** Connections page by keyset cursor only.
- **Unbounded lists.** Every list has a limit; 100 is the most a page
  returns unless you raise `maxLimit`.
- **Unbounded bulk writes.** `update<Plural>` and `delete<Plural>` need a
  `where` and are bounded by `limit`. Imports of more than `maxBatch`
  nodes belong in a Cypher load.
- **Everything on by default.** Mutations, subscriptions, filters and
  sorts are opt in, per type and per field.
- **JWT verification.** Your server verifies tokens and passes the claims.
- **Federation.**
- **Input object arguments on `@cypher` fields**, and user-defined
  `Subscription` fields. `Query` and `Mutation` fields are `@cypher`
  fields.
- **Aggregates, connections and single lookups on interfaces and
  unions.** They get a list root only.
- **Reading the properties of a single relationship.** Only list
  relationships have a connection. See
  [relationships](/docs/graphql/relationships#reading).
- **Relationship rules on a relationship to an interface or union.**
- **Plan checks for mutations and subscriptions.** `check` plans queries.
- **Mutations in the browser.** `@loradb/lora-wasm` serves reads;
  mutations, `explain()`, `check()` and the change feed need
  `@loradb/lora-node`.
- **Cross-process anything.** A database directory is open in one process
  at a time, so there is no cross-process change feed and no shared
  database between replicas. See [scaling](#scaling).
- **Subscriptions over outside writes, by default.** Without
  `changeFeed: true`, subscriptions see only writes made through the same
  `LoraGraphQL` instance.

## Every bound, in one place

All are constructor options unless marked. The
[API reference](/docs/graphql/api-reference#limits) describes each.

| Bound | Default | Over it |
| --- | --- | --- |
| Page size (`defaultLimit`, `maxLimit`, `@limit`) | 25, 100 | `LIMIT_EXCEEDED` |
| Estimated rows per operation (`maxCost`, `budget`) | 50,000 | `COST_EXCEEDED` |
| Statement time (`timeoutMs`) | 10 s | `DATABASE_ERROR` |
| Query time, all root fields (`operationTimeoutMs`) | 20 s | `TIMEOUT` |
| Statements in flight per operation (`maxConcurrentStatements`) | 2 | Queued |
| Nodes written per mutation (`maxBatch`) | 1000 | `LIMIT_EXCEEDED` |
| Relationships written per mutation | 10 × `maxBatch` | `LIMIT_EXCEEDED` |
| Relationship levels in a `where` (`maxFilterDepth`) | 2 | `BAD_USER_INPUT` |
| Items in an `in` list (`maxListFilter`) | 1000 | `BAD_USER_INPUT` |
| Characters in a string filter (`maxStringFilter`) | 10,000 | `BAD_USER_INPUT` |
| Items in a `@cypher` list argument (`maxListArgument`, `@size`) | 1000 | `BAD_USER_INPUT` |
| Document depth (`guards.maxDepth`) | 12 | `LIMIT_EXCEEDED` |
| Introspection depth (`guards.maxIntrospectionDepth`) | 20 | `LIMIT_EXCEEDED` |
| Aliases per document (`guards.maxAliases`) | 30 | `LIMIT_EXCEEDED` |
| Root fields per operation (`guards.maxRootFields`) | 20 | `LIMIT_EXCEEDED` |
| Tokens per document (`guards.maxTokens`) | 5000 | Syntax error |
| Subscriptions per scope (`maxSubscriptions`) | 100 | `LIMIT_EXCEEDED` |
| Relationship levels in a subscription `where` (`maxSubscriptionFilterDepth`) | 1 | `LIMIT_EXCEEDED` |
| Changes a subscriber may fall behind (`maxQueuedChanges`) | 1000 | Stream ends, `LIMIT_EXCEEDED` |
| Subscription check time (`subscriptionTimeoutMs`) | 2 s | Stream ends |
| Parsed documents cached | 500 | Oldest evicted |
| Compiled statements cached | 16 per field, 4096 total | Oldest evicted |
| Cache memory (`compileCacheBytes`) | 64 MiB each | Oldest evicted |

Not bounded today:

- **The write queue.** Mutations wait for the writer lock for up to
  `timeoutMs` each; nothing sheds load before that.
- **`totalCount` and aggregates.** They are charged against `maxCost` but
  have no cap of their own.
- **Work inside `@cypher` field statements.** See
  [their cost](/docs/graphql/cypher-fields#cost).

## Scaling

The numbers below come from the package's own load test
(`yarn bench:load`) on one shared 10-core machine. They show where the
ceilings are and roughly how the levers move them. They are not a
capacity plan: measure your schema, your data and your hardware.

### One writer

LoraDB commits one write transaction at a time. Readers use snapshots and
are never blocked by a writer: in a mixed test, keyed lookups kept a
median under 1 ms while 512 updates queued.

Writes are a different matter. A mutation holds the writer lock from its
first statement to its commit, across several round trips between
JavaScript and the engine. Measured ceilings were about 3,100 keyed
updates, 1,600 creates and 2,400 connects per second in memory, and about
35% lower with a write-ahead log. Past saturation, latency is queue depth
divided by throughput.

What this means for you:

- Keep `lora.begin()` transactions short and never hold one across a
  network call. Every other write waits.
- `@populatedBy` callbacks run inside the lock, one at a time. Keep them
  fast and free of I/O.
- A bulk delete looks up each node's neighbours with one statement per
  key, up to `maxBatch` statements in one lock hold. Prefer smaller
  `limit`s on write-heavy systems.
- Put a queue or rate limit in front of write-heavy endpoints. The library
  does not shed load yet.

### One JavaScript thread, and the worker pool

Reads run on Node's libuv worker pool, off the JavaScript thread. Two
ceilings follow:

- **The worker pool** has four threads by default, and read-heavy load
  saturates exactly four cores. Setting `UV_THREADPOOL_SIZE` to the number
  of cores (less what your HTTP layer needs) is free and was the largest
  single lever measured: going from 4 to 8 gave about 55% more paged reads
  and 78% more aggregates per second. Going past the core count made
  things worse.

  ```bash
  UV_THREADPOOL_SIZE=8 node server.js
  ```

  It must be set before the process starts.

- **The JavaScript thread** does GraphQL execution, compilation, result
  decoding and JSON for every request. The cheapest read, a lookup by key,
  topped out around 26,000 requests per second on one core however many
  were in flight. Large responses are worse: 10,000 nodes per response
  capped at about 120 requests per second and blocked the event loop for
  up to a second. Lower `maxCost` on public endpoints so one request
  cannot shape tens of thousands of nodes.

### More than one process

A database directory opens in one process. Node cluster mode, PM2
instances and container replicas cannot share a persistent database, and
in-memory databases in separate processes are simply different databases.

Within one process, `worker_threads` that open the same directory share
one engine and see each other's writes. In a preliminary test two workers
roughly doubled keyed reads over the main thread alone. Two caveats: this
has not been confirmed on a quiet machine, and `onWrite` is per
`LoraGraphQL` instance, so invalidating a cache across threads needs
`changeFeed: true` or your own broadcast.

### Memory and durability

These are properties of the engine that a GraphQL deployment inherits:

- The whole graph is held in RAM. There is no eviction.
- With a write-ahead log, group sync runs every second by default, so up
  to about a second of acknowledged writes can be lost on power loss or
  an operating system crash. A process crash alone loses nothing. Lower
  `groupSyncIntervalMs`, or call `db.sync()` after critical commits.
- A snapshot taken with `snapshotEveryCommits` is written inside the
  commit path, under the writer lock, so it shows up as a latency spike
  proportional to the size of the graph.

See [snapshots](/docs/snapshot) and [the write-ahead log](/docs/wal).

### Caches

- The parsed document cache holds 500 documents and evicts the oldest,
  not the least recently used. Clients that inline literal values instead
  of using variables produce a new document per request and defeat it, and
  each miss also misses the compile cache. Use variables, or persisted
  operations.
- A lookup by key compiles again for each distinct key, because the key is
  part of the cache key. The compile is cheap (around 0.1 ms) but it is
  not free.
- With `cursorSecret`, every edge's cursor is signed in JavaScript. A
  response with 20 parents of 100 edges signs about 2,000 cursors.

### Not load tested

The load test did not cover schemas with `@authorization` rules,
subscriptions over a network transport, cascading deletes or bulk
mutations. Subscription fan-out is indexed by type and key, but its
behaviour under load is unmeasured. If your workload leans on any of
these, test it before you depend on it.

## See also

- [Serving the schema](/docs/graphql/serving)
- [The smart layer: statistics and cost](/docs/graphql/smart-layer#s6-statistics-and-cost)
- [Engine limitations](/docs/limitations)
- [Performance](/docs/performance)
