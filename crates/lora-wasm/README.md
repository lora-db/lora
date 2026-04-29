# lora-wasm

WebAssembly bindings for the [Lora](../../README.md) in-memory graph
database. The package is designed for browsers and Node.js and exposes a
**strongly typed, async-facing API** that keeps the main thread responsive:
heavy query work can run inside a Web Worker while your UI code simply awaits
the result.

> **Status:** prototype / feasibility check. Not published to npm.

## Build

```bash
cd crates/lora-wasm
npm install
npm run build              # wasm-pack (node + bundler) + tsc
npm test                   # vitest
```

Build artefacts:

| Directory         | Target              | Purpose                                                         |
|-------------------|---------------------|-----------------------------------------------------------------|
| `pkg-node/`       | `--target nodejs`   | In-process usage from Node (vitest, CLI, loader-node.ts)        |
| `pkg-bundler/`    | `--target bundler`  | Consumption via Vite/webpack/esbuild                            |
| `pkg-web/`        | `--target web`      | Browser Worker entry — self-fetches the `.wasm` binary          |
| `dist/`           | TypeScript (`tsc`)  | Compiled wrapper (`Database`, worker, worker-client, types)     |

To run the full validation suite (typecheck, vitest, Playwright browser
test, npm pack dry-run), add:

```bash
npm run typecheck
npm test
npm run test:browser:install   # one-time chromium download
npm run test:browser
npm run pack:dry
```

## Execution modes

### 1. Default: Worker first, main-thread fallback

`lora-wasm` is **async-only** — the sole initialization pattern is
`createDatabase()`, which bootstraps the WASM module on first call.
There is no synchronous constructor and no `Database.create()`
static; `Database` is a type-only export.

In browser-like hosts, `createDatabase()` tries to spawn the packaged module
Worker first and pings it before returning. If the Worker cannot be created or
fails during startup, the package emits one `console.warn` and falls back to
the in-process WASM engine so the app can still run.

```ts
import { createDatabase, isNode } from "lora-wasm";

const db = await createDatabase(); // Worker-backed in browsers when possible
await db.execute("CREATE (:Person {name: $n})", { n: "Alice" });

const r = await db.execute("MATCH (n:Person) RETURN n");
for (const row of r.rows) {
  if (isNode(row.n)) console.log(row.n.properties.name);
}
```

To force the in-process engine:

```ts
const db = await createDatabase({ runtime: "main-thread" });
```

To require Worker startup instead of falling back:

```ts
const db = await createDatabase({ runtime: "worker" });
```

To disable the fallback warning:

```ts
const db = await createDatabase({ warnOnFallback: false });
```

### 2. Explicit main-thread WASM

```ts
import { createMainThreadDatabase } from "lora-wasm";

const db = await createMainThreadDatabase();
```

### 3. Custom Web Worker

Use this when you need to control the Worker URL, lifecycle, bundler entry, or
pooling strategy.

```ts
import { createWorkerDatabase } from "lora-wasm/worker-client";

const worker = new Worker(new URL("./worker.js", import.meta.url), {
  type: "module",
});
const db = createWorkerDatabase(worker);

await db.execute("CREATE (:N {n: 1})");     // runs off-main-thread
const { rows } = await db.execute("MATCH (n) RETURN n.n AS n");
```

The worker entry (`ts/worker.ts`) hosts the WASM module. The main thread only
posts messages, so long-running queries never block the event loop / UI.

### 4. In-process but typed like the worker (advanced)

The same `createWorkerDatabase` signature accepts any `WorkerLike` object —
useful for tests and for swapping execution backends behind the same API.

## Snapshots

WASM has no filesystem access, so snapshots never accept string paths.
`saveSnapshot()` defaults to `Uint8Array`, and can also return `ArrayBuffer`,
`Blob`, `Response`, `ReadableStream`, or an object `URL`. `loadSnapshot`
accepts `URL`, `Uint8Array`, `ArrayBuffer`, `Blob`, `Response`, and web
`ReadableStream<Uint8Array | ArrayBuffer>`. The bytes use the database
snapshot codec (`LORACOL1`) and can be loaded by native LoraDB. Older store
snapshots (`LORASNAP`) are still accepted by `loadSnapshot` for compatibility.
Snapshot bytes are uncompressed by default; pass `{ compression: "gzip" }` or
`{ compression: { format: "gzip", level: 1 } }` to save smaller
WASM-portable snapshots. Gzip levels `0..9` are supported; level `1` is the
fast default.

```ts
import { createDatabase } from "lora-wasm";

const db = await createDatabase();
await db.execute("CREATE (:Person {name: 'Alice'})");

const bytes = await db.saveSnapshot();
const compressed = await db.saveSnapshot({ compression: "gzip" });
const blob = await db.saveSnapshot({ format: "blob", compression: "gzip" });
const response = await db.saveSnapshot({ format: "response" });

await db.loadSnapshot(bytes);
await db.loadSnapshot(compressed);
await db.loadSnapshot(blob);
await db.loadSnapshot(response);
await db.loadSnapshot(new URL("/graph.lorasnap", location.href));
```

## Typed value model

| TS type                              | Runtime shape                                                               |
|--------------------------------------|-----------------------------------------------------------------------------|
| `null`/`boolean`/`number`/`string`   | pass-through                                                                |
| `LoraValue[]` / nested record      | arrays / objects                                                            |
| `LoraNode`                         | `{ kind: "node", id, labels, properties }`                                  |
| `LoraRelationship`                 | `{ kind: "relationship", id, startId, endId, type, properties }`            |
| `LoraPath`                         | `{ kind: "path", nodes: number[], rels: number[] }`                         |
| `LoraDate`…`LoraDuration`        | `{ kind: "date", iso: "YYYY-MM-DD" }` etc.                                  |
| `LoraPoint`                        | Discriminated union on `srid` — see below                                   |

`LoraPoint` is a union of four CRS-specific shapes:

| Shape                                                                                                   | Meaning       |
|---------------------------------------------------------------------------------------------------------|---------------|
| `{ kind: "point", srid: 7203, crs: "cartesian", x, y }`                                                 | Cartesian 2D  |
| `{ kind: "point", srid: 9157, crs: "cartesian-3D", x, y, z }`                                           | Cartesian 3D  |
| `{ kind: "point", srid: 4326, crs: "WGS-84-2D", x, y, longitude, latitude }`                            | WGS-84 2D     |
| `{ kind: "point", srid: 4979, crs: "WGS-84-3D", x, y, z, longitude, latitude, height }`                 | WGS-84 3D     |

Helper constructors: `date`, `time`, `datetime`, `localtime`, `localdatetime`,
`duration`, `cartesian`, `cartesian3d`, `wgs84`, `wgs84_3d`. Guards:
`isNode`, `isRelationship`, `isPath`, `isPoint`, `isTemporal`.

> `distance()` on WGS-84-3D points ignores `height`. See
> [functions reference](../../apps/loradb.com/docs/functions/overview.md) for the spatial
> reference and out-of-scope operations.

## Errors

`db.execute(...)` and the worker client throw `LoraError` with a narrowed
`code`:

- `LORA_ERROR` — parse / analyze / execute failure
- `INVALID_PARAMS` — a param value could not be mapped to a Lora value
- `WORKER_ERROR` — worker transport / lifecycle failure (worker client only)

## Shared type contract

The public TypeScript value model (`LoraValue`, `LoraNode`, …,
`QueryResult`, `LoraError`) lives in a single canonical file at
`crates/shared-ts/types.ts` and is copied into each consumer package by
its `sync:types` npm script. CI runs `verify:types` to fail on drift.
That keeps `lora-node` and `lora-wasm` locked to one identical
public surface — consumers can swap backends without rewriting types.

## Known limitations

- The wasm module is single-threaded. Parallel queries inside one worker
  serialise; spawn more workers for true parallelism.
- I64 values are delivered as JS `number` and lose precision above 2^53.
  Applications that need bigint precision should use the native
  `lora-node` binding instead.
- Wall-clock reads (`date()`, `datetime()`, `time()`, `localdatetime()`,
  `localtime()`) are routed through `js_sys::Date::now()` on wasm32 via
  the shim in `lora-store::temporal::unix_now`. The browser clock is
  millisecond-granular, so the nanosecond field on returned values is
  zero below the millisecond boundary.
- Engine errors cross the worker boundary as `LoraError` with a
  narrowed `code`; the engine does not currently offer query cancellation.

## Feasibility assessment

**What works today**

- `cargo check --target wasm32-unknown-unknown` passes for the whole
  database pipeline (`lora-ast`, `lora-parser`, `lora-analyzer`,
  `lora-compiler`, `lora-executor`, `lora-store`,
  `lora-database`, `lora-wasm`).
- `wasm-pack build` succeeds for three targets (`nodejs`, `bundler`,
  `web`) with a ~2.2 MB optimised `.wasm` each.
- The non-blocking worker-backed path is end-to-end verified: a real
  Chromium instance spawns a module Worker, loads the `pkg-web` bundle,
  and runs a CREATE + MATCH round-trip — asserted by a Playwright test
  behind `npm run test:browser`.
- `vitest` runs 18 in-process tests against the `nodejs` bundle,
  covering scalars, nested maps/lists, nodes, relationships, paths,
  points, all temporal kinds including `date()`/`datetime()` no-arg
  forms, parameter validation errors, and concurrent queries across the
  worker message protocol.
- The TypeScript public contract is shared verbatim with `lora-node`
  via `crates/shared-ts/types.ts`, enforced by `verify:types` in CI.
- `npm pack --dry-run` produces a 45-file, 6.6 MB tarball containing
  all three wasm bundles plus the compiled TS wrapper. No `file:` deps,
  no postinstall scripts.

**What still blocks a real npm publish**

- Package is marked `private: true` and the workspace is pre-1.0.
  Publishing needs a final scope decision (e.g. `@lora/wasm`),
  a LICENSE file, and a repository URL in `Cargo.toml`/`package.json`.
- The three wasm bundles are each ~2.2 MB. A production publish should
  either ship conditional exports that load only the bundle the
  consumer imports, or switch to compressed `.wasm` + `fetch` with
  `Content-Encoding: br`.
- I64 precision is capped at `Number.MAX_SAFE_INTEGER`. For larger
  integer properties we need a `bigint`-aware serializer on the wasm
  boundary.
- The engine is synchronous inside wasm — a single Worker serialises
  queries. Multi-tenant workloads need either multiple Workers (already
  possible), or an engine-level cooperative scheduler.
- No query cancellation or progress streaming crosses the Worker
  boundary; a slow query blocks that Worker until completion.

**Bottom line**

Yes — this database can be used from JavaScript and TypeScript in
practice, in the browser and in Node, without native bindings. The
worker-backed path keeps heavy query work off the main thread by
default, and the strongly-typed wrapper makes query results ergonomic
for TS consumers. The remaining gaps are packaging polish and two
well-known wasm constraints (i64 precision, no cancellation); neither
blocks feasibility.
