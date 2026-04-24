---
title: HTTP API Reference
sidebar_label: HTTP API
description: Endpoint reference for lora-server — the Axum-based HTTP wrapper around the LoraDB engine. Query shapes, response formats, health checks, and error codes.
---

# HTTP API Reference

Endpoint-by-endpoint reference for `lora-server` — the Axum-based HTTP
wrapper around the engine. Use this page when you're calling LoraDB
over the wire from a non-Rust/Node/Python stack, or when you want to
poke at the engine from `curl`. One process serves exactly one
in-memory graph.

For an install-and-run walkthrough (how to start the server, set host
and port, embed it in a larger Axum app), see the
[HTTP server quickstart](../getting-started/server).

## Endpoints at a glance

| Method | Path | Purpose |
|---|---|---|
| `GET` | [`/health`](#get-health) | Liveness probe |
| `POST` | [`/query`](#post-query) | Run a Cypher query |

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

See [Result formats](../concepts/result-formats) for how to choose.

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

- **Authentication** — none. Bind to `127.0.0.1` or put the server
  behind a reverse proxy.
- **TLS** — none. Terminate at a proxy.
- **Rate limiting** — none.
- **Parameters** — not yet. See
  [Limitations → Parameters](../limitations#parameters).
- **Multi-database** — one process, one graph. Run multiple processes
  on different ports for isolation.
- **Persistence** — all state is in-memory; lost on restart.

## See also

- [HTTP server quickstart](../getting-started/server) — install, run, embed.
- [Result formats](../concepts/result-formats) — what each `format` looks like.
- [Queries → Parameters](../queries/parameters) — typed parameter binding (via embedded bindings today).
- [Troubleshooting → Server](../troubleshooting#server) — port conflicts, 400s.
- [Limitations → HTTP server](../limitations#http-server).
