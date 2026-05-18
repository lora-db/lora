---
title: Try LoraDB in the Playground
sidebar_label: Playground
description: Use play.loradb.com to write LoraDB queries in the browser, inspect graph/table/JSON results, save local snapshots, and share query links without installing a binding.
---

# Try LoraDB in the Playground

The fastest way to try LoraDB is
[play.loradb.com](https://play.loradb.com). It runs LoraDB through
WebAssembly in your browser tab, stores playground state locally, and
needs no account or server.

Use it for learning the query surface, reducing a bug report, drafting
examples, or checking a small graph shape before moving the query into
Node, Python, WASM, Go, Ruby, Rust, or the HTTP server.

## What you get

| Surface | What it does |
|---|---|
| Editor | Query tabs, LoraDB-aware highlighting, completion, diagnostics, formatting, and `Cmd/Ctrl+Enter` to run |
| Results | Graph, table, JSON, and parser/analyzer views over the active query |
| Sidebar | Saved queries, schema browser, snapshots, history, and settings |
| Local persistence | IndexedDB and `localStorage` keep saved queries, snapshots, settings, history, and the auto-restored graph on the browser origin |
| Share links | Copy a URL with the active query encoded in the `#q=` hash |

## Run a first query

Open [play.loradb.com](https://play.loradb.com), paste this seed query,
and run it:

<QueryCodeBlock code={String.raw`CREATE (:Person {name: 'Ada', city: 'London'});
CREATE (:Person {name: 'Grace', city: 'New York'});
CREATE (:Person {name: 'Linus', city: 'Helsinki'});
MATCH (a:Person {name: 'Ada'}), (g:Person {name: 'Grace'}), (l:Person {name: 'Linus'})
CREATE (a)-[:KNOWS {since: 1843}]->(g);
MATCH (g:Person {name: 'Grace'}), (l:Person {name: 'Linus'})
CREATE (g)-[:KNOWS {since: 1991}]->(l);`} />

Then run:

<QueryCodeBlock code={String.raw`MATCH (p:Person)-[r:KNOWS]->(friend:Person)
RETURN p, r, friend`} />

The graph result tab shows the people and relationships. The table and
JSON result tabs show the same rows in more compact forms.

## Share a query

The Share action copies a URL that carries only the query body. It does
not include your database, saved queries, snapshots, or settings.

For a reproducible example with data:

1. Run the seed query.
2. Open Snapshots and create a snapshot.
3. Export the snapshot file.
4. Share the snapshot file and the query link together.

The query URL uses the hash fragment (`#q=...`) so the static export can
refresh cleanly. Very large queries can produce long URLs; when in
doubt, share a snapshot and keep the query short.

## Use literals in playground examples

Application code should use [parameters](../queries/parameters). The
playground editor can show parameter names in the analyzer view, but it
does not yet have a params drawer for supplying host-side values.

For playground-ready docs and examples, use trusted inline literals:

<QueryCodeBlock code={String.raw`MATCH (p:Person {name: 'Ada'})
RETURN p`} />

When you move the query into an application binding, switch to
parameters:

```ts
await db.execute(
  "MATCH (p:Person {name: $name}) RETURN p",
  { name: "Ada" },
);
```

## Boundaries

- The playground runs in one browser tab and is best for small local
  graphs, examples, and debugging.
- The hosted app has no account, no sync, and no shared database.
- Clearing browser site data clears local playground state.
- The Cancel button drops a pending result from the UI, but the
  underlying WASM call currently runs until it returns.
- Full `explain` / `profile` plans are available through bindings and
  HTTP endpoints. The playground's Plan tab is an editor analysis view.
- WASM is snapshot-only. For WAL-backed durability, use a filesystem
  binding or the HTTP server.

## Move from playground to code

The query text moves directly into a binding; only host values and
persistence setup change.

| Next step | Guide |
|---|---|
| Run the query in a Node or TypeScript app | [Node](./node) |
| Run it from Python | [Python](./python) |
| Embed the same WASM package yourself | [Browser (WASM)](./wasm) |
| Put it behind a local HTTP process | [HTTP server](./server) |
| Keep a durable filesystem-backed graph | [WAL and checkpoints](../wal) |

## See also

- [**Cookbook**](../cookbook) — scenario-driven queries.
- [**Query examples**](../queries/examples) — compact examples that run as-is.
- [**Limitations**](../limitations#browser-playground) — current playground gaps.
- [**WASM binding**](./wasm) — build your own browser or Worker integration.
