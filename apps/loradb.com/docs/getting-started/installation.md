---
title: Install and Set Up LoraDB
sidebar_label: Installation & Setup
description: Pick a LoraDB installation — Node.js, Python, WebAssembly, Go, Ruby, embedded Rust, or the HTTP server — with a short side-by-side of what each runtime is best for.
---

# Install and Set Up LoraDB

## Overview

LoraDB is one Rust engine with bindings for the major application
runtimes — Node.js, Python, WebAssembly, Go, and Ruby — plus a
standalone HTTP server and direct embedding from Rust. Pick whichever
matches your host language; every binding shares the same parser,
planner, executor, and result shape, so switching later is a
mechanical translation. This page helps you pick; each platform guide
covers install, connect, execute, and error handling end-to-end.

## Installation / Setup

### Pick a platform

| Platform | Package | Install | Guide |
|---|---|---|---|
| **Node / TS** | [![npm](https://img.shields.io/npm/v/@loradb/lora-node?label=%40loradb%2Flora-node&logo=npm)](https://www.npmjs.com/package/@loradb/lora-node) | `npm install @loradb/lora-node` | [Node →](./node) |
| **Python** | [![PyPI](https://img.shields.io/pypi/v/lora-python?label=pypi&logo=pypi&logoColor=white)](https://pypi.org/project/lora-python/) | `pip install lora-python` | [Python →](./python) |
| **Browser / WASM** | [![npm](https://img.shields.io/npm/v/@loradb/lora-wasm?label=%40loradb%2Flora-wasm&logo=npm)](https://www.npmjs.com/package/@loradb/lora-wasm) | `npm install @loradb/lora-wasm` | [WASM →](./wasm) |
| **Go** | [pkg.go.dev](https://pkg.go.dev/github.com/lora-db/lora/crates/lora-go) | `go get github.com/lora-db/lora/crates/lora-go` | [Go →](./go) |
| **Ruby** | [![Gem](https://img.shields.io/gem/v/lora-ruby?label=lora-ruby&logo=rubygems&logoColor=white)](https://rubygems.org/gems/lora-ruby) | `gem install lora-ruby` | [Ruby →](./ruby) |

:::tip

Click any badge to jump to its package-registry page. Each platform
guide also documents repo-local build steps for contributors working
from a clone.

:::

### Which to pick?

| If you… | Pick |
|---|---|
| Ship a Node server / CLI | Node.js |
| Build in Python (sync or asyncio) | Python |
| Run in the browser / Web Worker / edge | WASM |
| Build a Go service or CLI (cgo) | Go |
| Ship a Ruby app, worker, or Rails service | Ruby |

All bindings share the same query surface and result shape, so
switching later is a mechanical translation — the Cypher is
identical.

### Other runtimes

If you're building the engine into Rust directly, or prefer to reach
it over the wire, those paths are documented separately and share the
same Cypher surface:

- [**Rust crate**](./rust) — embed `lora-database` inline in your
  Rust binary for the lowest-overhead option.
  [![crates.io](https://img.shields.io/crates/v/lora-database?label=crates.io&logo=rust)](https://crates.io/crates/lora-database)
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
