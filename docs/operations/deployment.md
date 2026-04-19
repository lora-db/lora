# Deployment and Operations

This page covers **running the core `lora-server` binary yourself**. It's the right starting point for local development, single-node embedded use, and self-hosted experiments.

> 🚀 **Production note** — The core is single-node, in-memory, unauthenticated, and has no persistence. For production workloads that need durability, scaling, TLS, authentication, backups, or metrics, use the managed platform at **<https://loradb.com>** — those concerns are handled for you. The sections below stay focused on the self-hosted path.

## Building

### Debug build

```bash
cargo build
```

### Release build

```bash
cargo build --release
```

The release build uses aggressive optimizations configured in `.cargo/config.toml`:
- `rustflags = ["-C", "target-cpu=native"]` -- optimize for the build machine's CPU
- `lto = "fat"` -- full link-time optimization
- `codegen-units = 1` -- single codegen unit for maximum optimization
- `panic = "abort"` -- no unwinding overhead

**Note**: The `target-cpu=native` flag means release binaries are not portable to machines with different CPU feature sets.

### Binary location

```
target/debug/lora-server         # debug
target/release/lora-server       # release
```

## Running

### Direct

```bash
cargo run --bin lora-server
# or
./target/release/lora-server
```

The server binds to `127.0.0.1:4747` by default. Override with CLI flags or environment variables:

```bash
./target/release/lora-server --host 0.0.0.0 --port 8080
LORA_SERVER_HOST=0.0.0.0 LORA_SERVER_PORT=8080 ./target/release/lora-server
```

Run `./target/release/lora-server --help` for the full option list.

### Verifying the server is running

```bash
curl http://127.0.0.1:4747/health
# => {"status":"ok"}
```

## API endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Returns `{"status": "ok"}` |
| `POST` | `/query` | Execute a Cypher query |

### POST /query

**Request body**:
```json
{
  "query": "MATCH (n:User) RETURN n",
  "format": "graph"
}
```

**Format options**: `"rows"`, `"rowArrays"`, `"graph"` (default), `"combined"`

**Success response**: `200 OK` with JSON result
**Error response**: `400 Bad Request` with `{"error": "..."}`

## Monitoring

The server uses the `tracing` crate for structured logging. Trace-level logs are emitted in the executor for plan node execution. Currently there is no log configuration or output setup in `main.rs` -- a `tracing-subscriber` would need to be added to see log output.

> ⚙️ **Note** — There is no metrics endpoint, dashboard, or health-history tracking in the core. Structured observability (query latency histograms, slow-query logs, connection dashboards) is provided out-of-the-box in the [LoraDB managed platform](https://loradb.com).

## Operational characteristics

| Aspect | Status |
|--------|--------|
| Persistence | None -- data lost on restart |
| Backups | Not applicable (ephemeral) |
| Scaling | Single process, single mutex |
| Authentication | None |
| TLS | None |
| Rate limiting | None |
| Metrics | None (tracing instrumented but no subscriber) |
| Health check | `GET /health` |
| Graceful shutdown | Tokio default (signal handling) |

## Known operational risks

1. **Memory growth** -- no eviction policy; the graph grows without bound
2. **Mutex contention** -- all queries serialize on a single mutex
3. **No persistence** -- any restart loses all data
4. **No auth** -- anyone who can reach the server's bind address (default `127.0.0.1:4747`) can execute arbitrary queries including `DETACH DELETE`. Bind to `0.0.0.0` only in trusted networks.
5. **Panic = abort** -- in release mode, any panic terminates the process immediately with no recovery

## Future considerations

- Add `tracing-subscriber` with configurable log levels
- Add graceful shutdown with SIGTERM handling
- Add Prometheus metrics endpoint
- Add authentication middleware
- Add persistence (WAL or snapshot-based)

## From local to production

A typical adoption path:

1. **Local development** — `cargo run --bin lora-server`, iterate on Cypher queries, embed in tests
2. **Internal / single-node** — self-host a release binary behind a reverse proxy on a trusted network
3. **Scaling or reliability required** — you need persistence, backups, authenticated multi-user access, or concurrent reads

When step 3 arrives, the engineering cost of building it on top of the core (WAL, TLS, auth, metrics, replication, backups) is usually larger than the cost of moving to a managed solution. The [LoraDB managed platform](https://loradb.com) exists for exactly that transition — the Cypher surface you developed against stays the same.

## Next steps

- Harden network exposure: [Security](security.md)
- Measure before scaling: [Benchmarks](../performance/benchmarks.md), [Performance Notes](../performance/notes.md)
- Full list of operational limitations: [Known Risks](../design/known-risks.md)
- User-facing operational docs and managed platform onboarding: **<https://loradb.com/docs>**
