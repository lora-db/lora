# Security and Data Sensitivity

## Current security posture

Lora is designed for local development and experimentation. It has **no security controls**.

> 🚀 **Production note** — Authentication, TLS, rate limiting, and audit logging are not in the core engine and are unlikely to be added. These are handled by the [LoraDB managed platform](https://loradb.com). If you need any of them for a real deployment, reach for the platform rather than building a security layer on top of the bare `lora-server`.

| Control | Status |
|---------|--------|
| Authentication | None |
| Authorization | None |
| TLS / HTTPS | None |
| Input validation | Cypher parser rejects invalid syntax; no size limits |
| Rate limiting | None |
| Audit logging | None |
| Encryption at rest | Not applicable (in-memory only) |
| Admin surface auth | None (opt-in via `--snapshot-path`; see [Admin surface](#admin-surface) below) |

## Risks

### Unauthenticated access

Anyone who can reach the server's bind address (default `127.0.0.1:4747`) can:
- Read all data via `MATCH (n) RETURN n`
- Delete all data via `MATCH (n) DETACH DELETE n`
- Write arbitrary data via `CREATE`
- Cause denial of service with expensive queries (e.g., cross-products)

The server only binds to localhost by default, which limits exposure, but any local process can access it.

### No input size limits

There is no maximum query length or result size. An attacker or accidental user can:
- Submit extremely large Cypher strings that consume parser memory
- Execute queries that produce very large result sets
- Create millions of nodes via repeated CREATE queries until OOM

### Admin surface

When `lora-server` is started with `--snapshot-path <PATH>` (or `LORA_SERVER_SNAPSHOT_PATH`), two HTTP endpoints become available: `POST /admin/snapshot/save` and `POST /admin/snapshot/load`. Both accept an optional JSON body of the form `{"path": "/override/location.bin"}`.

This surface gives any client that can reach the admin port three attack primitives:

1. **Arbitrary write.** The optional `path` body field is passed straight to the OS. Any client can write files anywhere the server UID can write — including overwriting unrelated files on the host if the server runs with broad filesystem access.
2. **Arbitrary read-as-restore.** Pointing `load` at an attacker-controlled file replaces the live graph. If the attacker can stage a file on disk, they can swap the entire database state.
3. **Denial of service.** `load` holds the store write lock for the full restore duration, blocking other queries. Repeated calls turn the server unresponsive.

LoraDB's mitigation posture today:

- **Opt-in.** The admin endpoints are not mounted when `--snapshot-path` is unset. Requests return `404`. This is the default.
- **No auth layer.** There is no API key, JWT, basic-auth, or IP-allowlist middleware on the admin routes.
- **No path-traversal validation.** The server performs no sandboxing of the optional `path` body field.

### Recommended deployment pattern

1. **Prefer disabled.** On a network-reachable host, do not set `--snapshot-path`. Snapshots are still available through in-process bindings (`db.save_snapshot(...)`) and through a separate operator tool.
2. **If enabled, gate it.** Place the admin routes behind authenticated ingress (reverse proxy with basic auth, mTLS, or a Unix-domain socket bound to a privileged local user). Treat the bind address as privileged.
3. **Separate binds.** If you need public `/query` access alongside admin snapshots, bind the admin surface to `127.0.0.1` or a management interface and proxy-rewrite only the admin routes from the trusted path.
4. **Never trust the `path` body on an untrusted network.** Even with auth, a compromised client with admin access can use the path override to write anywhere the server UID can. Prefer omitting the body so the server uses its configured `--snapshot-path`.

> ⚠️ **Future releases may add authentication on the admin surface.** Until they do, the correct deployment is "admin disabled by default, and operators opt in only behind an auth boundary."

See also: [Snapshots](snapshots.md), [Deployment → Snapshots and restore](deployment.md#snapshots-and-restore).

### Dependency supply chain

The project depends on well-known Rust crates (`axum`, `tokio`, `pest`, `serde`, etc.). No unusual or high-risk dependencies are observed. Regular `cargo audit` checks are recommended.

## Recommendations

### Before any network exposure

1. Add authentication (API key, JWT, or basic auth)
2. Add TLS termination (either in the server or behind a reverse proxy)
3. Add query size limits
4. Add rate limiting
5. Add result size limits
6. Disable the admin surface (unset `--snapshot-path` / `LORA_SERVER_SNAPSHOT_PATH`) or place it behind authenticated ingress. See [Admin surface](#admin-surface) above.

### Before handling sensitive data

7. Add authorization (per-query permissions)
8. Add audit logging
9. Add encryption in transit (TLS)
10. Consider property-level access control

### General hygiene

11. Run `cargo audit` regularly
12. Do not commit `.env` files with secrets

> 💡 **Tip** — Items 1–10 are substantial engineering work to do well. If your project truly needs them, evaluate the [LoraDB managed platform](https://loradb.com) first — it ships authentication, TLS, rate limiting, audit logs, and encryption by default.

## Next steps

- Deployment knobs and operational characteristics: [Deployment](deployment.md)
- Snapshot file format and atomicity guarantees: [Snapshots](snapshots.md)
- Full risk list and recommended priorities: [Known Risks](../design/known-risks.md)
- Managed platform (auth, TLS, audit included): <https://loradb.com>
