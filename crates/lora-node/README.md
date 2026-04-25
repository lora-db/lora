# lora-node

Node.js / TypeScript bindings for the [Lora](../../README.md) graph
engine. The package exposes a first-class typed API: query results are
modelled as discriminated unions, temporal values carry `kind` tags, and the
`Database` class is strongly typed in both directions (params and rows).

**Non-blocking:** `execute()` dispatches each query to the libuv threadpool via
[`napi::Task`](https://napi.rs/docs/compat-mode/async-task). The JS event loop
stays free for the full duration of a query — a 2 000-node MATCH happily
interleaves with `setImmediate` ticks on the main thread (proven by a
dedicated vitest).

> **Status:** prototype / feasibility check. Not published to npm.

## Install (local dev)

```bash
cd crates/lora-node
npm install
npm run build   # builds the Rust cdylib + TypeScript declarations
npm test        # runs the vitest suite
```

The `npm run build:native` step uses [`@napi-rs/cli`](https://napi.rs/) and
produces a platform-specific `lora-node.<platform>-<arch>.node` artifact next
to `package.json`.

## Usage

`lora-node` is **async-only** — the sole initialization pattern is
`createDatabase(...)`. There is no synchronous constructor and no
`Database.create()` static; `Database` is a type-only export.

```ts
import { createDatabase, isNode, type LoraNode } from "lora-node";

const db = await createDatabase(); // in-memory by default
await db.execute("CREATE (:Person {name: $n, age: $a})", { n: "Alice", a: 30 });

const res = await db.execute<{ n: LoraNode }>("MATCH (n:Person) RETURN n");
for (const row of res.rows) {
  if (isNode(row.n)) {
    console.log(row.n.properties.name);
  }
}
```

The initialization rule is:

```ts
import { createDatabase } from "lora-node";

const inMemory = await createDatabase();            // in-memory only
const persistent = await createDatabase("./app");  // persistent: pass a directory string
```

If you want persistence, pass a **directory string** to `createDatabase(...)`.

Node also has a WAL-backed convenience overload:

```ts
import { createDatabase } from "lora-node";

const db = await createDatabase("./.lora-wal"); // persistent: directory string
```

The string is treated as the WAL directory path verbatim. Relative
paths resolve from the current working directory. This is a Node-only
initialization convenience; the query surface, shared types, and async
method signatures still match `lora-wasm`.

Call `db.dispose()` when you need to release the native handle eagerly,
especially before reopening the same WAL directory in the same process.

## Typed value model

| TS type                 | Runtime shape                                                                 |
|-------------------------|-------------------------------------------------------------------------------|
| `null`/`boolean`/`number`/`string` | pass-through JS primitives                                                     |
| `LoraValue[]` / object | homogeneous arrays and nested records                                          |
| `LoraNode`            | `{ kind: "node", id, labels, properties }`                                      |
| `LoraRelationship`    | `{ kind: "relationship", id, startId, endId, type, properties }`                |
| `LoraPath`            | `{ kind: "path", nodes: number[], rels: number[] }`                             |
| `LoraDate`…`LoraDuration` | `{ kind: "date", iso: "YYYY-MM-DD" }` etc.                              |
| `LoraPoint`           | Discriminated union on `srid`, see below                                       |

`LoraPoint` is a discriminated union over the four supported CRSes:

| Shape                                                                                                        | Meaning              |
|--------------------------------------------------------------------------------------------------------------|----------------------|
| `{ kind: "point", srid: 7203, crs: "cartesian", x, y }`                                                      | Cartesian 2D         |
| `{ kind: "point", srid: 9157, crs: "cartesian-3D", x, y, z }`                                                | Cartesian 3D         |
| `{ kind: "point", srid: 4326, crs: "WGS-84-2D", x, y, longitude, latitude }`                                 | WGS-84 2D            |
| `{ kind: "point", srid: 4979, crs: "WGS-84-3D", x, y, z, longitude, latitude, height }`                      | WGS-84 3D            |

Helper constructors (`date("2025-01-15")`, `cartesian(1, 2)`, `cartesian3d(1, 2, 3)`,
`wgs84(lon, lat)`, `wgs84_3d(lon, lat, height)`, `duration("P1M")`, …) and
narrowing guards (`isNode`, `isRelationship`, `isPath`, `isPoint`, `isTemporal`)
are exported from `lora-node`.

> `distance()` on WGS-84-3D points ignores `height` — see
> [functions reference](../../apps/loradb.com/docs/functions/overview.md) for the full spatial
> reference and known limitations.

## Architecture

```
lora-database (Rust)
   └── lora-node (crate, cdylib)        <- napi-rs bindings, AsyncTask
          └── ts/index.ts                 <- strongly-typed async wrapper
                 └── ../shared-ts/types.ts  <- shared TS contract (with lora-wasm)
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

The Rust crate is added to the workspace root (`Cargo.toml`). The Node side is
self-contained inside this directory. Only sub-millisecond operations
(`clear`, `nodeCount`, `relationshipCount`) stay synchronous inside napi; the
TS wrapper still exposes them as `Promise`-returning methods to keep the API
identical to `lora-wasm`.

## Errors

`db.execute(...)` throws `LoraError` with a narrowed `code`:

- `LORA_ERROR` — parse / analyze / execute failure
- `INVALID_PARAMS` — a param value could not be mapped to a `LoraValue`

## Known limitations

- **Concurrent writes.** Each `execute()` hops through the threadpool and
  serialises on the store mutex inside Rust, so concurrent read/read and
  read/write traffic works. Firing many concurrent write queries against
  the same `Database` (e.g. 2 000 parallel `CREATE`s via `Promise.all`)
  can expose races in the underlying engine — treat the mutex as
  per-operation, not per-query. Prefer `await`-in-a-loop or a single
  batched query for heavy write workloads.
- **I64 precision.** Integer values above `Number.MAX_SAFE_INTEGER`
  (2^53) are returned as JS `number` and lose precision. A `bigint`-aware
  path would require extending the value serializer.
- **Cancellation.** The napi `Task` abstraction does not support
  cancellation once dispatched; a runaway query runs to completion.
- **WAL surface.** This first Node persistence slice only exposes
  WAL-backed initialization via `createDatabase(walDir)`. Checkpoint,
  truncate, status, and sync-mode controls are not exposed yet.
- **WAL directory ownership.** A WAL directory can only be open by one
  live handle at a time. Dispose the first handle before reopening the
  same directory.
