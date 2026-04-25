---
title: HTTP API Reference
sidebar_label: HTTP API
description: Endpoint reference for lora-server — the Axum-based HTTP wrapper around the LoraDB engine. Query shapes, response formats, health checks, snapshots, checkpoints, and WAL admin routes.
---

# HTTP API Reference

Endpoint-by-endpoint reference for `lora-server` — the Axum-based
HTTP wrapper around the engine. Reach for this page when you're
calling LoraDB over the wire from a stack without an in-process
binding, or when you want to poke at the engine from `curl`. One
process serves exactly one graph, optionally paired with snapshots and
WAL-backed recovery.

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
| `POST` | [`/admin/checkpoint`](#post-admincheckpoint-opt-in) | Write a checkpoint snapshot (opt-in; only when `--wal-dir` is set) |
| `POST` | [`/admin/wal/status`](#post-adminwalstatus-opt-in) | Inspect WAL state (opt-in; only when `--wal-dir` is set) |
| `POST` | [`/admin/wal/truncate`](#post-adminwaltruncate-opt-in) | Truncate safe WAL history (opt-in; only when `--wal-dir` is set) |

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

The admin surface is split in two:

- `POST /admin/snapshot/save` and `POST /admin/snapshot/load` mount only
  when `lora-server` starts with `--snapshot-path <PATH>`.
- `POST /admin/checkpoint`, `POST /admin/wal/status`, and
  `POST /admin/wal/truncate` mount only when `lora-server` starts with
  `--wal-dir <DIR>`.

The two flags are independent. You can run snapshot admin without WAL,
WAL admin without snapshot save/load, or both together. See the
[HTTP server quickstart](../getting-started/server#snapshots-wal-and-restore),
the canonical [Snapshots guide](../snapshot), and
[WAL and checkpoints](../wal).

:::caution Security

The admin endpoints have **no authentication**, and the optional `path`
body field is passed straight to the OS — any client that can reach the
admin port can write files anywhere the server UID can write, or swap
the live graph by pointing `load` at an attacker-staged file. The same
warning applies to `/admin/checkpoint`. Do not expose them on an
untrusted network. See [Limitations → HTTP server](../limitations#http-server).

:::

### Snapshot save / load request body

Both endpoints accept the same optional JSON body:

```json
{ "path": "/custom/location/snapshot.bin" }
```

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Override the server's default `--snapshot-path` for this request only. Omit the body (or omit `path`) to use the configured default. |

`content-type: application/json` is required when sending a body.

### Snapshot save / load response (success)

`200 OK`, body is a `SnapshotMeta`:

```json
{
  "formatVersion": 1,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": null,
  "path": "/var/lib/lora/db.bin"
}
```

| Field | Type | Description |
|---|---|---|
| `formatVersion` | number | The snapshot file format version (currently `1`). |
| `nodeCount` | number | Nodes in the saved / loaded graph. |
| `relationshipCount` | number | Relationships in the saved / loaded graph. |
| `walLsn` | number or null | `null` for a pure snapshot; non-`null` for a checkpoint snapshot written with WAL enabled. |
| `path` | string | Filesystem path the server actually used. |

### Snapshot save / load response (error)

- `500 Internal Server Error` — path cannot be read / written, file is corrupt, permissions fail, parent directory is missing, or the save / load itself errors.
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

## `POST /admin/checkpoint` (opt-in)

Mounted only when the server starts with `--wal-dir <DIR>`. The route
does not require `/admin/snapshot/save` or `/admin/snapshot/load` to be
mounted.

### Request body

Optional JSON body with a conditional field:

```json
{ "path": "/var/lib/lora/checkpoint.bin" }
```

| Field | Type | Required | Description |
|---|---|---|---|
| `path` | string | conditional | Target snapshot path for the checkpoint. Required unless the server was started with `--snapshot-path`. |

If `--snapshot-path` is configured, omitting the body uses that path as
the default checkpoint target. Without `--snapshot-path`, the body must
include `path` or the route returns `400 Bad Request`.

### Response (success)

`200 OK`, body matches the snapshot response shape:

```json
{
  "formatVersion": 1,
  "nodeCount": 1024,
  "relationshipCount": 4096,
  "walLsn": 4815,
  "path": "/var/lib/lora/checkpoint.bin"
}
```

The important difference is `walLsn`: a checkpoint stamps the snapshot
with the WAL's durable fence. For checkpoint-heavy deployments, use
`--wal-sync-mode per-commit` unless you are intentionally managing
group-mode fsync lag.

### Examples

```bash
# WAL-only server: pass a checkpoint path explicitly.
curl -sX POST http://127.0.0.1:4747/admin/checkpoint \
  -H 'content-type: application/json' \
  -d '{"path": "/var/lib/lora/checkpoint.bin"}'

# Server started with --snapshot-path: the body can be omitted.
curl -sX POST http://127.0.0.1:4747/admin/checkpoint
```

### Response (error)

- `400 Bad Request` — no `path` in the request body and no
  `--snapshot-path` configured on the server.
- `404 Not Found` — the server was not started with `--wal-dir`.
- `500 Internal Server Error` — the checkpoint write itself failed.

## `POST /admin/wal/status` (opt-in)

Mounted only when the server starts with `--wal-dir <DIR>`.

### Request

No body.

### Response (success)

`200 OK`:

```json
{
  "durableLsn": 4815,
  "nextLsn": 4820,
  "activeSegmentId": 3,
  "oldestSegmentId": 2,
  "bgFailure": null
}
```

| Field | Type | Description |
|---|---|---|
| `durableLsn` | number | Highest LSN known durable on disk. In `none` sync mode, this is only a logical checkpoint fence. |
| `nextLsn` | number | Next LSN the WAL will allocate. |
| `activeSegmentId` | number | Numeric id of the segment currently accepting appends. |
| `oldestSegmentId` | number | Numeric id of the oldest retained segment. |
| `bgFailure` | string or null | Latched background fsync failure, populated when group mode goes unhealthy. |

### Response (error)

- `404 Not Found` — the server was not started with `--wal-dir`.
- `500 Internal Server Error` — WAL status could not be read.

## `POST /admin/wal/truncate` (opt-in)

Mounted only when the server starts with `--wal-dir <DIR>`.

### Request body

Optional JSON body:

```json
{ "fenceLsn": 4815 }
```

If omitted, the server truncates up to the WAL's current `durableLsn`.
Only sealed segments are removed; the active segment and the segment
immediately before it are retained.

### Response (success)

`204 No Content`

### Response (error)

- `404 Not Found` — the server was not started with `--wal-dir`.
- `500 Internal Server Error` — truncation failed or WAL status could
  not be read for the default fence.

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

Relevant durability flags:

- `--snapshot-path <PATH>` / `LORA_SERVER_SNAPSHOT_PATH`
- `--restore-from <PATH>`
- `--wal-dir <DIR>` / `LORA_SERVER_WAL_DIR`
- `--wal-sync-mode <MODE>` / `LORA_SERVER_WAL_SYNC_MODE`

## What isn't here

- **Authentication — not supported.** Bind to `127.0.0.1` or put the
  server behind a reverse proxy.
- **TLS — not supported.** Terminate at a proxy.
- **Rate limiting — not supported.**
- **Parameters — not yet supported.** See
  [Limitations → Parameters](../limitations#parameters).
- **Multi-database — not supported.** One process, one graph. Run
  multiple processes on different ports for isolation.
- **HTTP auth / TLS on the admin surface — not supported.** Snapshot
  and WAL admin routes are opt-in, but still unauthenticated when
  enabled.

## See also

- [HTTP server quickstart](../getting-started/server) — install, run, embed.
- [HTTP server quickstart → Snapshots, WAL, and restore](../getting-started/server#snapshots-wal-and-restore) — flag reference for the admin endpoints.
- [WAL and checkpoints](../wal) — recovery model, sync modes, and route semantics.
- [Result formats](../concepts/result-formats) — what each `format` looks like.
- [Queries → Parameters](../queries/parameters) — typed parameter binding (via in-process bindings today).
- [Troubleshooting → Snapshots](../troubleshooting#snapshots) — 404 on admin routes, malformed files, version mismatches.
- [Troubleshooting → WAL and checkpoints](../troubleshooting#wal-and-checkpoints) — 404, checkpoint path errors, poisoned WALs.
- [Troubleshooting → Server](../troubleshooting#server) — port conflicts, 400s.
- [Limitations → HTTP server](../limitations#http-server).
