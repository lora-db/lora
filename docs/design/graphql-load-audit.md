# lora-graphql under load: audit

Date: 2026-09-30. Measured on `main` after v0.18.0, with lora-node built from `f6251b0` ("never park a libuv worker on an interactive transaction"). The deadlock numbers in finding 1 are from the v0.18.0 binding, before that commit.

The question: how many concurrent requests a lora-graphql server takes, how well it keeps performing as load grows, and what becomes a bottleneck in production. Classification follows `known-risks.md`:

- **Observed**: measured with the load harness, or read directly in the code.
- **Inferred**: deduced from the code, not measured.

## How it was measured

`packages/lora-graphql/bench/load` (`yarn bench:load`, see its README).

- **Server:** a fresh server process per scenario, holding the bench graph (scale 1: 20k festivals, 2k users, 100k FOLLOWS), behind a minimal `node:http` endpoint with a parsed-document cache.
- **Load:** 4 client threads.
  - Closed loop: 1 to 512 requests in flight.
  - Open loop: fixed arrival rates, latency counted from the scheduled send time.
  - 1 s warmup and 5 s measured per step.
- **Per step:** exact latency percentiles, the server's CPU in cores, event-loop delay and RSS. Mixed scenarios also give per-operation percentiles.
- **Machine:** Apple M1 Max (10 cores, 32 GB), Node 24.11.
- **Defaults** unless stated: in-memory database, `UV_THREADPOOL_SIZE` 4, the driver as it was before the finding 2 fix ("stream").

Client and server share the machine: the numbers compare configurations; they are not a capacity plan.

## Headline numbers

Peak throughput, and latency at 32 in flight. "stream" is the driver before the finding 2 fix, which sent every read through `db.stream()`; "transaction" is the same server with reads sent through `db.transaction()` instead. Finding 2 has the numbers after the fix.

| scenario | stream: peak ok/s | stream: p99 at 32 | transaction: peak ok/s | transaction: p99 at 32 | cores used (stream / tx) |
|---|--:|--:|--:|--:|--:|
| `festival(key)` | 26,000 | 2.5 ms | 23,200 | 2.7 ms | 1.0 / 1.9 |
| `user → follows(10) → genre` | 6,100 | 10 ms | 14,000 | 3.6 ms | 1.0 / 4.0 |
| `festivals` filtered, sorted, limit 25 | 106 | 587 ms | 443 | 90 ms | 1.0 / 4.1 |
| connection first 20 + `totalCount` | 275 | 148 ms | 275 | 161 ms | 4.1 / 4.1 |
| `festivalsAggregate { count }` | 236 | 273 ms | 728 | 55 ms | 1.0 / 4.1 |
| 25 rows + two `@cypher` fields | 18 | 3,074 ms | 56 | 611 ms | 1.0 / 4.0 |
| 100 users × 100 follows (10k nodes) | 55 | 1,138 ms | 120 | 293 ms | 1.0 / 3.1 |
| `updateFestival` | 3,120 | 13 ms | same path | | 1.3 |
| `createFestivals` (1 node) | 1,630, falling | 52 ms | same path | | 1.8 |
| `updateUser` connect | 2,440 | 16 ms | same path | | 1.2 |
| mixed (40% key, 20% traverse, 10% each page, connection, `@cypher`, update) | 149 | 2,528 ms | 346 | 225 ms | 1.2 / 3.9 |

**Mixed traffic at a fixed arrival rate** (open loop). This is the number closest to "how many users can it serve":

| offered/s | stream: `festival(key)` p50 / p99 | transaction: `festival(key)` p50 / p99 | transaction: all ops p99 |
|--:|--:|--:|--:|
| 50 | 1.6 / 138 ms | 1.5 / 9 ms | 95 ms |
| 100 | 142 / 1,120 ms | 1.6 / 95 ms | 172 ms |
| 200 | 499 / 6,783 ms, 2.5% timeouts | 1.4 / 37 ms | 105 ms |
| 300 | 16% errors, ramp stopped | 94 / 249 ms | 272 ms |
| 400 | | 1,291 / 2,031 ms (saturated) | 2,032 ms |

On the stream driver, a mix with 30% scan-type queries saturated below 100 req/s. After that point, a cheap key lookup waited a median of 142 ms behind heavy queries. Routing reads through transactions lifted the knee to about 250 req/s.

## Findings

Ranked by production impact. Each finding lists what was seen, why it happens, and possible solutions.

### 1. Concurrent mutations deadlocked the process (fixed on main, unreleased)

- **Classification:** Observed, v0.18.0 binding.
- **What happens:**
  - With `UV_THREADPOOL_SIZE` 4, 4 concurrent `updateFestival` calls ran at 2,790/s.
  - With 5, throughput dropped to **0** and the server sat idle at 0.02 cores.
  - The stall was permanent, `process.exit()` included: the load harness had to SIGKILL the server.
  - The threshold is exactly the pool size plus one.
- **Why:** a read-write `begin()` waited for the writer lock on a libuv worker, and so did each statement. Once every worker waited, the lock holder's next statement could never run. `f6251b0` settles the promises from the transaction's own thread instead. Measured after the fix: 512 concurrent mutations complete, with no errors.
- **Solutions:**
  - Release `f6251b0` as a lora-node patch, and raise lora-graphql's `@loradb/lora-node` peer range to that version. Every v0.18.0 server that takes more than 4 concurrent mutations is one burst away from a hang.
  - Auto-commit and batched writes waiting for the lock still occupy a pool worker (the commit message says so). Meanwhile they starve reads on the transaction path, and fs, dns and zlib. Give the binding's lock wait the same treatment.
  - Pass `timeoutMs` and `signal` to `begin()` (`src/driver.ts:162`). Today the wait for the lock is unbounded and cannot be aborted, and an open transaction has no lifetime limit.

### 2. Reads run synchronously on the JS main thread (fixed)

- **Status:** fixed (`65599ba`, `0544a1a`). `loraDriver` routes each read to the cheapest path that keeps the JS thread free:
  - A single verified statement runs with `execute()` on a libuv worker. That costs 22 µs per call on a keyed lookup, against 50 µs for `transaction()`.
  - A `@key` lookup of stored properties only (`bounded`) streams its one row synchronously (7 µs).
  - Several statements share one read-only transaction.
  - `test/driver.test.ts` pins the routing.
- **After the fix, same harness:**

  | scenario | before | after |
  |---|--:|--:|
  | page | 106/s | ~440/s |
  | aggregate | 236/s | ~730/s |
  | traverse | 6,100/s | ~13,800/s |
  | `@cypher` fields | 18/s | ~54/s |
  | mixed | 149/s | ~390/s |

  - The event loop is no longer blocked by a read.
  - In open-loop mixed traffic at 200 req/s, key lookups went from p50 499 ms to 1.3 ms.
  - Interleaved on one binary, in process: key lookups run at 47.8k ops/s on v0.18.0, 44.9k all-transaction, and 49.4k with the final routing.
- **E13 check:** on 500k nodes, every generated read shape stops at `LIMIT` on the transaction path as early as it did on a stream (about 0.3 ms each).
- **Before the fix:** the driver sent every single-statement generated read through `db.stream().toArray()`. `streamNext` is a synchronous napi call, so the whole query, and the conversion of each cell, ran on the main thread.
  - A query that took 10 ms blocked every other request for 10 ms: the `page` query stayed at about 105 req/s from 1 to 512 in flight, the event loop was blocked up to 1.1 s at a time, and at 512 connections the server stopped accepting sockets (`ETIMEDOUT`).
  - `stream()` checks the deadline only between rows, so a blocking operator (sort, aggregation, DISTINCT) inside one `next()` could not be stopped by `timeoutMs`, and an `AbortSignal` listener could not fire while the thread was blocked.
- **Still open (binding):** `db.stream()` itself is still synchronous, for any caller that uses it on a large result. Make it asynchronous: open the cursor and pull rows in batches of N on a worker (an `AsyncTask` per batch), and decode on the main thread only once per batch.

### 3. Two throughput ceilings: one JS thread, and the libuv pool

- **Classification:** Observed.
- **The JS thread:**
  - The cheapest read, a keyed lookup, tops out at 26k req/s at exactly 1.0 cores, whatever the concurrency. Past about 8 in flight, extra concurrency only adds latency (p99 23 ms at 512).
  - graphql-js execution, compilation, result decoding, JSON and HTTP all share that thread.
  - Large responses make it worse: 10k nodes per response caps at 120 req/s, and blocks the loop for up to 1 s even on the transaction path.
- **The pool:**
  - On the transaction path, reads run on libuv workers, and every read-heavy scenario pinned the server at 4.0 cores, the default `UV_THREADPOOL_SIZE`.
  - At 128 in flight:

    | pool size | page req/s | aggregate req/s | notes |
    |--:|--:|--:|---|
    | 4 | 424 | 663 | |
    | 8 | 643 | 1,178 | about +55% and +78% |
    | 16 | 533 | 930 | oversubscribed: 10 cores, shared with the 4 client threads |
- **Horizontal scaling is constrained:** a database directory opens in one process only (`LORA_LOCKED`), so Node cluster mode, PM2 `-i` and replicas cannot share a persistent database, and in-memory replicas diverge.
- **Solutions:**
  - Document `UV_THREADPOOL_SIZE` = cores (minus what the HTTP layer needs) in the serving docs. It is free, and it is the single biggest lever once finding 2 is fixed.
  - **Worker threads sharing one engine:** verified that two `worker_threads` opening the same persistent directory share one engine and see each other's writes. The persistent-database registry is process-wide (`crates/bindings/lora-node/src/lib.rs:43`). This allows one process with N GraphQL threads, for example `reusePort` listeners on Linux, or a dispatcher.
    - Not benchmarked yet.
    - Caveat: `lora.onWrite` is per instance, so cache invalidation across threads needs the change feed or a broadcast.
    - In-memory databases are not shared.
  - Lower `maxCost` (default 50k rows) for public endpoints, so one request cannot shape tens of thousands of nodes on the event loop.

### 4. One writer at a time, holding the lock across JS round trips

- **Classification:** Observed.
- **Ceilings:** writes top out at 3,100 updates/s, 1,600 creates/s and 2,400 connects/s. Past saturation, latency is queue depth over throughput: 512 queued updates wait 175 ms.
- **Reads are not blocked:** in the byKey-plus-update mix, the key lookups kept a p50 under 1 ms at 512 in flight. Readers use snapshots.
- **With a WAL** (`--persist`): updates drop to 2,000/s (−35%) and creates to 1,050/s.
- **Why:**
  - One global writer mutex. A mutation opens a transaction, then runs its phases as separate awaited statements before committing: creates, disconnects, links, updates, nested deletes, cardinality, required and validation checks, then the payload read-back (`src/execute/mutate.ts:698`). A one-node create is 2.75 statements.
  - Each statement is a round trip between the JS thread and the transaction thread, all while holding the lock.
  - Also inside the lock:
    - `@populatedBy` callbacks, awaited one at a time (`mutate.ts:1239`);
    - a delete's neighbour lookup, one statement per key (`mutate.ts:1622`), up to `maxBatch` (1000) statements for a bulk delete.
- **Solutions:**
  - **Fewer round trips:** send a mutation's statements through `tx.executeMany` (one native call), and fold the validation and read-back into the last write. The lock hold then shrinks toward one round trip.
  - Resolve `@populatedBy` callbacks before `begin()`, in parallel.
  - Batch the delete neighbour lookup into one `UNWIND` statement, as the next statement already does.
  - **Bound the write queue in lora-graphql:** a JS-side semaphore with a maximum depth that fails fast (`OVERLOADED`, HTTP 503) instead of queuing without limit. Clients then back off before latency explodes.

### 5. A write to a shared indexed value costs O(nodes with that value)

- **Classification:** Observed, engine.
- **What happens:**
  - `createFestivals` got slower as the test ran: 1,020 → 404 creates/s over 30 s at a constant 1 in flight.
  - Isolated: a create whose indexed `capacity` value is shared by k nodes costs 0.15 ms at k = 4k and 0.55 ms at k = 24k, on both the staged and the auto-commit path. Distinct values stay flat at 0.10 ms.
  - The bench schema hits it because `capacity: Int @default(value: 100)` gives every created festival the same value.
- **Why:**
  - A posting list for one indexed value is an `IdSet` (`crates/lora-store/src/memory/id_set.rs`). Past 64 ids it becomes a plain `BTreeSet<u64>`.
  - The index maps are copy-on-write by partition (`CowMap` in `property_index.rs`, `CowOrdMap` in `sorted_property_index.rs`), but the bucket value inside a partition is cloned whole. A write therefore deep-copies the posting list of every indexed value it touches, twice for a RANGE index (hash plus sorted).
- **Production impact:** low-cardinality indexed fields are the normal case for `@filterable`:
  - enums (`status`),
  - booleans,
  - `@default` values,
  - country and genre keys.

  At a million nodes sharing `status: ACTIVE`, every write to one of them copies a million-id set. Because writes are serialized, that caps the write rate for the whole database.
- **Solutions:**
  - **Engine:** make large posting lists copy-on-write in chunks, as `ChunkedVec` does for labels, or keep a staged delta over a shared base. Keep it internal: no `imbl`, for the MPL license.
  - **Library:** `lora-graphql lint` or `analyze` could warn when an inferred index covers an enum, a boolean or a `@default` field. It could also let such filters run unindexed when the engine is not fixed.

### 6. Scans the cost limit does not charge for

- **Classification:** Observed.
- **What happens:** at scale ×5 (100k festivals), keyed lookups and traversals stayed flat (21.7k and 12.5k req/s), but three shapes lost about 6× throughput:
  - the filtered, sorted page: 443 → 76;
  - the connection with `totalCount`: 271 → 42;
  - the aggregate: 713 → 113.
- **Plans** (`db.explain`):
  - The page query is `NodeByPropertyRangeScan(capacity > $p0)` → `Sort` → `Limit 25`. The range filter matches up to 98% of the label, and all of it is sorted to return 25 rows. There is no top-k sort, and no index-ordered scan on `name` with early termination, although the planner's own estimate (6,667 rows) says the filter is not selective.
  - `totalCount` repeats the scan in a second statement, with `count()`. Two statements also mean a shared read-only transaction rather than one `execute()` (`src/compile/read.ts:361`).
  - `festivalsAggregate` is a full scan.
  - All three count as 0 or 1 against `maxCost`, which counts projected rows, not scanned rows (`read.ts:336-376`).
- **Solutions:**
  - **Engine:**
    - a top-k sort for `ORDER BY … LIMIT`;
    - choosing the index-ordered scan when the filter is non-selective;
    - counting a range from the index without visiting nodes.
  - **Library:**
    - charge `maxCost` from `explain()` estimates, or from `analyze()` statistics, for scans, `totalCount` and aggregates;
    - an opt-in `totalCount` cap;
    - `persistedOnly` in production, so only reviewed operations run.

### 7. `@cypher` fields run per row, with no cost

- **Classification:** Observed.
- **What happens:** 25 festivals, each with `followerCount` and `similar(limit: 3)`, take 66 ms of engine time: 16 req/s per core. `similar` matches the festival's roughly 2,000 genre siblings and sorts them, once per row. None of it is charged against `maxCost`.
- **Solutions:**
  - a `cost` argument on `@cypher`, or an estimate from `explain()` at startup;
  - documentation of the per-row semantics;
  - the engine's top-k sort (finding 6) helps `ORDER BY … LIMIT` inside the statement.

### 8. The compile cache is keyed on all claims and all variables

- **Classification:** Observed.
- **What happens:**
  - The cache key is the full coerced variables plus the full JWT claims, with 16 entries per field node (`src/lora-graphql.ts:1245`).
  - Real tokens differ per user and per issue (`sub`, `iat`, `jti`), so with per-user tokens every request recompiles.
  - Measured with a distinct token per request, on a schema with no auth rules at all: keyed lookups drop 22.6k → 19.4k req/s (−14%). Schemas with `@authorization` rules compile more per request, and so lose more.
  - Large variables, such as embedding vectors, are serialized into the key on every call.
- **Fixed:** the cache (`src/compile/cache.ts`) keys an entry on what the compile read: the variables the field references, each claim it looked up, the `$context` values it read, whether the request is authenticated, and the statistics version. A claim substituted into a rule's `node` filter (`"$jwt.sub"`) becomes a parameter slot, so users with different subjects share one compile. Capped at 16 entries per field node and 4,096 in total. A keyed lookup on a type with a `$jwt.sub` rule costs 13% less CPU per request.
- **Still open:** keyed lookups miss on every request, because the key variable is part of the cache key. Rebinding variables into parameter slots, the way claims are rebound, would let them hit.

### 9. Subscriptions and the change feed

- **Classification:** Inferred from the code; not load tested.
- **Fan-out:**
  - Every subscriber is its own feed listener, and rebuilds its events per change (`src/lora-graphql.ts:1397`, `:1714`).
  - `#visible` and `#resolveByKey` compile, and usually run, one read per subscriber per event.
  - Cost is therefore O(subscribers × changes). A 1,000-node bulk write with 10k subscribers is about 10M iterations plus the reads, on the event loop, and those reads run through the driver from finding 2.
- **Crash risk:** a feed error that is not `LORA_CHANGES_LAGGED` is rethrown inside a floating `void this.#pump(...)` (`src/execute/feed.ts:59`, `:79`). That is an unhandled rejection: by default it crashes the process, and otherwise the feed silently stops.
- **Engine side:**
  - Once a feed is open, every commit builds its change batch under the writer lock.
  - Retention is a ring of 1,024 batches, with full property maps.
- **Fixed (fan-out):** subscribers are indexed by type, then by key, behind one listener. Each change's events are built once per type it touches, subscribers to other types do no work, and the visibility and node-read compiles are cached per subscription (recompiled when statistics or the `$context` values they read change). With 2,000 subscribers, a 10-node write to the type 710 of them follow went from 179 to 87 ms (median, loaded machine).
- **Solutions still open:**
  - Batch `node` reads per change.
  - Catch and surface feed errors (reopen with backoff, and report through `onError`).
  - Add a subscription scenario to the harness before relying on subscriptions at scale.

### 10. Smaller costs on the request path

- **Classification:** Observed in code; not measured separately.
- **Cursor signing:** HMAC-SHA256 is pure JS (`src/compile/hmac.ts`). The key pads are rebuilt, and a new `TextEncoder` is created, on every `sign()` (`src/compile/cursor.ts:93`). This runs once per edge, and twice more for `pageInfo`, so 20 parents × 100 edges is about 2,000 HMACs.
  - Fix: use `node:crypto.createHmac` when it is available, and precompute the pads per secret.
- **Document cache:** it is FIFO, not LRU (`src/lora-graphql.ts:298`, 500 entries). Clients that inline literals instead of using variables thrash it, and each miss also starts the compile cache cold.
- **`onWrite` listeners:** they run synchronously on the mutating request's path.

### 11. Operational limits to plan around

- **Classification:** Observed in code (binding audit).
- **Memory:** the whole graph is held in RAM, with no eviction. Long-lived readers pin old snapshots while writes copy the chunks they touch.
- **Durability:** WAL group sync runs every 1,000 ms from Node, so up to about 1 s of acknowledged writes can be lost on an OS crash or power loss. A process crash alone is safe.
  - Fix: set `groupSyncIntervalMs` lower, or call `db.sync()` after critical commits.
- **Named databases:** they have no managed snapshots. The WAL grows, and a restart replays all of it; the container cap is 4 GiB.
- **Managed snapshots:** `openWalDatabase` with `snapshotEveryCommits` writes the snapshot inside the commit path, under the writer lock: an O(graph) latency spike every N commits.

## What already holds up

- **Bounded pages and cost:** every list is bounded (limit 25 by default, 100 at most). Connections fetch `first+1`. `maxCost` is summed across aliases.
- **Limits and guards:**
  - `timeoutMs` defaults to 10 s;
  - document guards cap depth, aliases, root fields and tokens;
  - introspection is off in production.
- **Bounded caches and queues:** documents 500, compiles 16 per field and 4,096 in total, change-feed ids 50k, subscriber queues 1,000.
- **Plan-cache friendly statements:** every value is a parameter, so statement text is stable.
- **Snapshot reads:** writers never block readers.
- **Early failure:** an over-limit request fails before it touches the database.

## Recommended order

1. Release the lora-node pool fix (`f6251b0`) and raise the peer range. Until then, any production v0.18.0 server can hang.
2. Done: reads run off the JS thread, except a bounded `@key` lookup, which streams its one row (finding 2). Still to do: document `UV_THREADPOOL_SIZE` (finding 3), and make `db.stream()` asynchronous in the binding.
3. Fix the posting-list copy in the engine (finding 5). Until then, add the lint warning.
4. Shorten the write lock hold with `executeMany`, and add a bounded write queue with load shedding (finding 4).
5. Charge scans in `maxCost`. Add top-k sort and ordered-index selection to the engine (findings 6 and 7).
6. Done: the compile-cache key (finding 8) and the feed pump's unhandled rejection (finding 9).
7. Prototype and benchmark worker threads sharing one engine, as the scaling story past one JS thread (finding 3).

## Gaps in this audit

- No schema with `@authorization` rules was loaded, so claim-dependent compile costs are a lower bound.
- Subscriptions, cascading deletes and multi-node bulk mutations were not load tested.
- Client and server shared one 10-core machine. Measure on the target hardware with the client elsewhere before sizing a deployment.
