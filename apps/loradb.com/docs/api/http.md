---
title: HTTP API Reference
sidebar_label: HTTP API
description: Endpoint reference for lora-server — the Axum-based HTTP wrapper around the LoraDB engine. Query shapes, response formats, health checks, and error codes.
---

# HTTP API Reference

Endpoint-by-endpoint reference for `lora-server` — the Axum-based
HTTP wrapper around the engine. Reach for this page when you're
calling LoraDB over the wire from a stack without an in-process
binding, or when you want to poke at the engine from `curl`. One
process serves exactly one in-memory graph.

For an install-and-run walkthrough (how to start the server, set
host and port, embed it in a larger Axum app), see the
[HTTP server quickstart](../getting-started/server).

## Endpoints at a glance

| Method | Path | Purpose |
|---|---|---|
| `GET` | [`/health`](#get-health) | Liveness probe |
| `POST` | [`/query`](#post-query) | Run a Cypher query |
| `POST` | [`/admin/snapshot/save`](#admin-endpoints-opt-in) | Save a snapshot (opt-in; only when `--snapshot-path` is set) |
| `POST` | [`/admin/snapshot/load`](#admin-endpoints-opt-in) | Restore a snapshot (opt-in; only when `--snapshot-path` is set) |

Anything else returns `404`.

## `GET /health`

Returns `200 OK` if the process is alive.

### Request

```http
GET /health HTTP/1.1
```

### Response

```json
{ "status": "ok" }
```

## `POST /query`

Run a single Cypher statement (or multi-statement document) and get
a structured result back.

### Request body

```json
{
  "query":  "MATCH (p:Person) RETURN p",
  "format": "rows"
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Cypher source. |
| `format` | string | no | One of `"rows"`, `"rowArrays"`, `"graph"`, `"combined"`. Defaults to `"graph"`. |

:::caution

The request body does **not** yet accept a `params` field. Bind
parameters via the [Rust](../getting-started/rust#parameterised-query),
[Node](../getting-started/node#parameterised-query),
[Python](../getting-started/python#parameterised-query), or
[WASM](../getting-started/wasm#parameterised-query) bindings. See
[Limitations → Parameters](../limitations#parameters).

:::

`content-type: application/json` is required. Anything else yields
`400`.

### Response (success)

`200 OK`, body is a JSON object whose shape depends on `format`. See
[Result formats](../concepts/result-formats) for each shape in detail.

Quick reference:

| `format` | Body shape |
|---|---|
| `rows` | `{ "rows": [ { col: value, ... } ] }` |
| `rowArrays` | `{ "columns": [...], "rows": [[...], ...] }` |
| `graph` | `{ "graph": { "nodes": [...], "relationships": [...] } }` |
| `combined` | `{ "columns": [...], "data": [...], "graph": {...} }` |

### Response (error)

`400 Bad Request`, body is:

```json
{ "error": "parse error: expected ')' at position 17" }
```

The `error` string is the engine's underlying message. Parse,
semantic, and runtime errors all return `400` — distinguish by the
text if you need to, or treat "any non-2xx is a query error" and log
the message.

## Admin endpoints (opt-in)

`POST /admin/snapshot/save` and `POST /admin/snapshot/load` let a client persist the live graph to disk and restore it later. They are **opt-in**: both endpoints return `404` unless `lora-server` is started with `--snapshot-path <PATH>` (or the `LORA_SERVER_SNAPSHOT_PATH` env var). See the [HTTP server quickstart → Snapshots and restore](../getting-started/server#snapshots-and-restore) and the canonical [Snapshots guide](../snapshot) for the feature overview and every binding's equivalent API.

:::caution Security

The admin endpoints have **no authentication**, and the optional `path` body field is passed straight to the OS — any client that can reach the admin port can write files anywhere the server UID can write, or swap the live graph by pointing `load` at an attacker-staged file. Do not expose them on an untrusted network. See [Limitations → HTTP server](../limitations#http-server).

:::

### Request body

Both endpoints accept the same optional JSON body:

```json
{ "path": "/custom/location/snapshot.bin" }
```

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Override the server's default `--snapshot-path` for this request only. Omit the body (or omit `path`) to use the configured default. |

`content-type: application/json` is required when sending a body.

### Response (success)

`200 OK`, body is a `SnapshotMeta`:

```json
{
  "formatVersion": 1,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null
}
```

| Field | Type | Description |
|---|---|---|
| `formatVersion` | number | The snapshot file format version (currently `1`). |
| `nodeCount` | number | Nodes in the saved / loaded graph. |
| `relationshipCount` | number | Relationships in the saved / loaded graph. |
| `walLsn` | number or null | Reserved for a future WAL / checkpoint hybrid. Always `null` today. |

### Response (error)

- `400 Bad Request` — malformed JSON, or the path cannot be read/written (permissions, missing parent directory, corrupt file for `load`).
- `404 Not Found` — the server was not started with `--snapshot-path`, so the admin routes are not mounted.

Error bodies match `/query`'s shape:

```json
{ "error": "snapshot load failed: bad magic" }
```

### Examples

```bash
# Save to the configured default path
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save

# Save to an override path
curl -sX POST http://127.0.0.1:4747/admin/snapshot/save \
  -H 'content-type: application/json' \
  -d '{"path": "/var/backups/lora/2026-04-24.bin"}'

# Load from the configured default path
curl -sX POST http://127.0.0.1:4747/admin/snapshot/load
```

## Examples

### Minimal round-trip

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query": "CREATE (:Person {name: \"Ada\"})"}'

curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query": "MATCH (p:Person) RETURN p.name AS name", "format": "rows"}'
```

The first call writes a node; its body is
`{"graph": {"nodes": [...], "relationships": []}}` because the
engine default is `graph` and `CREATE` contributes the new node.
The second call asks for `rows` explicitly and returns
`{"rows": [{"name": "Ada"}]}` — one row per match, keyed by the
`AS` alias.

### Choose a result format

```bash
# Column-indexed (smallest payload for wide result sets)
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query": "MATCH (p:Person) RETURN p.name, p.born",
       "format": "rowArrays"}'

# Graph (nodes + edges, de-duplicated)
curl -s http://127.0.0.1:4747/query \
  -H 'content-type: application/json' \
  -d '{"query": "MATCH (a)-[r]->(b) RETURN a, r, b",
       "format": "graph"}'
```

`rowArrays` comes back as `{"columns": [...], "rows": [[...], ...]}`
— one `columns` list plus one tuple per row, so the column keys
aren't repeated. `graph` returns
`{"graph": {"nodes": [...], "relationships": [...]}}` with each
entity listed once even when many rows reference it — ideal for
visualisers. See [Result formats](../concepts/result-formats) for
the full shape of each.

### Node client

```ts
async function runQuery(query: string, format = 'rows') {
  const res = await fetch('http://127.0.0.1:4747/query', {
    method:  'POST',
    headers: { 'content-type': 'application/json' },
    body:    JSON.stringify({ query, format }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error ?? `http ${res.status}`);
  }
  return res.json();
}
```

## Configuration

`lora-server` takes its bind address from:

1. CLI flags: `--host`, `--port`.
2. Env vars: `LORA_SERVER_HOST`, `LORA_SERVER_PORT`.
3. Defaults: `127.0.0.1:4747`.

```bash
lora-server --host 0.0.0.0 --port 8080
LORA_SERVER_HOST=0.0.0.0 LORA_SERVER_PORT=8080 lora-server
```

Full walkthrough in the [HTTP server quickstart](../getting-started/server#configure).

## What isn't here

- **Authentication — not supported.** Bind to `127.0.0.1` or put the
  server behind a reverse proxy.
- **TLS — not supported.** Terminate at a proxy.
- **Rate limiting — not supported.**
- **Parameters — not yet supported.** See
  [Limitations → Parameters](../limitations#parameters).
- **Multi-database — not supported.** One process, one graph. Run
  multiple processes on different ports for isolation.
- **WAL / continuous persistence — not supported.** Point-in-time
  snapshots are available through the [admin endpoints](#admin-endpoints-opt-in)
  when opt-in; data between saves is lost on crash.

## See also

- [HTTP server quickstart](../getting-started/server) — install, run, embed.
- [HTTP server quickstart → Snapshots and restore](../getting-started/server#snapshots-and-restore) — flag reference for the admin endpoints.
- [Result formats](../concepts/result-formats) — what each `format` looks like.
- [Queries → Parameters](../queries/parameters) — typed parameter binding (via in-process bindings today).
- [Troubleshooting → Snapshots](../troubleshooting#snapshots) — 404 on admin routes, malformed files, version mismatches.
- [Troubleshooting → Server](../troubleshooting#server) — port conflicts, 400s.
- [Limitations → HTTP server](../limitations#http-server).
