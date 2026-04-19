---
title: What is LoraDB
sidebar_label: What is LoraDB
slug: /
---

# What is LoraDB

LoraDB is an in-memory **graph database** with a **Cypher-like query
engine**, written in Rust. Small, embeddable, easy to reason about.

## What you get

- **Labeled property graph** — nodes with labels, typed relationships,
  properties on both. See [Graph Model](./concepts/graph-model).
- **Cypher-like queries** — [`MATCH`](./queries/match),
  [`CREATE`](./queries/create), [`WHERE`](./queries/where),
  [`WITH`](./queries/return-with#with),
  [`MERGE`](./queries/unwind-merge#merge),
  [shortest paths](./queries/paths#shortest-paths),
  [aggregation](./queries/aggregation).
- **Three primary bindings over one Rust engine** — Node/TypeScript,
  Python, WebAssembly.
- **An HTTP server** if you'd rather hit it with `curl`.

## Pick your platform

| Platform | Guide |
|---|---|
| **Node.js / TypeScript** — native binding | [Node →](./getting-started/node) |
| **Python** — sync and asyncio | [Python →](./getting-started/python) |
| **Browser / WASM** — in-process or Web Worker | [WASM →](./getting-started/wasm) |

### Other runtimes

| Platform | Guide |
|---|---|
| **Rust** — embed the crate directly | [Rust →](./getting-started/rust) |
| **HTTP server** — `POST /query` | [Server →](./getting-started/server) |

## New here?

1. Skim the [**Graph model**](./concepts/graph-model) — 2 min.
2. Follow your language's guide — 5 min.
3. Run the [**Ten-Minute Tour**](./getting-started/tutorial) —
   create → match → filter → aggregate → paths → CASE.
4. Keep [**Query Examples**](./queries/examples) open as a cheatsheet.

## Documentation map

| Section | Contents |
|---|---|
| [**Concepts**](./concepts/graph-model) | Graph model, nodes, relationships, properties |
| [**Getting Started**](./getting-started/installation) | Install, tutorial, per-language guides |
| [**Queries**](./queries/) | Clause reference: MATCH, WHERE, RETURN, aggregation, paths |
| [**Functions**](./functions/overview) | String, math, list, temporal, spatial, aggregation |
| [**Data Types**](./data-types/overview) | Scalars, lists, maps, temporals, spatial points |
| [**Cookbook**](./cookbook) | Scenario-driven recipes: social, e-commerce, events, geo |
| [**Troubleshooting**](./troubleshooting) | Common errors and fixes |
| [**Limitations**](./limitations) | What isn't supported yet |

Stuck? See [**Troubleshooting**](./troubleshooting). For what isn't
supported yet, see [**Limitations**](./limitations).
