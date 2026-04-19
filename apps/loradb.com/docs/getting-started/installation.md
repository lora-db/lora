---
title: Install and Set Up LoraDB
sidebar_label: Installation & Setup
---

# Install and Set Up LoraDB

## Overview

LoraDB is one Rust engine with bindings for three primary runtimes —
Node.js, Python, and WebAssembly. Pick whichever matches your host
language; every binding shares the same parser, planner, executor,
and result shape, so switching later is a mechanical translation.
This page helps you pick; each platform guide covers install,
connect, execute, and error handling end-to-end.

## Installation / Setup

### Pick a platform

| | Install | Import | Guide |
|---|---|---|---|
| **Node / TS** | `npm install lora-node` | `import { Database } from 'lora-node'` | [Node →](./node) |
| **Python** | `pip install lora-python` | `from lora_python import Database` | [Python →](./python) |
| **Browser / WASM** | `npm install lora-wasm` | `import { Database } from 'lora-wasm'` | [WASM →](./wasm) |

:::note

Pre-release — packages aren't on npm / PyPI yet. Each platform guide
includes repo-local build steps.

:::

### Which to pick?

| If you… | Pick |
|---|---|
| Ship a Node server / CLI | Node.js |
| Build in Python (sync or asyncio) | Python |
| Run in the browser / Web Worker / edge | WASM |

All bindings share the same query surface and result shape, so
switching later is a mechanical translation — the Cypher is
identical.

### Other runtimes

If you're building the engine into Rust directly, or prefer to reach
it over the wire, those paths are documented separately and share the
same Cypher surface:

- [**Rust crate**](./rust) — embed `lora-database` inline in your
  Rust binary for the lowest-overhead option.
- [**HTTP server**](./server) — run `lora-server` and `POST /query`
  from any language.

## Creating a Client / Connection

Every binding exposes the same two primitives:

1. A `Database` with `execute(query, params?)`.
2. A result: `{ columns, rows }`, where each row maps column name →
   typed value.

See each platform guide for the language-specific shape.

## Running Your First Query

```cypher
CREATE (:Person {name: 'Ada'})
```

```cypher
MATCH (p:Person) RETURN p.name
```

In any binding that's two `execute` calls; the platform guide shows
the language-specific syntax.

## Examples

### Shared value model

Typed values follow one contract (defined in
`crates/shared-ts/types.ts`): primitives, lists/maps, graph entities
(tagged `{kind: "node" | "relationship" | "path"}`), temporals
(tagged `{kind: "date" | "datetime" | ...}`), and points (tagged
`{kind: "point", srid, crs, ...}`).

See [**Data Types Overview**](../data-types/overview) for the full
catalogue and each binding's parameters section for how host values
map in.

## Common Patterns

### Embed one database

Each binding defaults to **one process, one in-memory graph**.
Parallel queries on the same handle serialise on a mutex — spawn
multiple `Database` instances for read parallelism.

### Bulk-load from the host

The idiomatic large-write shape across every binding is
[`UNWIND $rows AS row CREATE …`](../queries/unwind-merge#bulk-load-from-parameter).
The `$rows` parameter comes from a plain list in the host language.

### Share a database across modules

Wrap the handle in whatever sharing primitive your language provides
— `Arc` in Rust, a module singleton in Node/Python, a Worker in the
browser.

## Error Handling

Every binding exposes two error layers:

- **Query-level errors** — parse, semantic, or runtime — surface the
  engine's message. Typical cases live in
  [Troubleshooting](../troubleshooting).
- **Connection / host-level errors** — language-specific (HTTP
  status, FFI exceptions, spawn failures). Each platform guide
  covers its own.

## Performance / Best Practices

- **No persistence.** Data lives in memory; see
  [Limitations → Storage](../limitations#storage).
- **No query cancellation.** Once dispatched, queries run to
  completion. Keep queries bounded (`LIMIT`, `*..N` caps).
- **Parameters, not string interpolation.** The only safe way to
  mix untrusted input into a query.

## See also

- [**Ten-Minute Tour**](./tutorial) — guided walkthrough.
- [**Graph Model**](../concepts/graph-model) — what lives in the graph.
- [**Query Examples**](../queries/examples) — copy-paste recipes.
- [**Cookbook**](../cookbook) — scenario-based recipes.
- [**Data Types**](../data-types/overview) — values and parameters.
- [**Troubleshooting**](../troubleshooting) — when something goes wrong.
