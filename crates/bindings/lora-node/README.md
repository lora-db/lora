# lora-node

Node.js / TypeScript bindings for the [Lora](../../../README.md) graph
engine. The package exposes a first-class typed API: query results are
modelled as discriminated unions, temporal values carry `kind` tags, and the
`Database` class is strongly typed in both directions (params and rows).

**Non-blocking:** `execute()` dispatches each query to the libuv threadpool via
[`napi::Task`](https://napi.rs/docs/compat-mode/async-task). The JS event loop
stays free for the full duration of a query — a 2 000-node MATCH happily
interleaves with `setImmediate` ticks on the main thread (proven by a
dedicated vitest).

> **Package:** `@loradb/lora-node`.

## Install (local dev)

From the repository root:

```bash
corepack enable
yarn install --immutable
yarn workspace @loradb/lora-node build   # Rust cdylib + TypeScript declarations
yarn workspace @loradb/lora-node test    # vitest suite
```

The `build:native` step uses [`@napi-rs/cli`](https://napi.rs/) and
produces a platform-specific `lora-node.<platform>-<arch>.node` artifact next
to `package.json`.

## Usage

`lora-node` is **async-only** — the sole initialization pattern is
`createDatabase(...)`. There is no synchronous constructor and no
`Database.create()` static; `Database` is a type-only export.

```ts
import { createDatabase, isNode, type LoraNode } from "@loradb/lora-node";

const db = await createDatabase(); // in-memory by default
await db.execute("CREATE (:Person {name: $n, age: $a})", { n: "Alice", a: 30 });

const res = await db.execute<{ n: LoraNode }>("MATCH (n:Person) RETURN n");
for (const row of res.rows) {
  if (isNode(row.n)) {
    console.log(row.n.properties.name);
  }
}
```

### Timeouts and cancellation

Every `execute()`, `stream()` and `transaction()` call takes an optional
third argument. A query that runs out of time or is cancelled stops at its
next check point, rolls back any writes, releases its locks, and rejects:
with a `LoraError` coded `LORA_TIMEOUT` for `timeoutMs`, or with the
signal's reason (an `AbortError`) for `signal`.

```ts
await db.execute(query, params, { timeoutMs: 250 });

const controller = new AbortController();
const pending = db.execute(query, params, { signal: controller.signal });
controller.abort(); // rejects with AbortError

// Database-wide default; a call can override it, and `timeoutMs: 0` opts out.
const bounded = await createDatabase("app", { queryTimeoutMs: 1000 });
```

### Interactive transactions

`db.begin()` opens a transaction you can drive statement by statement,
with application logic in between. A `read_write` transaction holds the
writer lock until it commits or rolls back, so no other write interleaves
(other writers wait; keep it short). A failed statement rolls it back,
including one the engine never ran (an already-aborted `signal`, invalid
options): once `tx.isOpen` is `false` the writer lock is free. An
unfinished transaction rolls back when disposed. Calls still queued on a
transaction that failed, committed or rolled back reject with
`LORA_TRANSACTION`, as does a `begin()` still waiting for the writer lock
when its database is disposed. A statement's `timeoutMs` starts when it
runs, not while it waits behind earlier statements of the transaction.

```ts
await using tx = await db.begin("read_write");
const { rows } = await tx.execute("MATCH (t:Trip {key: $k}) RETURN t.free AS free", { k });
if ((rows[0].free as number) > 0) {
  await tx.execute("MATCH (t:Trip {key: $k}) SET t.free = t.free - 1", { k });
}
await tx.commit();
```

A `read_write` `begin()` waits for the writer lock for as long as another
writer holds it. Bound the wait with `timeoutMs`, or abandon it with a
`signal`:

```ts
// Rejects with LORA_TIMEOUT if the lock is not free within 2 seconds.
const tx = await db.begin("read_write", { timeoutMs: 2000, signal });
```

Past `timeoutMs` nothing is held, and the error says what it waited for
(`query deadline exceeded waiting for the writer lock`), as it does for an
auto-commit write that gives up behind an open transaction. An aborted
`signal` rejects `begin()`
with the signal's reason at once; if the transaction opens afterwards it
is rolled back. Both bound the wait, not the transaction: one that is
opened and never finished holds the lock until it is disposed.

A `Transaction` or row stream dropped without being finished is rolled
back once it is garbage-collected, and a dropped change feed is closed.
Open transactions and streams are also rolled back when the process exits.
Collection happens at an unpredictable time, so finish them explicitly
rather than relying on it.

`db.transaction(statements)` runs a batch in one transaction and commits:
the whole thing is one call. A read-write batch waits for the writer lock
on a thread of its own, never on a libuv worker, so queued write batches
leave reads running. The lock is released before
JavaScript hears of the result. Pair it with `assert(condition,
message)`, which fails the statement and so rolls the batch back, to
change data and check the outcome without a round trip:

```ts
await db.transaction([
  { query: "MATCH (a:Account {id: $id}) SET a.balance = a.balance - $n", params: { id, n } },
  { query: "MATCH (a:Account {id: $id}) RETURN assert(a.balance >= 0, 'insufficient funds')", params: { id } },
]);
```

The error of a failing statement ends with `(statement i of n)`.

`tx.executeMany()` runs several statements in one native call and returns
their results in order, saving the round trip each `execute()` costs
(about 0.1 ms per statement down to 0.03 ms). Each statement sees the
writes before it. The first failing statement stops the batch and rolls
the transaction back; its error message ends with `(statement i of n)`.
`timeoutMs` and `signal` bound the whole batch.

```ts
const [created, total] = await tx.executeMany([
  { query: "CREATE (:Item {k: $k})", params: { k } },
  { query: "MATCH (i:Item) RETURN count(i) AS c" },
]);
```

Schema commands (`CREATE CONSTRAINT`, `CREATE INDEX`, `DROP ...`) work
inside `transaction()` and `db.begin()` and commit or roll back together
with the data statements.

#### Open transaction limit

Each open `begin()` transaction and each mutating `stream()` that opens
on its own thread (see Known limitations) runs on a dedicated OS thread.
A database runs at most `maxOpenTransactions` of them at once, 1024 by
default. Past the limit, `begin()` rejects and `stream()` throws at once
with `LORA_TOO_MANY_OPEN`. They do not wait in a queue: a queued
transaction could wait for one that JavaScript has not finished yet.
A slot frees up as soon as its transaction commits or rolls back (a
failed statement or a failed `begin()` included) or its stream ends or is
closed. A dropped handle frees its slot once it is garbage-collected and
rolled back; one dropped while still waiting for the writer lock frees it
when that wait ends.

```ts
const db = await createDatabase("app", { maxOpenTransactions: 256 });
try {
  await using tx = await db.begin();
  // ...
} catch (err) {
  if (err instanceof LoraError && err.code === "LORA_TOO_MANY_OPEN") {
    // Too many transactions in flight: shed load or retry later.
  }
}
```

Read-write `db.transaction()` batches also run on threads of their own,
but nothing in JavaScript holds one open, so they queue instead: at most
`maxOpenTransactions` batches run at once (a budget separate from the one
above), and the rest wait their turn, first in, first out. Time spent
waiting counts against the batch's `timeoutMs` (or `queryTimeoutMs`),
which rejects with `LORA_TIMEOUT`, and its `signal` abandons the wait. A
waiting batch does not keep the process alive.

`openWalDatabase()` takes the same option. The limits are per database
handle.

### Change feed

`db.changes()` yields one `LoraChangeBatch` per committed write, in commit
order. Every write path counts: `execute()`, `stream()`, `transaction()`,
`db.begin()` / `executeMany()`, imports, `clear()` and `loadSnapshot()`.
Rolled-back work never appears.

```ts
const feed = db.changes({ fromLsn: savedLsn }); // omit fromLsn to start now
await feed.ready; // optional: every commit after this point is delivered
for await (const batch of feed) {
  for (const change of batch.changes) {
    if (change.kind === "nodeUpdated" && change.labels.includes("User")) {
      invalidate(change.properties.id, change.setKeys);
    }
  }
  savedLsn = batch.lsn;
}
```

A batch is `{ lsn, changes }`. `lsn` is a strictly increasing resume
token. Each change reports the net effect of the write on one entity, in
the order the write first touched it:

| `kind`                                       | Fields                                                                                 |
| -------------------------------------------- | -------------------------------------------------------------------------------------- |
| `nodeCreated`, `nodeDeleted`                 | `id`, `labels`, `properties`                                                           |
| `nodeUpdated`                                | `id`, `labels`, `properties`, `setKeys`, `removedKeys`, `addedLabels`, `removedLabels` |
| `relationshipCreated`, `relationshipDeleted` | `id`, `type`, `startId`, `endId`, `properties`                                         |
| `relationshipUpdated`                        | the relationship fields plus `setKeys`, `removedKeys`                                  |
| `reset`                                      | none: the whole graph was replaced (`clear()`, `loadSnapshot()`)                       |

Created and updated entities carry their state after the commit. Deleted
entities carry their last committed state, so a consumer can still read
their keys. An entity created and deleted in the same write is left out.
When a write touches a key or label more than once, the last operation
decides whether it is listed as set or removed.

Resuming:

- In-memory databases number commits with a process-local counter and keep
  the last 1024 batches. `fromLsn` works within that window.
- WAL-backed databases (`databaseName`, `openWalDatabase`) use the WAL's
  commit LSN. `fromLsn` resumes across restarts for as far back as the WAL
  holds history: the feed rebuilds older batches by replaying the WAL from
  an empty graph, a managed checkpoint snapshot or the snapshot the last
  `db.sync()` wrote, so an old resume point costs a replay. For a named
  database `db.sync()` is a checkpoint: it drops the WAL its snapshot
  covers, so history starts at the last `sync()`.
- An LSN the database no longer retains fails with
  `LORA_CHANGES_TRUNCATED`: start a new feed and re-read current state.

Writers never wait for a feed. Each feed buffers up to `bufferSize` batches
(default 1024); a consumer that falls further behind receives the buffered
batches and then `LORA_CHANGES_LAGGED`, and should resume with
`fromLsn: feed.lastLsn`. `break`, `feed.close()`, an aborted `signal`
(`next()` rejects with its reason) and `db.dispose()` end the feed. An open
feed does not keep the process alive on its own.

Capture starts with the first `changes()` call on a database and stays on
until it closes. It costs about 0.5 µs per single-row write in the engine
(and about 35% on bulk `UNWIND` writes) plus the consumer's own work.
Snapshot restores on a WAL-backed database appear in the live feed as
`reset` but are not part of the WAL history a later resume replays.

### Explain & Profile

`db.explain()` and `db.profile()` are first-class methods alongside
`db.execute()`. They are intentionally *separate calls*, not a flag on
`execute()`, so you have to opt in explicitly to plan inspection or
metrics collection.

```ts
const plan = await db.explain(
  "MATCH (p:Person) WHERE p.name = $name RETURN p",
  { name: "Alice" },
);
console.log(plan.shape);          // "readOnly"
console.log(plan.tree.operator);  // top-most operator label

const profile = await db.profile(
  "MATCH (p:Person) WHERE p.name = $name RETURN p",
  { name: "Alice" },
);
console.log(profile.metrics.totalElapsedNs);
console.log(profile.metrics.perOperator);
```

`explain()` never invokes the executor — calling it on a mutating
query (`CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE`) leaves the graph
untouched.

> **`profile()` executes the query for real.** Mutating queries
> produce the same side effects as `execute()`: the WAL is written,
> snapshots observe the commit, and the live store advances. Use
> `explain()` to inspect a mutating plan without running it.

The initialization rule is:

```ts
import { createDatabase } from "@loradb/lora-node";

const inMemory = await createDatabase();           // in-memory only
const defaultPersistent = await createDatabase("app"); // ./app.loradb
const nestedPersistent = await createDatabase("app", {
  databaseDir: "./data",
  syncMode: "groupSync",                          // default
});                                                // ./data/app.loradb
```

Passing a database name enables persistence. Use `databaseDir` when you want a
directory other than the current working directory.
The default `syncMode: "groupSync"` writes WAL bytes before `execute()` resolves
and batches fsyncs for write-heavy workloads. Call `await db.sync()` when you
need an immediate durability boundary before copying the portable `.loradb`
archive while the database is still open. `sync()` also checkpoints: it
writes the graph as the archive's base snapshot and drops the WAL that
snapshot covers, so the archive holds the graph and the writes since the last
`sync()`, not every write ever made.

Node also has a container-backed convenience overload:

```ts
import { createDatabase } from "@loradb/lora-node";

const db = await createDatabase("app", { databaseDir: "./data" });
```

The database name is validated and resolved under `databaseDir`, or under the
current working directory when no directory is supplied, appending `.loradb` to
the basename when needed. Relative paths resolve from the current working
directory. This is a Node-only initialization convenience; the query surface,
shared types, and async method signatures still match `lora-wasm`.

Persistent opens for the same resolved archive path in one Node process share a
single live native engine. Call `db.dispose()` when you need to release a handle
eagerly; cross-process opens of the same archive are blocked to prevent
split-brain writers.

For explicit WAL directories with managed snapshots, use `openWalDatabase`:

```ts
import { openWalDatabase } from "@loradb/lora-node";

const db = await openWalDatabase({
  walDir: "./data/wal",
  snapshotDir: "./data/snapshots",
  snapshotEveryCommits: 1000,
  snapshotKeepOld: 2,
});
```

`snapshotOptions` accepts the same compression/encryption options as
`saveSnapshot`.

### Online backups

Copying a named database's directory while it runs does not give a
database, and another process cannot open it. Take the copy from the
process that owns it:

```ts
const snapshot = await db.snapshot("./backups/2026-10-26");
// { bytes: 69283, lsn: 3, nodeCount: 100000,
//   path: './backups/2026-10-26/app.loradb', relationshipCount: 0 }
```

It writes `<targetDir>/<name>.loradb` (the target must be missing or empty)
as of one committed transaction, every write up to `lsn`, while reads and
writes keep running, off the JS thread and in bounded memory. The file
appears only once it is complete, so a crash mid-copy leaves no partial
database. Open it with `createDatabase("app", { databaseDir: targetDir })`.
`await db.checkpoint()` checkpoints on demand and resolves with the
snapshot's metadata.

For a directory no process has open, the package's `lora` command does the
same, and refuses a directory another process owns:

```bash
npx lora snapshot ./data ./backups/offline
```

## Snapshots

`saveSnapshot(path)` writes the current graph to a local file. Plain strings
are always treated as paths. Calling `saveSnapshot()` returns a Node `Buffer`;
object formats such as `{ format: "base64" }`, `{ format: "arrayBuffer" }`,
`{ format: "uint8Array" }`, and `{ format: "stream" }` return in-memory
snapshot data in that shape. `{ format: "path", path }` accepts either a path
string or `file:` URL.

`loadSnapshot` accepts a `NodeSnapshotSource`: a filesystem path, `file:` URL,
HTTP(S) or `data:` URL, `Buffer`, `Uint8Array`, `ArrayBuffer`, Node
`Readable`, web `ReadableStream`, or async iterable of byte chunks.

```ts
import { readFile } from "node:fs/promises";
import { createReadStream } from "node:fs";
import { pathToFileURL } from "node:url";
import { createDatabase } from "@loradb/lora-node";

const db = await createDatabase();
await db.execute("CREATE (:Person {name: 'Alice'})");
await db.saveSnapshot("./graph.lorasnap");
const bytes = await db.saveSnapshot();
const base64 = await db.saveSnapshot({ format: "base64" });
const stream = await db.saveSnapshot({ format: "stream" });

await db.loadSnapshot("./graph.lorasnap");
await db.loadSnapshot(pathToFileURL("./graph.lorasnap"));
await db.loadSnapshot(await readFile("./graph.lorasnap"));
await db.loadSnapshot(createReadStream("./graph.lorasnap"));
await db.loadSnapshot(bytes);
await db.loadSnapshot(stream);
await db.loadSnapshot(new URL("https://example.com/graph.lorasnap"));
```

Loading replaces the graph under the writer lock and runs off the event
loop. Like any write, it waits for an open transaction or write stream to
finish first.

## Typed value model

| TS type                 | Runtime shape                                                                 |
|-------------------------|-------------------------------------------------------------------------------|
| `null`/`boolean`/`number`/`string` | pass-through JS primitives                                                     |
| `bigint`              | integers outside `Number.MIN_SAFE_INTEGER..MAX_SAFE_INTEGER` (exact 64-bit)     |
| `LoraValue[]` / object | homogeneous arrays and nested records                                          |
| `LoraNode`            | `{ kind: "node", id, labels, properties }`                                      |
| `LoraRelationship`    | `{ kind: "relationship", id, startId, endId, type, properties }`                |
| `LoraPath`            | `{ kind: "path", nodes: number[], rels: number[] }`                             |
| `LoraDate`…`LoraDuration` | `{ kind: "date", iso: "YYYY-MM-DD" }` etc.                              |
| `LoraPoint`           | Discriminated union on `srid`, see below                                       |
| `LoraBinary`          | `{ kind: "binary", length, segments: number[][] }`; concatenate the segments    |

`LoraPoint` is a discriminated union over the four supported CRSes:

| Shape                                                                                                        | Meaning              |
|--------------------------------------------------------------------------------------------------------------|----------------------|
| `{ kind: "point", srid: 7203, crs: "cartesian", x, y }`                                                      | Cartesian 2D         |
| `{ kind: "point", srid: 9157, crs: "cartesian-3D", x, y, z }`                                                | Cartesian 3D         |
| `{ kind: "point", srid: 4326, crs: "WGS-84-2D", x, y, longitude, latitude }`                                 | WGS-84 2D            |
| `{ kind: "point", srid: 4979, crs: "WGS-84-3D", x, y, z, longitude, latitude, height }`                      | WGS-84 3D            |

Point parameters accept the same shape reads return: `{ kind: "point",
latitude, longitude[, height] }` (WGS-84 unless `srid` says otherwise) or
`{ kind: "point", srid, x, y[, z] }`.

Integer parameters outside the safe range must be passed as `bigint`; an
integer-valued `number` past 2^53 is rejected with `LORA_INVALID_PARAMS`
because JavaScript has already rounded it.

Bytes go in as a `Buffer`, a `Uint8Array` or an `ArrayBuffer` and are stored
as a binary value; they come back as a `LoraBinary`:

```ts
const body = await readFile("./record.json.gz");
await db.execute("CREATE (:Record {key: $key, body: $body})", { key: "r1", body });
const { rows } = await db.execute("MATCH (r:Record {key: 'r1'}) RETURN r.body AS body");
const bytes = Buffer.from((rows[0].body as LoraBinary).segments.flat());
```

Other typed arrays (`Float32Array`, `DataView`, …) are refused with
`LORA_INVALID_PARAMS`; pass numbers as `vector(...)`.

Helper constructors (`date("2025-01-15")`, `cartesian(1, 2)`, `cartesian3d(1, 2, 3)`,
`wgs84(lon, lat)`, `wgs84_3d(lon, lat, height)`, `duration("P1M")`, …) and
narrowing guards (`isNode`, `isRelationship`, `isPath`, `isPoint`, `isTemporal`)
are exported from `lora-node`.

> `distance()` on WGS-84-3D points ignores `height` — see
> [functions reference](../../../apps/loradb.com/docs/functions/overview.md) for the full spatial
> reference and known limitations.

## Architecture

```
lora-database (Rust)
   └── lora-node (crate, cdylib)        <- napi-rs bindings, AsyncTask
          └── ts/index.ts                 <- strongly-typed async wrapper
                 └── ../shared-ts/types.ts     <- shared TS contract (with lora-wasm)
```

Query execution path:

```
JS main thread         libuv threadpool             Rust
──────────────         ───────────────────          ────────────────
db.execute(…)   ──►   ExecuteTask::compute()   ──►  parser → analyzer →
                                                    compiler → executor →
                                                    storage
             ◄──   resolve() wraps serde_json::Value
                   into JsUnknown and resolves the Promise
```

Interactive transactions (`db.begin()`) take another path: each open
transaction lives on its own thread, which takes and holds the writer lock,
and every call on it (`execute`, `executeMany`, `commit`, `rollback`) is a
message to that thread, whose reply settles the promise on the JS thread. No
libuv worker ever waits for a transaction, so any number of concurrent
`begin()` calls — waiting their turn for the writer lock — leaves the pool free
for other queries, and the transaction holding the lock always gets to run its
next statement.

The Rust crate is added to the workspace root (`Cargo.toml`). The Node side is
self-contained inside this directory. Only sub-millisecond operations
(`clear`, `nodeCount`, `relationshipCount`) stay synchronous inside napi; the
TS wrapper still exposes them as `Promise`-returning methods to keep the API
identical to `lora-wasm`.

## Errors

`db.execute(...)` throws `LoraError` with a narrowed `code` from the
`LoraErrorCode` union — these mirror `lora_database::LoraErrorCode` 1:1.

Common ones:

- `LORA_PARSE` — Cypher syntax could not be parsed
- `LORA_SEMANTIC` — analysis failure (unknown variable, label, type mismatch, …)
- `LORA_INVALID_PARAMS` — a parameter value could not be coerced to a `LoraValue`
- `LORA_READ_ONLY` — mutating statement issued in a read-only context
- `LORA_NOT_FOUND` — a named entity does not exist
- `LORA_CONSTRAINT`, `LORA_UNIQUE_CONSTRAINT`, `LORA_NOT_NULL_CONSTRAINT`,
  `LORA_FOREIGN_KEY`, `LORA_TRANSACTION` — graph constraint or transaction failure
- `LORA_INVALID_VECTOR` — vector value failed dimension / coordinate-type validation
- `LORA_TIMEOUT` — query exceeded its cooperative deadline
- `LORA_DATABASE_NAME` — logical database name violates the portable-path rules
- `LORA_CONFIG`, `LORA_VALIDATION` — configuration or validation failure
- `LORA_IO`, `LORA_CONNECTION`, `LORA_WAL_CORRUPTION`, `LORA_WAL_POISONED` — storage failures
- `LORA_SNAPSHOT_CODEC`, `LORA_SNAPSHOT_CRYPTO` — snapshot codec / crypto failures
- `LORA_LOCKED` — the database directory is locked by another process
- `LORA_CHANGES_TRUNCATED`, `LORA_CHANGES_LAGGED`: a change feed cannot
  resume from its `fromLsn`, or fell behind its buffer (see Change feed)
- `LORA_INTERNAL` — last-resort fallback when the engine cannot classify the failure
- `LORA_TOO_MANY_OPEN`: `maxOpenTransactions` transactions and write streams
  are already open on this database (see Open transaction limit)
- `UNKNOWN` — catch-all for messages without a recognized code

See `ts/types.ts` (`LoraErrorCode`) for the full list.

## Known limitations

- **Concurrent writes.** Each `execute()` hops through the threadpool; read
  queries can share the store read lock, while writes serialize on the store
  write lock. Firing many concurrent write queries against the same
  `Database` (e.g. 2 000 parallel `CREATE`s via `Promise.all`) works but
  queues behind that write lock — and an auto-commit or batched write waiting
  for it occupies a pool thread meanwhile, delaying other queries (never
  deadlocking: the writer holding the lock always runs). Prefer
  `await`-in-a-loop or a single batched query for heavy write workloads.
  While an interactive transaction or a mutating stream is open (it holds
  the lock until JavaScript finishes it), auto-commit writes, `clear()`,
  `sync()` and `loadSnapshot()` wait on lora's own threads instead, so the
  pool stays free for `fs`, `dns`, `zlib` and `crypto`.
  Interactive transactions wait on their own threads, not the pool, and so
  do mutating `stream()`s while the lock may be busy: one opens without
  blocking the event loop, and its first `next()` waits for the writer
  lock. A mutating stream opened while nothing else writes (no transaction,
  other mutating stream or pool query in flight) finds the lock free and
  is pulled synchronously, as cheaply as a read.
- **Stream pulls run on the event loop.** A read-only `stream()` (and a
  mutating one opened while nothing else writes) is pulled synchronously on
  the JS thread. Its `timeoutMs` is enforced inside each pull, so a pull
  that does a lot of work (a large aggregation) stops at the deadline, but
  an `AbortSignal` cannot fire while a pull runs. Give expensive streams a
  `timeoutMs`, or use `execute()`, which runs on the threadpool.
- **WAL surface.** Node persistence exposes container-backed initialization,
  `syncMode: "groupSync"`, `db.sync()`, `db.checkpoint()` and
  `db.snapshot()`. WAL truncate and status controls are not exposed yet.
- **Archive ownership.** One archive can only be open by one writer process at a
  time. Multiple Node handles in the same process share the same live engine;
  a second process is rejected with `LORA_LOCKED` while the first holds the
  archive lock, which `dispose()` releases.
