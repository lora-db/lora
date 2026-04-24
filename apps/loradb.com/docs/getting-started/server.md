---
title: Running LoraDB as an HTTP Server
sidebar_label: HTTP Server
description: Run LoraDB as an HTTP service with lora-server — a small Axum wrapper around the engine for curl probing, polyglot stacks, and demos. One process, one in-memory graph.
---

# Running LoraDB as an HTTP Server

## Overview

`lora-server` wraps the Rust engine in a small Axum HTTP server —
useful for probing the engine with `curl`, serving a polyglot stack,
or running demos. One process serves exactly one in-memory graph.

## Installation / Setup

### Install

```bash
cargo install --path crates/lora-server
```

Or, inside the workspace:

```bash
cargo run --release -p lora-server
```

### Configure

```bash
lora-server                          # 127.0.0.1:4747
lora-server --host 0.0.0.0 --port 8080
LORA_SERVER_HOST=0.0.0.0 LORA_SERVER_PORT=8080 lora-server
```

Precedence (first match wins): CLI flags → environment variables →
built-in defaults (`127.0.0.1:4747`).

Every flag also has an env-var equivalent:

| Flag | Env var | Default | Description |
|---|---|---|---|
| `--host <ADDR>` | `LORA_SERVER_HOST` | `127.0.0.1` | Bind address. |
| `--port <PORT>` | `LORA_SERVER_PORT` | `4747` | Bind port. |
| `--snapshot-path <PATH>` | `LORA_SERVER_SNAPSHOT_PATH` | unset | Default file for the admin snapshot endpoints. **Also gates whether they are mounted** — unset = 404. |
| `--restore-from <PATH>` | — | unset | Load a snapshot at boot, before accepting queries. |

### Snapshots and restore

LoraDB can save the live graph to a single file and restore it at
boot or on demand:

- **`--snapshot-path <PATH>`** (or `LORA_SERVER_SNAPSHOT_PATH`)
  enables the admin endpoints
  [`POST /admin/snapshot/save`](../api/http#admin-endpoints-opt-in)
  and `POST /admin/snapshot/load`, and supplies the default file
  they operate on. If unset, the admin routes return `404`.
- **`--restore-from <PATH>`** loads a snapshot at startup. A missing
  file is fine — the server starts with an empty graph and logs a
  message. A malformed file is fatal.

Typical cron-friendly setup — boot from, and save back to, the same
file:

```bash
lora-server \
  --host 127.0.0.1 --port 4747 \
  --snapshot-path /var/lib/lora/db.bin \
  --restore-from  /var/lib/lora/db.bin
```

Save on demand:

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save
# => {"formatVersion":1,"nodeCount":1024,"relationshipCount":4096,"walLsn":null}
```

Or save to an ad-hoc override path for a single call:

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save \
  -H 'content-type: application/json' \
  -d '{"path":"/var/backups/lora/2026-04-24.bin"}'
```

Load (restores on top of the live graph — serialises against every
other query):

```bash
curl -sX POST http://127.0.0.1:4747/admin/snapshot/load
```

`--restore-from` is independent of `--snapshot-path`. You can restore
from a read-only seed (`/var/lib/lora/seed.bin`) and snapshot to a
writable path (`/var/lib/lora/runtime.bin`).

:::caution Security

The admin endpoints have **no authentication** and the optional `path`
body field is passed straight to the OS — anyone who can reach the
admin port can write files anywhere the server UID can write, or swap
the live graph by pointing `load` at an attacker-staged file. See
[Limitations → HTTP server](../limitations#http-server) and the
[HTTP API reference](../api/http#admin-endpoints-opt-in) before
exposing them.

:::

See also the canonical [Snapshots guide](../snapshot) for the
metadata shape, file format, and every binding's save / load API.

## Creating a Client / Connection

The client is any HTTP client. Verify the server is alive before
sending queries:

```bash
curl http://127.0.0.1:4747/health
# { "status": "ok" }
```

## Running Your First Query

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"CREATE (:Person {name: \"Ada\"})"}'
```

Then read it back:

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"MATCH (p:Person) RETURN p.name AS name","format":"rows"}'
```

## Examples

### Minimal working example with `curl`

Shown above. Two `POST /query` calls.

### Parameterised query

:::caution

`POST /query` does **not** currently accept a `params` body field —
see [Limitations → Parameters](../limitations#parameters).
Interpolate constants safely into the query string yourself, or use
the Rust API. HTTP parameters are on the roadmap.

:::

Safe-enough pattern — build the literal server-side when the values
are trusted and fully encoded:

```bash
NAME='Ada'
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  --data-binary "$(jq -n --arg q "MATCH (p:Person {name: '$NAME'}) RETURN p" '{query:$q}')"
```

For anything user-supplied, run against the [Rust binding](./rust)
with real parameters and expose a narrower API on top.

### Structured result handling with `jq`

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query":"MATCH (p:Person) RETURN p.name AS name","format":"rows"}' \
  | jq '.rows[].name'
```

### Node client example

```ts
async function runQuery(query: string) {
  const res = await fetch('http://127.0.0.1:4747/query', {
    method:  'POST',
    headers: { 'content-type': 'application/json' },
    body:    JSON.stringify({ query, format: 'rows' }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error ?? `http ${res.status}`);
  }
  return res.json() as Promise<{ columns: string[]; rows: any[] }>;
}

const { rows } = await runQuery('MATCH (p:Person) RETURN count(*) AS n');
console.log(rows[0].n);
```

### Handle errors

HTTP status codes:

| Status | Meaning |
|---|---|
| `200` | Query executed successfully; body is a `QueryResult` |
| `400` | Parse / semantic / runtime error; body is `{ "error": "…" }` |

```json
{ "error": "parse error: expected ')' at position 17" }
```

Handle both explicitly; never assume `200` on a mis-typed query.

### Embedding in a larger Axum app

`lora-server` is also a library — embed it in a larger Axum
application, or run several processes on different ports for
isolation:

```rust
use std::sync::Arc;
use lora_database::Database;
use lora_server::build_app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = Arc::new(Database::in_memory());
    let app = build_app(Arc::clone(&db));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4747").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

Mount `build_app(db)` under any sub-path, combine it with your own
routes, add middleware — it's a standard Axum `Router`.

## Endpoints

### `GET /health`

Liveness check.

```bash
curl http://127.0.0.1:4747/health
# { "status": "ok" }
```

### `POST /query`

Request body:

```json
{
  "query":  "MATCH (n) RETURN n",
  "format": "rowArrays"
}
```

- `query` — Cypher string (required).
- `format` — one of `"rows"`, `"rowArrays"`, `"graph"`, `"combined"`
  (optional; defaults to `"graph"`). See
  [Result formats](../concepts/result-formats) for the full shape of each.

### `POST /admin/snapshot/save` (opt-in)

### `POST /admin/snapshot/load` (opt-in)

Both are mounted only when the server is started with
`--snapshot-path <PATH>` (or `LORA_SERVER_SNAPSHOT_PATH`). Otherwise
they return `404`. See [Snapshots and restore](#snapshots-and-restore)
above, and the full reference in
[HTTP API → Admin endpoints (opt-in)](../api/http#admin-endpoints-opt-in).

## Common Patterns

### Seed via stdin

```bash
cat seed.cypher | while IFS= read -r q; do
  curl -s http://127.0.0.1:4747/query \
    -H 'content-type: application/json' \
    --data-binary "$(jq -n --arg q "$q" '{query:$q}')" > /dev/null
done
```

Where `seed.cypher` has one Cypher statement per line.

### Health check script

```bash
status=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:4747/health)
[ "$status" = 200 ] && echo 'ok' || echo 'down'
```

### Embedding with custom routes

```rust
use axum::routing::get;
use std::sync::Arc;
use lora_database::Database;
use lora_server::build_app;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = Arc::new(Database::in_memory());
    let app = build_app(Arc::clone(&db))
        .route("/version", get(|| async { "loradb custom" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4747").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

### Multiple graphs

One process serves exactly one graph. Run multiple processes on
different ports and put a reverse proxy in front when you need
isolation.

## Error Handling

| Symptom | Likely cause | Fix |
|---|---|---|
| `Address already in use` | Port held by another process | See [Troubleshooting → Server](../troubleshooting#server) |
| `400` on every request | Missing `content-type: application/json` | Add the header |
| Silent empty rows | Query targets a label that doesn't exist yet | Seed before reading |

## What's _not_ here

- **Authentication, TLS, rate limiting** — none. Bind to
  `127.0.0.1` or put it behind a reverse proxy. The admin snapshot
  endpoints also ship without auth — see
  [Snapshots and restore](#snapshots-and-restore).
- **Parameter binding over HTTP** — the `/query` body does **not**
  currently accept a `params` field. Bind via the Rust API today;
  HTTP params are on the roadmap. See
  [Limitations](../limitations).
- **WAL / continuous persistence** — not supported. Point-in-time
  snapshots are available via [Snapshots and restore](#snapshots-and-restore);
  data between saves is lost on crash.
- **Multiple databases** — one process serves exactly one graph.
  Run multiple processes on different ports if you need isolation.

## Performance / Best Practices

- Put the server behind a reverse proxy (nginx, Caddy, Traefik) for
  TLS and rate limiting — the built-in server has none.
- Bind to `127.0.0.1` unless you control the network.
- For a polyglot stack, embed `build_app(db)` into a larger Axum
  process rather than running a separate `lora-server`.

## See also

- [**HTTP API reference**](../api/http) — endpoint-by-endpoint reference.
- [**HTTP API → Admin endpoints (opt-in)**](../api/http#admin-endpoints-opt-in) — full reference for the snapshot endpoints.
- [**Snapshots guide**](../snapshot) — canonical feature page: metadata shape, file format, every binding's API.
- [**Result formats**](../concepts/result-formats) — the four response shapes.
- [**Rust guide**](./rust) — native API (what the server wraps).
- [**Queries**](../queries/) — the query language the server exposes.
- [**Cookbook**](../cookbook) — scenario-based recipes, including backup-and-restore.
- [**Limitations → HTTP server**](../limitations#http-server) —
  auth, TLS, parameters.
- [**Troubleshooting → Snapshots**](../troubleshooting#snapshots) — 404, malformed file, version mismatch.
- [**Troubleshooting → Server**](../troubleshooting#server) — port
  conflicts, connection issues.
