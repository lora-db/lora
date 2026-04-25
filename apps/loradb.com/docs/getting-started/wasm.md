---
title: Running LoraDB in the Browser with WebAssembly
sidebar_label: Browser (WASM)
description: Run LoraDB in the browser via WebAssembly with lora-wasm — including the Worker variant for keeping the main thread responsive and the same API as the Node binding.
---

# Running LoraDB in the Browser with WebAssembly

## Overview

`lora-wasm` runs the full LoraDB engine in the browser (or Node) via
WebAssembly. The surface, helpers, and type guards match
[`lora-node`](./node) exactly — the same code ports with an import
swap. For browser apps, prefer the **Worker** variant so the main
thread stays responsive.

## Installation / Setup

[![npm (@loradb/lora-wasm)](https://img.shields.io/npm/v/@loradb/lora-wasm?label=%40loradb%2Flora-wasm&logo=npm)](https://www.npmjs.com/package/@loradb/lora-wasm)

### Targets

`lora-wasm` ships three targets out of the same source:

| Target | Use in | Entry |
|---|---|---|
| Node | Server-side JS, tests, scripts | `import { createDatabase } from '@loradb/lora-wasm'` |
| Bundler | Vite / webpack / esbuild | `import { createDatabase } from '@loradb/lora-wasm/bundler'` |
| Web | Raw `<script type=module>` | `import { createDatabase } from '@loradb/lora-wasm/web'` |

### Requirements

- Node.js **20+** for building / testing
- A bundler (Vite, webpack, esbuild, Rollup) for browser usage, _or_
  a host that serves `.wasm` with the correct MIME type.

### Install

```bash
npm install @loradb/lora-wasm
```

## Creating a Client / Connection

### In-process (Node or bundler)

`lora-wasm` is **async-only**. The one supported initialization
pattern is `createDatabase()`:

```ts
import { createDatabase } from '@loradb/lora-wasm';

const db = await createDatabase();
```

`createDatabase()` is the single entry point — there is no
synchronous constructor and no `Database.create()` static. It
bootstraps the WASM module on the first call, so the engine is
guaranteed to be ready before the first query runs. Every method
on the returned instance returns a Promise for API symmetry with
`lora-node` and the Worker variant.

Unlike `lora-node`, the WASM binding does **not** accept a directory
string for persistent initialization. `createDatabase()` is always an
in-memory database; persistency in WASM is byte-based through
`saveSnapshotToBytes` / `loadSnapshotFromBytes`.

:::caution Do not skip the `await`
`createDatabase()` returns a `Promise`. Calling `execute()` on the
unresolved promise will throw. Always `await` the factory before
running queries, and never instantiate the `Database` type
directly — it is exported as a **type only**.
:::

### Browser Worker (recommended)

```ts
// src/worker.ts
import 'lora-wasm/worker';
```

```ts
// src/main.ts
import { createWorkerDatabase } from '@loradb/lora-wasm/worker-client';

const worker = new Worker(new URL('./worker.ts', import.meta.url), {
  type: 'module',
});

const db = createWorkerDatabase(worker);
```

`WorkerDatabase` has the same surface as `Database` (`execute`,
`clear`, `nodeCount`, `relationshipCount`). Every call posts a
message to the worker and awaits the reply, so the main thread
never blocks on the engine.

## Running Your First Query

```ts
import { createDatabase } from '@loradb/lora-wasm';

const db = await createDatabase();

await db.execute("CREATE (:Person {name: 'Ada'})");

const res = await db.execute("MATCH (n:Person) RETURN n.name AS name");
console.log(res.rows); // [ { name: 'Ada' } ]
```

Note: inside WASM, queries execute **synchronously** — the Promise
resolves on the same microtask tick. For heavy queries in the
browser, use the Worker variant.

## Examples

### Minimal working example

Shown above.

### Parameterised query

```ts
const res = await db.execute(
  "MATCH (u:User) WHERE u.handle = $handle RETURN u.id AS id",
  { handle: 'alice' }
);
```

### Structured result handling (typed helpers)

```ts
import { createDatabase, wgs84 } from '@loradb/lora-wasm';

const db = await createDatabase();

await db.execute(
  "CREATE (:City {name: $name, location: $loc})",
  { name: 'Amsterdam', loc: wgs84(4.89, 52.37) }
);
```

See the [Node guide → typed helpers](./node#typed-helpers) —
`date`, `duration`, `cartesian`, `wgs84`, … export from both
packages with identical signatures.

### React + Worker example

```tsx
// src/worker.ts
import 'lora-wasm/worker';

// src/useDb.ts
import { createWorkerDatabase, type WorkerDatabase } from '@loradb/lora-wasm/worker-client';
import { useEffect, useState } from 'react';

let dbPromise: Promise<WorkerDatabase> | null = null;

function getDb() {
  if (!dbPromise) {
    const worker = new Worker(new URL('./worker.ts', import.meta.url), {
      type: 'module',
    });
    dbPromise = Promise.resolve(createWorkerDatabase(worker));
  }
  return dbPromise;
}

export function useUserCount() {
  const [n, setN] = useState<number | null>(null);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const db = await getDb();
      const { rows } = await db.execute(
        "MATCH (u:User) RETURN count(*) AS n"
      );
      if (!cancelled) setN(rows[0].n as number);
    })();
    return () => { cancelled = true; };
  }, []);
  return n;
}
```

The main thread posts messages; the engine runs in the Worker; the
UI stays interactive.

### Handle errors

```ts
try {
  await db.execute("BAD QUERY");
} catch (err) {
  // WASM surfaces engine errors as plain Error objects
  console.error((err as Error).message);
}
```

### Browser constraints and concurrency

- WASM execution is synchronous inside the Worker — a heavy query
  blocks the worker thread, not the UI. Use one Worker per
  independent read path for concurrency.
- `Database` instances in the main thread and in a Worker have
  **separate** graphs — WASM instances don't share memory. Use
  `execute` to serialise data between them if you need to sync.
- Shared-memory WASM (SAB + threaded wasm-bindgen) is not
  supported.

## Common Patterns

### Persisting your graph

The browser WASM binding has no filesystem, so the snapshot API is
**byte-in / byte-out**. Save produces a `Uint8Array`; load consumes
one. Store the bytes wherever your app already stores state —
IndexedDB, the fetch API, OPFS, a backend:

```ts
// Dump the full graph to bytes.
const bytes: Uint8Array = await db.saveSnapshotToBytes();

// Later (same or next session), restore from bytes.
await db.loadSnapshotFromBytes(bytes);
```

The Node target of `@loradb/lora-wasm` exposes the same byte API for
parity, so host code ports unchanged between targets (use the
filesystem-backed `saveSnapshot(path)` on `@loradb/lora-node` only
when you want a path-based API).

The Worker-backed surface (`createWorkerDatabase`) does not yet plumb
snapshots through the worker protocol. To snapshot from a worker
today, call `saveSnapshotToBytes` inside the worker and post the bytes
back to the main thread yourself.

See the canonical [Snapshots guide](../snapshot) for the full metadata
shape and atomic-rename guarantees (the latter apply to path-based
writes in the other bindings; byte-based persistence is atomic only as
far as the surrounding storage layer allows).

### Persist across reloads with IndexedDB

```ts
const DB = 'loradb-snapshots', STORE = 'graph', KEY = 'main';

async function idb(): Promise<IDBDatabase> {
  return await new Promise((ok, err) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore(STORE);
    r.onsuccess = () => ok(r.result);
    r.onerror   = () => err(r.error);
  });
}

async function saveToIdb(db: Database) {
  const bytes = await db.saveSnapshotToBytes();
  const idbDb = await idb();
  await new Promise<void>((ok, err) => {
    const tx = idbDb.transaction(STORE, 'readwrite');
    tx.objectStore(STORE).put(bytes, KEY);
    tx.oncomplete = () => ok();
    tx.onerror    = () => err(tx.error);
  });
}

async function loadFromIdb(db: Database) {
  const idbDb = await idb();
  const bytes = await new Promise<Uint8Array | undefined>((ok, err) => {
    const tx = idbDb.transaction(STORE, 'readonly');
    const r  = tx.objectStore(STORE).get(KEY);
    r.onsuccess = () => ok(r.result);
    r.onerror   = () => err(r.error);
  });
  if (bytes) await db.loadSnapshotFromBytes(bytes);
}
```

### Run heavy queries without blocking the UI

Use the Worker variant — see
[Browser Worker (recommended)](#browser-worker-recommended) above.
Every call posts a message and awaits the reply, so the main thread
stays interactive.

### Bundler notes

#### Vite

```ts
// vite.config.ts
import { defineConfig } from 'vite';

export default defineConfig({
  optimizeDeps: { exclude: ['lora-wasm'] },
  worker: { format: 'es' },
});
```

#### webpack / Next.js

Ensure `.wasm` is served with `Content-Type: application/wasm`. For
Next.js, mark the package as `serverExternalPackages` if you use it
only on the edge / server.

#### Raw browser

The `/web` subpath loads `.wasm` relative to the current page.
You'll need to serve the package files unmodified.

### Methods

```ts
await db.execute(query, params?);       // returns { columns, rows }
await db.clear();
await db.nodeCount();
await db.relationshipCount();
db.dispose();                           // release the WASM handle
```

`dispose()` drops the underlying WASM reference. After calling it,
further `execute` calls will throw.

## Common initialization mistakes

| ❌ Wrong | ✅ Right |
|---|---|
| `const db = new Database()` | `const db = await createDatabase()` |
| `await init(); const db = new Database()` | `const db = await createDatabase()` (init is handled inside) |
| `const db = Database.create()` (missing `await`) | `const db = await createDatabase()` |
| `Database.create()` (legacy name) | `createDatabase()` |

`Database` is a **type-only** export in `lora-wasm`. Importing it
as a value and calling `new Database()` is a compile error —
synchronous initialization has been removed so the WASM module
can never be queried before it is bootstrapped.

## Error Handling

WASM surfaces engine errors as plain `Error` with the engine's
message. There is no structured error class equivalent to
`lora-node`'s `LoraError` — match on the message text or let it
bubble to a generic handler.

## Performance / Best Practices

- **Single-threaded by default.** Parallel `execute()` calls on one
  instance serialise. For parallel reads in the browser, spin up
  multiple Workers.
- **Integer precision.** Same 2^53 limit as `lora-node` — `i64`
  values outside the safe integer range lose precision.
- **Wall-clock resolution.** `date()` / `datetime()` without
  arguments use `performance.now()` / `Date.now()` at millisecond
  granularity — the nanosecond field is zero.
- **Bundle size.** Each target is ~2 MB uncompressed. For
  production, serve compressed (`.wasm` → Brotli / gzip).

## See also

- [**Node guide**](./node) — shared surface, helpers, type guards.
- [**Queries → Parameters**](../queries/parameters) — typed parameter binding.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Data Types**](../data-types/overview) — host-value mapping.
- [**Limitations**](../limitations) — persistence caveat.
- [**Troubleshooting**](../troubleshooting).
