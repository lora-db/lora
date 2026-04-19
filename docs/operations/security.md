# Security and Data Sensitivity

## Current security posture

Lora is designed for local development and experimentation. It has **no security controls**.

| Control | Status |
|---------|--------|
| Authentication | None |
| Authorization | None |
| TLS / HTTPS | None |
| Input validation | Cypher parser rejects invalid syntax; no size limits |
| Rate limiting | None |
| Audit logging | None |
| Encryption at rest | Not applicable (in-memory only) |

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

### Dependency supply chain

The project depends on well-known Rust crates (`axum`, `tokio`, `pest`, `serde`, etc.). No unusual or high-risk dependencies are observed. Regular `cargo audit` checks are recommended.

## Recommendations

### Before any network exposure

1. Add authentication (API key, JWT, or basic auth)
2. Add TLS termination (either in the server or behind a reverse proxy)
3. Add query size limits
4. Add rate limiting
5. Add result size limits

### Before handling sensitive data

6. Add authorization (per-query permissions)
7. Add audit logging
8. Add encryption in transit (TLS)
9. Consider property-level access control

### General hygiene

10. Run `cargo audit` regularly
11. Do not commit `.env` files with secrets
