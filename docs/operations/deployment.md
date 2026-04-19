# Deployment and Operations

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
