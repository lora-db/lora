---
title: Running LoraDB in the Browser with WebAssembly
sidebar_label: Browser (WASM)
---

# Running LoraDB in the Browser with WebAssembly

## Overview

`lora-wasm` runs the full LoraDB engine in the browser (or Node) via
WebAssembly. The surface, helpers, and type guards match
[`lora-node`](./node) exactly — the same code ports with an import
swap. For browser apps, prefer the **Worker** variant so the main
thread stays responsive.

## Installation / Setup

### Targets

`lora-wasm` ships three targets out of the same source:

| Target | Use in | Entry |
|---|---|---|
| Node | Server-side JS, tests, scripts | `import { Database } from 'lora-wasm'` |
| Bundler | Vite / webpack / esbuild | `import { Database } from 'lora-wasm/bundler'` |
| Web | Raw `<script type=module>` | `import { Database } from 'lora-wasm/web'` |

### Requirements

- Node.js **20+** for building / testing
- A bundler (Vite, webpack, esbuild, Rollup) for browser usage, _or_
  a host that serves `.wasm` with the correct MIME type.

### Install (from source, pre-release)

```bash
cd crates/lora-wasm
npm install
npm run build      # wasm-pack for all 3 targets + tsc
```

### Install (after publish)

```bash
npm install lora-wasm
```

## Creating a Client / Connection

### In-process (Node or bundler)

```ts
import { Database } from 'lora-wasm';

const db = await Database.create();
```

`Database.create()` bootstraps the WASM module on first call. Every
method returns a Promise for API symmetry with `lora-node` and the
Worker variant.

### Browser Worker (recommended)

```ts
// src/worker.ts
import 'lora-wasm/worker';
```

```ts
// src/main.ts
import { createWorkerDatabase } from 'lora-wasm/worker-client';

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
import { Database } from 'lora-wasm';

const db = await Database.create();

await db.execute("CREATE (:Person {name: 'Ada'})");

const res = await db.execute("MATCH (n:Person) RETURN n.name AS name");
console.log(res.rows); // [ { name: 'Ada' } ]
```

Note: inside WASM, queries execute **synchronously** — the Promise
resolves on the same microtask tick. For heavy queries in the
browser, use the Worker variant.

## Examples

### A. Minimal working example

Shown above.

### B. Parameterised query

```ts
const res = await db.execute(
  "MATCH (u:User) WHERE u.handle = $handle RETURN u.id AS id",
  { handle: 'alice' }
);
```

### C. Structured result handling (typed helpers)

```ts
import { Database, wgs84 } from 'lora-wasm';

const db = await Database.create();

await db.execute(
  "CREATE (:City {name: $name, location: $loc})",
  { name: 'Amsterdam', loc: wgs84(4.89, 52.37) }
);
```

See the [Node guide → typed helpers](./node#typed-helpers) —
`date`, `duration`, `cartesian`, `wgs84`, … export from both
packages with identical signatures.

### D. Application-style example — React + Worker

```tsx
// src/worker.ts
import 'lora-wasm/worker';

// src/useDb.ts
import { createWorkerDatabase, type WorkerDatabase } from 'lora-wasm/worker-client';
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

### E. Error handling

```ts
try {
  await db.execute("BAD QUERY");
} catch (err) {
  // WASM surfaces engine errors as plain Error objects
  console.error((err as Error).message);
}
```

### F. Browser constraints / async

- WASM execution is synchronous inside the Worker — a heavy query
  blocks the worker thread, not the UI. Use one Worker per
  independent read path for concurrency.
- `Database` instances in the main thread and in a Worker have
  **separate** graphs — WASM instances don't share memory. Use
  `execute` to serialise data between them if you need to sync.
- Shared-memory WASM (SAB + threaded wasm-bindgen) is not
  supported.

## Common Patterns

### Persist across reloads with IndexedDB

LoraDB itself is in-memory (see
[Limitations → Storage](../limitations#storage)). Dump and re-seed:

```ts
// Dump every node + edge
const nodes = await db.execute(
  "MATCH (n) RETURN labels(n) AS labels, properties(n) AS props"
);
const edges = await db.execute(
  "MATCH (a)-[r]->(b) RETURN id(a) AS from, id(b) AS to, type(r) AS type, properties(r) AS props"
);
// Store nodes.rows + edges.rows in IndexedDB

// On next load, UNWIND back in via CREATE + MERGE
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
- [**Queries → Parameters**](../queries/#parameters).
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Data Types**](../data-types/overview) — host-value mapping.
- [**Limitations**](../limitations) — persistence caveat.
- [**Troubleshooting**](../troubleshooting).
