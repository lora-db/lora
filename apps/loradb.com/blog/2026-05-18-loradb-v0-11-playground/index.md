---
slug: loradb-v0-11-playground
title: "LoraDB v0.11: Cypher in your browser"
description: "LoraDB v0.11 launches play.loradb.com — an in-browser IDE that runs the same Cypher engine, planner, and executor as the crate, compiled to WASM, with a graph canvas, schema browser, snapshots, and shareable query URLs. No server, no install."
authors: [loradb]
tags: [release-notes, announcement, playground, wasm, tooling]
image: /img/blog/loradb-v0-11-playground-header.png
---

![LoraDB v0.11 — Cypher in your browser.](/img/blog/loradb-v0-11-playground-header.png)

LoraDB v0.11 is a surface release.

v0.5 made the engine stream. v0.6 made persistence feel like a system.
v0.7 was a process release. v0.8 made the planner and executor
observable. v0.9 gave the planner a real schema catalog. v0.10 made
the function library a library.

v0.11 puts a face on the engine. [`play.loradb.com`](https://play.loradb.com)
is a browser IDE that runs the same parser, planner, executor, and
storage you'd run from the crate — compiled to WASM, hosted as flat
files, with no server in the loop.

<!-- truncate -->

## What the playground is

Open a tab. Write Cypher. See the graph.

The playground is a Next.js app that loads three of LoraDB's published
client packages and wires them into a Dockview workbench:

- [`@loradb/lora-wasm`](https://www.npmjs.com/package/@loradb/lora-wasm) — the
  full Rust engine compiled to WebAssembly, running in a Web Worker.
- [`@loradb/lora-query`](https://www.npmjs.com/package/@loradb/lora-query) — a
  Monaco-based Cypher editor with LoraDB-aware tokens, completion, and
  diagnostics.
- [`@loradb/lora-graph-canvas`](https://www.npmjs.com/package/@loradb/lora-graph-canvas) — a
  React graph canvas that renders query results as nodes and edges
  with selection, hover, and labels.

Around those three packages the app adds tabs, a results grid, a
schema browser, history, saved queries, snapshots, a command spotlight,
hot-keys, and a share-by-URL flow. Everything lives in IndexedDB and
`localStorage`; nothing leaves the tab.

## Same engine, not a fork

The playground does not re-implement Cypher in the browser. It loads
the same `lora-database` crate the Rust, Node, Python, Ruby, and Go
bindings use, with `wasm-pack` as the build step:

- Same parser. The grammar from `crates/lora-parser` is the one
  parsing your query.
- Same analyzer. `BUILTIN_SPECS` from v0.10 resolves the same 236
  function signatures.
- Same planner and optimizer. The catalog-backed scans from v0.9
  light up the same way.
- Same executor. Rows stream out of the same physical operators.
- Same storage. Snapshots round-trip through the same codec the
  filesystem bindings use.

If a query works in the playground, it works in your service — or it
doesn't work in either. That property is the point: the playground is
a debugger you reach by URL, not a different surface to learn.

## Four panes, one workbench

The default layout splits the window four ways: editor, result,
sidebar, and inspector. Every pane is a Dockview panel — drag it,
detach it, resize it.

**Editor.** Cypher syntax highlighting from `lora-query`, multi-tab,
Cmd-Enter to run. Errors render with squiggles at the analyzer's
reported span. Format on save snaps statements onto canonical
indentation.

**Result.** Four views over the same result set, switchable per tab:

- _Graph_ — `lora-graph-canvas` renders nodes and edges with labels
  and labels-of-relationships. Click a node to pin it in the inspector;
  drag to lay out the canvas.
- _Table_ — a result grid sorted by column. Booleans, vectors, maps,
  durations, and points all render with their tagged shape.
- _JSON_ — the raw row payload, useful when the table view's truncation
  makes a structure hard to read.
- _Plan_ — `EXPLAIN` / `PROFILE` output as a tree, with rows-in,
  rows-out, and timing per operator. Same data the v0.8 binding tests
  pin.

**Sidebar.** Five panels stacked: history, saved queries, schema
browser (labels, types, indexes, constraints — read from the v0.9
catalog), snapshots, settings.

**Inspector.** A drawer that opens on a clicked node or relationship.
Properties, labels, IDs, and the same tagged values you'd see across a
binding.

## Sharing a query is a URL

Every query is encodable in a hash fragment. Hit "Share" and the
playground writes the current Cypher into the URL hash, base64-encoded
with a compact codec.

That makes the playground a primitive a documentation page can lean
on: a `?#q=...` link in a doc, a PR comment, or a Discord message
opens the playground with the query loaded and run. No screenshots.
No reproduction steps. No "paste this into the REPL".

The shape encodes the query, the tab title, and the result view. It
does not encode the user's database — IndexedDB is origin-scoped and
local to each visitor — so a shared link runs the query against
_their_ data, not yours. If you want the query and a seed graph
together, the playground also ships snapshot export and import:

1. Run the seed `CREATE` statements.
2. Sidebar → Snapshots → New snapshot.
3. Share both the snapshot file and the query URL.

The same snapshot codec the filesystem bindings use writes the file,
so a v0.11 playground snapshot opens cleanly from Node, Python, or
Rust.

## What runs where

The playground is a fully static export. There are no API routes, no
server actions, no edge functions. `next build` writes flat HTML, JS,
CSS, and WASM into `apps/play.loradb.com/out`, and Cloudflare Pages
serves it.

That has three useful consequences:

- **No backend to share with you.** The engine is in your tab. There is
  no shared database, no rate limit, no multi-tenant queue. Your
  queries run on your machine.
- **No account.** There is no sign-in. There is also no sync — your
  graph lives in your browser, and clearing site data clears it.
- **Honest scale.** WASM is faster than people expect, but it is still
  a single tab. Build a million-node graph in the browser and you'll
  feel it. The playground is for shape, not load.

A graph that needs to survive a tab refresh, span machines, or back a
production system still wants the Node, Python, or Rust binding — or
the HTTP server.

## What the playground does not do (yet)

Per the snapshot-release framing we've kept since v0.3, the playground
is honest about its boundaries.

- **No host-side parameter binding.** The examples in
  [the cookbook](/docs/queries/examples) use inline literals. In
  application code, `$name` and `$city` go through your binding's
  `params` argument; the in-browser editor does not yet expose a
  parameters drawer.
- **No multi-database.** One database per origin. Open the playground
  in two tabs and you get two views of the same graph; open it in a
  private window and you get an empty one.
- **No editing while a query runs.** The Web Worker is single-engine
  per tab. A long `MATCH` blocks the next query until it completes or
  is cancelled.
- **No remote import.** You can paste Cypher, import a snapshot file,
  or hit `Run`. The playground will not fetch a remote URL to seed
  your database — that path stays out of the trust boundary.
- **Hash-encoded share URLs only.** State lives in the URL hash so a
  static export can refresh cleanly. Path-based deep links are not
  supported yet.

## New on npm

Two packages graduated to public npm in this cycle:

- [`@loradb/lora-query`](https://www.npmjs.com/package/@loradb/lora-query)
  ships the Cypher editor as a standalone React component with its own
  themed Monaco language and worker, so you can drop it into any
  documentation site, blog, or internal tool.
- [`@loradb/lora-graph-canvas`](https://www.npmjs.com/package/@loradb/lora-graph-canvas)
  ships the graph canvas as a React component with deletion guard,
  reduced-motion hooks, and a small tool API for selection.

Both are MIT-licensed under the docs-site terms and re-export
themable defaults plus a CSS entrypoint. The playground depends on
them through yarn workspaces; downstream apps install them like any
React package.

The crates, server, and bindings stay under BSL 1.1 with a three-year
change date and an Apache 2.0 change license.

## How v0.11 fits the journey

v0.5 streamed. v0.6 persisted. v0.7 was a process release. v0.8 made
the planner and executor observable. v0.9 added the schema catalog.
v0.10 made the function library a library.

v0.11 makes the engine reachable in a tab. The Cypher you type in the
playground is the Cypher your service runs. The plan you see is the
plan your service plans. The result is the result.

That changes what a documentation page can ask of a reader. Instead of
"clone the repo, install the toolchain, open a REPL, paste this
query", a page can link to a playground URL and let the reader watch
the engine answer.

The next steps are obvious from here: a parameters drawer for the
editor, a way to seed a snapshot directly from a doc link, and a
hosted scratch-pad that survives a browser switch when the user opts
in.

## Read next

- [Open the playground](https://play.loradb.com)
- [Playground page on loradb.com](/playground)
- [Cookbook — queries that run as-is](/docs/cookbook)
- [WASM binding guide](/docs/getting-started/wasm)
- [Limitations](/docs/limitations)

v0.11 is the release where LoraDB stops being something you have to
install before you can read it.
