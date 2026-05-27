---
title: Install and Set Up LoraDB
sidebar_label: Installation & Setup
description: Pick a LoraDB installation — Node.js, Python, WebAssembly, Go, Ruby, embedded Rust, or the HTTP server — with a short side-by-side of what each runtime is best for.
---

# Install and Set Up LoraDB

## Overview

LoraDB is one Rust engine with bindings for the major application
runtimes — Node.js, Python, WebAssembly, Go, and Ruby — plus a
standalone HTTP server and direct embedding from Rust. Every binding
shares the same parser, planner, executor, and result shape, so
switching hosts later is a mechanical translation. This page helps
you pick; each binding guide covers install, connect, execute, and
error handling end-to-end.

If you only want to try the query surface first, start with the
[browser playground](./playground). It runs LoraDB through WASM without
installing a package.

<HowTo
  name="Install LoraDB and run your first query"
  description="The shortest happy path: pick the binding for your runtime, install one package, open an in-memory handle, and run a Cypher query."
  totalTime="PT5M"
  steps={[
    {
      name: "Pick a runtime",
      text: "Choose the binding that matches your host: Node.js, Python, WebAssembly in the browser, Go, Ruby, the lora-database Rust crate for direct embedding, or the lora-server HTTP service. Every binding wraps the same engine.",
      url: "#pick-a-platform",
    },
    {
      name: "Install the package",
      text: "Run npm install @loradb/lora-node, pip install lora-python, npm install @loradb/lora-wasm, go get github.com/lora-db/lora/crates/bindings/lora-go, gem install loradb, or cargo add lora-database depending on the runtime you picked.",
      url: "#installation--setup",
    },
    {
      name: "Open an in-memory database",
      text: "Call Database.create() (Node, Python, Ruby) or its equivalent in your binding. The handle starts with an empty graph and is safe to share across threads / async tasks.",
      url: "#creating-a-client--connection",
    },
    {
      name: "Run a Cypher query",
      text: "Execute CREATE (:Person {name: 'Ada'}) followed by MATCH (p:Person) RETURN p.name to confirm the install works end-to-end.",
      url: "#running-your-first-query",
    },
    {
      name: "Add persistence when ready",
      text: "Save the graph with a snapshot (saveSnapshot / save_snapshot_to) or open with WAL-backed durability for continuous persistence. Snapshots are atomic on rename; WAL replays above the snapshot fence on recover.",
      url: "#examples",
    },
  ]}
/>

## First-time checklist

Before choosing an install path, decide three things:

| Question | Recommendation |
|---|---|
| Do you need parameters for user input? | Use any binding or HTTP `/query` with `params`. Never interpolate raw input into query text. |
| Should data survive process restarts? | Open a named `.loradb` archive, save snapshots, or use a WAL-backed open. Plain in-memory handles are scratch graphs. |
| Will the graph be reachable over a network? | Prefer an in-process binding. If you use `lora-server`, keep it on `127.0.0.1` or put auth, TLS, and rate limiting in front. |

The shortest happy path for most app developers is: install the
binding for your host language, run the minimal example in that guide,
then add persistence only when the prototype needs it.

## Installation / Setup

### Requirements by surface

| Surface | Extra requirements |
|---|---|
| Node / TS | Node.js 18+ for the published package; Node.js 20+ for repo-local workspace tooling |
| Python | Python 3.8+; `maturin` only when building from source |
| WASM | Node.js 20+ for bundling/testing |
| Go | Go 1.21+, cgo enabled, C toolchain, and `liblora_ffi` |
| Ruby | Ruby 3.1+, Bundler, and a native build toolchain |
| Rust / server from source | Rust 1.87+ through `rustup` |

Published packages hide most native build steps. Repo-local development needs
the toolchains above because the bindings compile the Rust engine.

### Pick a platform

| Platform | Package | Install | Guide |
|---|---|---|---|
| **Node / TS** | [![npm](https://img.shields.io/npm/v/@loradb/lora-node?label=%40loradb%2Flora-node&logo=npm)](https://www.npmjs.com/package/@loradb/lora-node) | `npm install @loradb/lora-node` | [Node →](./node) |
| **Python** | [![PyPI](https://img.shields.io/pypi/v/lora-python?label=pypi&logo=pypi&logoColor=white)](https://pypi.org/project/lora-python/) | `pip install lora-python` | [Python →](./python) |
| **Browser / WASM** | [![npm](https://img.shields.io/npm/v/@loradb/lora-wasm?label=%40loradb%2Flora-wasm&logo=npm)](https://www.npmjs.com/package/@loradb/lora-wasm) | `npm install @loradb/lora-wasm` | [WASM →](./wasm) |
| **Go** | [pkg.go.dev](https://pkg.go.dev/github.com/lora-db/lora/crates/bindings/lora-go) | `go get github.com/lora-db/lora/crates/bindings/lora-go` | [Go →](./go) |
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
| Evaluate from a shell or another language | HTTP server |

All bindings share the same query surface and result shape — the
Cypher is identical, only the host-language wrapper differs.

### Rust and HTTP server

Two more paths share the same Cypher surface:

- [**Rust crate**](./rust) — embed `lora-database` directly in a
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

<QueryCodeBlock code={String.raw`CREATE (:Person {name: 'Ada'})`} />

<QueryCodeBlock code={String.raw`MATCH (p:Person) RETURN p.name`} />

In any binding that's two `execute` calls; the platform guide shows
the language-specific syntax.

## Examples

### Shared value model

Typed values follow one contract (defined in
`crates/bindings/shared-ts/types.ts`): primitives, lists/maps, graph entities
(tagged `{kind: "node" | "relationship" | "path"}`), temporals
(tagged `{kind: "date" | "datetime" | ...}`), and points (tagged
`{kind: "point", srid, crs, ...}`).

See [**Data Types Overview**](../data-types/overview) for the full
catalogue and each binding's parameters section for how host values
map in.

## Common Patterns

### One process, one graph

Each binding defaults to **one process, one in-memory graph**.
Auto-commit reads on the same handle can overlap on snapshots; write commits
and explicit read-write transactions serialize. Spawn multiple `Database`
instances only when you intentionally want separate graphs or archives.

If you want persistence, opt into it explicitly:

- On `lora-node`, pass a database name and database directory to `createDatabase('app', { databaseDir: './data' })`.
- On `lora-node`, use `openWalDatabase({ walDir: './data/wal' })` for an explicit WAL directory.
- On Rust / `lora-server`, configure a WAL directory.
- On Python, Go, and Ruby, pass a database name plus their `database_dir` /
  `DatabaseDir` option for `.loradb` archives, or use `open_wal` /
  `OpenWal` for an explicit WAL directory.

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

- **Persistence depends on the binding.** Point-in-time snapshots via
  `save_snapshot` / `load_snapshot` exist on every binding
  (byte-based on WASM). Continuous durability via WAL exists on every
  filesystem-backed binding: Rust, `lora-node`, Python, Go, Ruby, and
  `lora-server`. WASM remains snapshot-only.
  See [Limitations → Storage](../limitations#storage).
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
