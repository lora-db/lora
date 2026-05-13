---
title: Using LoraDB in Node.js and TypeScript
sidebar_label: Node.js
description: Install and use LoraDB in Node.js or TypeScript via the lora-node N-API binding — async queries on libuv, explain/profile diagnostics, helpers, snapshots, WAL persistence, and shared result shapes.
---

# Using LoraDB in Node.js and TypeScript

## Overview

`lora-node` is a native N-API binding. Queries run on the libuv
threadpool so they don't block the event loop. Auto-commit reads can
overlap on engine snapshots; write commits still serialize. The
surface, helpers, and type guards match the
[WASM binding](./wasm) for query execution and result handling — the
same query code largely ports with an import swap. Node also adds
filesystem-backed persistence the WASM build cannot: container-backed
`.loradb` files, explicit WAL directories, and path-based snapshot
save/load.

## Installation / Setup

[![npm (@loradb/lora-node)](https://img.shields.io/npm/v/@loradb/lora-node?label=%40loradb%2Flora-node&logo=npm)](https://www.npmjs.com/package/@loradb/lora-node)

### Requirements

- Node.js **18+**
- For building from source: Rust toolchain (`rustup`) +
  `@napi-rs/cli`

### Install

Install from npm:

```bash
npm install @loradb/lora-node
```

When working inside this repository, build from source:

```bash
cd crates/bindings/lora-node
npm install
npm run build        # builds native .node artifact + TypeScript
```

## Creating a Client / Connection

`lora-node` is **async-only**. The common initialization path is
`createDatabase(...)`, which returns a `Promise<Database>`:

```ts
import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase(); // in-memory by default
```

There is no synchronous constructor and no `Database.create()` static.
Use `createDatabase(...)` for in-memory or container-backed `.loradb`
databases, and `openWalDatabase(...)` when you want an explicit WAL
directory.

Rule of thumb:

```ts
import { createDatabase, openWalDatabase } from '@loradb/lora-node';

const inMemory = await createDatabase();           // in-memory database
const persistent = await createDatabase('app', { databaseDir: './data' }); // ./data/app.loradb
const wal = await openWalDatabase({
  walDir: './data/app.wal',
  snapshotDir: './data/app.snapshots',
  snapshotEveryCommits: 1000,
});
```

If you want persistence, pass a database name and `databaseDir` to
`createDatabase(...)`, or pass `walDir` to `openWalDatabase(...)`.

To open an container-backed embedded database instead of a fresh in-memory
one, pass a database name and `databaseDir`:

```ts
import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase('app', { databaseDir: './data' }); // ./data/app.loradb
```

The name is validated and resolved to `<databaseDir>/<name>.loradb`.
Relative paths resolve from the current working directory. On boot,
committed WAL records inside that archive are replayed automatically before
the handle is returned.

:::caution Do not skip the `await`
`createDatabase()` returns a `Promise`. Calling `execute()` on the
unresolved promise will throw. Always `await` the factory before
running queries, and never instantiate the `Database` type
directly — it is exported as a **type only**.
:::

## Running Your First Query

```ts
import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase();

await db.execute("CREATE (:Person {name: 'Ada', born: 1815})");

const result = await db.execute(
  "MATCH (p:Person) RETURN p.name AS name, p.born AS born"
);

console.log(result.rows);
// [ { name: 'Ada', born: 1815 } ]
```

## Examples

### Minimal working example

Already shown above — `await createDatabase()` → `execute` →
inspect `result.rows`.

### Parameterised query

```ts
const result = await db.execute(
  "MATCH (p:Person) WHERE p.name = $name RETURN p.name AS name",
  { name: 'Ada' }
);
```

Values map automatically: JS numbers → `Int` or `Float`, strings →
`String`, booleans → `Bool`, `null` → `Null`, arrays → `List`, plain
objects → `Map`. Dates and spatial points use helper factories — see
[Typed helpers](#typed-helpers).

### Explain and profile

`explain` and `profile` are first-class binding methods, not Cypher
keywords that you prepend to the query string. Use `db.explain(...)`
when you want the compiled plan without running the executor:

```ts
const plan = await db.explain(
  "MATCH (p:Person) WHERE p.name = $name RETURN p",
  { name: 'Ada' }
);

console.log(plan.shape);          // "readOnly" or "mutating"
console.log(plan.resultColumns);  // ["p"]
console.log(plan.tree.operator);  // top-level physical operator
```

The plan tree contains stable operator `id`s, an `operator` label,
opaque human-readable `details`, `estimatedRows` (currently `null`),
and child operators. `explain` never invokes the executor, so even
`CREATE`, `MERGE`, `SET`, `DELETE`, and `REMOVE` plans leave the graph
untouched.

Use `db.profile(...)` when you want the same plan plus runtime metrics:

```ts
const profile = await db.profile(
  "MATCH (p:Person) WHERE p.name = $name RETURN p",
  { name: 'Ada' }
);

console.log(profile.metrics.totalElapsedNs);
console.log(profile.metrics.totalRows);
console.log(profile.metrics.mutated);
console.log(profile.metrics.perOperator);
```

:::caution `profile` executes the query
Mutating queries passed to `profile` produce the same side effects as
`execute`: WAL-backed databases write the commit, snapshots observe the
new state, and the live graph advances. Use `explain` to inspect a
mutating plan without running it.
:::

Both methods accept the same parameter values as `execute`, including
tagged helper structs for temporal, spatial, vector, and binary values.
Graph structs such as `LoraNode` are returned by queries; for input,
pass property values or typed helper values:

```ts
import { date, wgs84 } from '@loradb/lora-node';

const params = {
  since: date('1800-01-01'),
  near: wgs84(4.89, 52.37),
  radius: 5000,
};

const plan = await db.explain(
  `MATCH (c:City)
   WHERE c.founded >= $since
     AND geo.distance(c.location, $near) < $radius
   RETURN c.name AS name`,
  params
);

const profile = await db.profile(
  `MATCH (c:City)
   WHERE c.founded >= $since
     AND geo.distance(c.location, $near) < $radius
   RETURN c.name AS name`,
  params
);
```

### Structured result handling

```ts
import type { LoraNode } from '@loradb/lora-node';
import { isNode } from '@loradb/lora-node';

const res = await db.execute<{ n: LoraNode }>(
  "MATCH (n:Person) RETURN n"
);

for (const row of res.rows) {
  if (isNode(row.n)) {
    console.log(row.n.id, row.n.labels, row.n.properties);
  }
}
```

### Express route handler

```ts
import express from 'express';
import { createDatabase, LoraError } from '@loradb/lora-node';

const db = await createDatabase();
const app = express();
app.use(express.json());

app.get('/users/:id', async (req, res) => {
  try {
    const { rows } = await db.execute(
      "MATCH (u:User {id: $id}) RETURN u {.id, .handle, .tier} AS user",
      { id: Number(req.params.id) }
    );
    if (rows.length === 0) return res.status(404).end();
    res.json(rows[0].user);
  } catch (err) {
    if (err instanceof LoraError) {
      return res.status(400).json({ error: err.message, code: err.code });
    }
    console.error(err);
    res.status(500).end();
  }
});

app.listen(3000);
```

The same shape generalises to Fastify, Hono, and the edge/serverless
handlers — the `Database` instance lives at module scope.

### Handle errors

```ts
import { LoraError } from '@loradb/lora-node';

try {
  await db.execute("BAD QUERY");
} catch (err) {
  if (err instanceof LoraError) {
    console.error(err.code);   // "LORA_ERROR" | "INVALID_PARAMS"
    console.error(err.message);
  } else {
    throw err;                 // unexpected — rethrow
  }
}
```

### Concurrency

```ts
// Five lookups in parallel — read-only queries can overlap on snapshots
const handles = ['alice', 'bob', 'carol', 'dan', 'eve'];
const results = await Promise.all(
  handles.map(h =>
    db.execute("MATCH (u:User {handle: $h}) RETURN u.id", { h })
  )
);
```

The event loop stays responsive. Read-only calls can overlap on engine
snapshots; write commits still serialize.

### Persisting your graph

LoraDB has three Node persistence shapes:

- `createDatabase()` for a purely in-memory graph.
- `createDatabase('app', { databaseDir: './data' })` for container-backed
  recovery between process restarts.
- `openWalDatabase({ walDir: './data/wal', snapshotDir: './data/snapshots' })`
  for an explicit WAL directory with optional managed snapshots.
- `saveSnapshot` / `loadSnapshot` for point-in-time files that you can
  move, back up, or load into a fresh handle.

```ts
import {
  createDatabase,
  openWalDatabase,
  type SnapshotMeta,
} from '@loradb/lora-node';

const db = await createDatabase();
await db.execute("CREATE (:Person {name: 'Ada'})");

// Save everything to disk.
const meta: SnapshotMeta = await db.saveSnapshot('graph.bin');
console.log(meta.nodeCount, meta.relationshipCount);

// Restore into a fresh handle (in a new process, for example).
const db2 = await createDatabase();
await db2.loadSnapshot('graph.bin');

const durable = await openWalDatabase({
  walDir: './data/wal',
  snapshotDir: './data/snapshots',
  snapshotEveryCommits: 1000,
  snapshotOptions: { compression: { format: 'gzip', level: 1 } },
  syncMode: 'groupSync',
});
```

`saveSnapshot` / `loadSnapshot` are `async` like every other
`@loradb/lora-node` call, but the underlying engine still encodes or decodes the
whole graph. When you are using
plain `createDatabase()` with no archive or WAL path, a crash loses
all in-memory state. Manual snapshots protect only the mutations saved
before the crash; WAL-backed opens replay committed writes on restart.

`saveSnapshot()` with no path returns a `Buffer`. `saveSnapshot(path)`
writes atomically to disk and returns `SnapshotMeta`. `loadSnapshot`
accepts a path string / `URL`, `Buffer`, `Uint8Array`, `ArrayBuffer`,
Node `Readable`, Web `ReadableStream`, or async iterable.

See the canonical [Snapshots guide](../snapshot) for the full
metadata shape, atomic-rename guarantees, and boundaries.

## Common Patterns

### Bulk insert from a JS array

```ts
const rows = [
  { id: 1, name: 'Ada' },
  { id: 2, name: 'Grace' },
  { id: 3, name: 'Alan' },
];

await db.execute(
  `UNWIND $rows AS row
   CREATE (:User {id: row.id, name: row.name})`,
  { rows }
);
```

See [`UNWIND`](../queries/unwind-merge#bulk-load-from-parameter).

### Typed helpers

Build typed temporal / spatial values in JS and pass them as
parameters:

```ts
import { createDatabase, date, duration, wgs84 } from '@loradb/lora-node';

const db = await createDatabase();

await db.execute(
  "CREATE (:Trip {when: $when, span: $span, origin: $origin})",
  {
    when:   date('2026-05-01'),
    span:   duration('PT90M'),
    origin: wgs84(4.89, 52.37),
  }
);
```

Available factories: `date`, `time`, `localtime`, `datetime`,
`localdatetime`, `duration`, `cartesian`, `cartesian3d`, `wgs84`,
`wgs84_3d`.

### Type guards

| Function | Narrows to |
|---|---|
| `isNode(v)` | `LoraNode` |
| `isRelationship(v)` | `LoraRelationship` |
| `isPath(v)` | `LoraPath` |
| `isPoint(v)` | `LoraPoint` |
| `isTemporal(v)` | any temporal variant |

### Other methods

```ts
await db.clear();                    // drop all nodes + relationships
await db.nodeCount();                // number of nodes
await db.relationshipCount();        // number of relationships
db.dispose();                        // release the native handle
```

`clear` / `nodeCount` / `relationshipCount` return Promises for API
symmetry but run synchronously inside the native layer. `dispose()` is
synchronous and idempotent; call it when you need to reopen the same
archive or WAL directory inside the same process.

### Repository pattern

`Database` is exported as a type-only symbol — use it to annotate
the instance that `createDatabase()` returned:

```ts
import { createDatabase, type Database } from '@loradb/lora-node';

export class UserRepo {
  constructor(private readonly db: Database) {}

  async upsert(id: number, handle: string) {
    await this.db.execute(
      `MERGE (u:User {id: $id})
         ON CREATE SET u.created = temporal.timestamp()
         SET u.handle = $handle, u.updated = temporal.timestamp()`,
      { id, handle }
    );
  }

  async findByHandle(handle: string) {
    const { rows } = await this.db.execute(
      "MATCH (u:User {handle: $handle}) RETURN u {.*} AS user",
      { handle }
    );
    return rows[0]?.user ?? null;
  }
}

// Wire it up — initialization stays async-first at module scope.
const db = await createDatabase();
const users = new UserRepo(db);
```

## Common initialization mistakes

| ❌ Wrong | ✅ Right |
|---|---|
| `const db = new Database()` | `const db = await createDatabase()` |
| `const db = Database.create()` (missing `await`) | `const db = await createDatabase()` |
| `Database.create()` (legacy name) | `createDatabase()` |
| `import { Database } from '@loradb/lora-node'; new Database()` | `import { createDatabase } from '@loradb/lora-node'`; then `await createDatabase()` |
| `createDatabase(undefined, { walDir: './wal' })` | `openWalDatabase({ walDir: './wal' })` |

`Database` is a **type-only** export. Importing it as a value and
calling `new Database()` is a compile error — synchronous
initialization has been removed on purpose.

## Error Handling

Two classes to know:

| Class | When |
|---|---|
| `LoraError` | Any engine-level failure — parse, semantic, runtime |
| `InvalidParamsError` | Host supplied a parameter that couldn't be mapped to a `LoraValue` |

For the engine-level cases see the
[Troubleshooting guide](../troubleshooting).

## Performance / Best Practices

- **Integer precision.** Engine integers are `i64`. JS `number`
  loses precision above `Number.MAX_SAFE_INTEGER` (2^53). For very
  large IDs prefer `bigint` parameters or string encoding. See
  [Troubleshooting → integer precision](../troubleshooting#integer-precision-lost-in-js).
- **Concurrency.** Each `Database` has its own in-memory graph. Auto-commit
  reads can overlap on Arc snapshots; write commits and explicit read-write
  transactions serialize. The event loop stays free while native work runs.
- **No cancellation.** Once dispatched, a query runs to completion.
  Bound variable-length patterns and `UNWIND` list sizes.
- **Dispose explicitly** only when you need to release the native
  handle eagerly, especially before reopening the same archive in
  one process; otherwise GC eventually cleans up.

## See also

- [**Ten-Minute Tour**](./tutorial) — same queries in Node.
- [**Queries → Parameters**](../queries/parameters) — binding typed values.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Data Types**](../data-types/overview) — host-value mapping.
- [**WASM guide**](./wasm) — same API, browser target.
- [**Troubleshooting**](../troubleshooting).
