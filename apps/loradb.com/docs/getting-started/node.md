---
title: Using LoraDB in Node.js and TypeScript
sidebar_label: Node.js
description: Install and use LoraDB in Node.js or TypeScript via the lora-node N-API binding — async queries on libuv, helpers, type guards, and the shared result shapes.
---

# Using LoraDB in Node.js and TypeScript

## Overview

`lora-node` is a native N-API binding. Queries run on the libuv
threadpool so they don't block the event loop, though parallel calls
on a single `Database` still serialise on the engine mutex. The
surface, helpers, and type guards match the
[WASM binding](./wasm) exactly — the same code ports with an import
swap.

## Installation / Setup

[![npm (@loradb/lora-node)](https://img.shields.io/npm/v/@loradb/lora-node?label=%40loradb%2Flora-node&logo=npm)](https://www.npmjs.com/package/@loradb/lora-node)

### Requirements

- Node.js **18+**
- For building from source: Rust toolchain (`rustup`) +
  `@napi-rs/cli`

### Install

While pre-release, build from source:

```bash
cd crates/lora-node
npm install
npm run build        # builds native .node artifact + TypeScript
```

After publish:

```bash
npm install @loradb/lora-node
```

## Creating a Client / Connection

`lora-node` is **async-only**. The one supported initialization
pattern is `createDatabase()`, which returns a `Promise<Database>`:

```ts
import { createDatabase } from '@loradb/lora-node';

const db = await createDatabase();
```

`createDatabase()` is the single entry point — there is no
synchronous constructor and no `Database.create()` static. This
keeps the Node and WASM surfaces identical (swap the import,
nothing else changes) and lets the binding extend initialization
later without breaking callers.

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
// Five lookups in parallel — each awaits the engine mutex
const handles = ['alice', 'bob', 'carol', 'dan', 'eve'];
const results = await Promise.all(
  handles.map(h =>
    db.execute("MATCH (u:User {handle: $h}) RETURN u.id", { h })
  )
);
```

The event loop stays responsive, but the five queries execute in
series inside the native layer. For read parallelism, spin up
multiple `Database` instances (each with its own graph).

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
```

All return Promises for API symmetry; `clear` / `nodeCount` /
`relationshipCount` run synchronously inside the native layer.

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
         ON CREATE SET u.created = timestamp()
         SET u.handle = $handle, u.updated = timestamp()`,
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
| `import { Database } from '@loradb/lora-node'; new Database()` | `import { createDatabase } from 'lora-node'`; then `await createDatabase()` |

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
- **Concurrency.** Each `Database` has its own in-memory graph
  guarded by a mutex. Parallel `execute()` calls against one
  instance serialise in the native layer — the event loop stays
  free, but execution is one-at-a-time. For read parallelism, spawn
  multiple instances.
- **No cancellation.** Once dispatched, a query runs to completion.
  Bound variable-length patterns and `UNWIND` list sizes.
- **Dispose explicitly** only when you need to release the native
  handle eagerly; otherwise GC cleans up.

## See also

- [**Ten-Minute Tour**](./tutorial) — same queries in Node.
- [**Queries → Parameters**](../queries/parameters) — binding typed values.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Data Types**](../data-types/overview) — host-value mapping.
- [**WASM guide**](./wasm) — same API, browser target.
- [**Troubleshooting**](../troubleshooting).
