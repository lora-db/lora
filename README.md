# LoraDB

**An in-memory graph database with full Cypher support, written from scratch in Rust.**

Lora parses, analyzes, compiles, and executes Cypher queries against a property-graph store that lives in process memory, exposed over an HTTP/JSON API. The entire pipeline — PEG parser, semantic analyzer, logical and physical planners, executor, and storage — is implemented in this workspace, with no external query engine.

Reach for Lora when you want a fast, embeddable graph engine for local development, tests, notebooks, prototypes, or graph-shaped workloads that don't need a clustered database behind them.

## Quickstart

```bash
# Clone and run (requires Rust stable — pinned via rust-toolchain.toml)
git clone <this-repo> lora && cd lora
cargo run --bin lora-server
# => Lora server running at http://127.0.0.1:4747
```

Create nodes, link them, query the graph:

```bash
# Create two users and a relationship
curl -s localhost:4747/query -H 'Content-Type: application/json' \
  -d '{"query": "CREATE (a:User {name: \"Alice\"}), (b:User {name: \"Bob\"}), (a)-[:FOLLOWS {since: 2024}]->(b)"}'

# Query the graph
curl -s localhost:4747/query -H 'Content-Type: application/json' \
  -d '{"query": "MATCH (a)-[r:FOLLOWS]->(b) RETURN a.name AS follower, b.name AS followee, r.since"}'
```

Prefer a binary? Download a platform build from the [Releases page](../../releases) — see [Releases](#releases) below.

## Why developers use Lora

- **Local-first** — a single in-process engine or a single static binary; no cluster, no daemon zoo, no config files
- **Zero-setup Cypher** — full read/write Cypher with no schema migrations; labels, relationship types, and properties are created on the fly
- **Fast** — millions of nodes/sec on scans and projections (see [Performance Benchmarks](#performance-benchmarks))
- **Embeddable** — use it as a Rust crate, over HTTP, or from Node.js, Python, and WebAssembly bindings
- **Transparent pipeline** — every stage (parse → analyze → compile → execute) is plain, inspectable Rust; easy to read, easy to extend
- **Batteries included** — temporal and spatial types, 60+ built-in functions, variable-length paths, `shortestPath`, aggregations, expressions, list/pattern comprehensions

## Key capabilities

- **Full Cypher pipeline** — parse (PEG) → analyze → compile (logical + physical) → execute
- **Property-graph model** — nodes with labels, relationships with a single type, properties on both
- **Read queries** — `MATCH`, `OPTIONAL MATCH`, `WHERE`, `RETURN`, `WITH`, `ORDER BY`, `SKIP`, `LIMIT`, `DISTINCT`, `UNWIND`, `UNION` / `UNION ALL`
- **Write queries** — `CREATE`, `SET`, `MERGE` (with `ON MATCH` / `ON CREATE`), `DELETE`, `DETACH DELETE`, `REMOVE`
- **Expressions** — arithmetic, boolean logic, comparison, string operators (`STARTS WITH`, `ENDS WITH`, `CONTAINS`), `IN`, `IS NULL / IS NOT NULL`, `CASE`, regex (`=~`), list / pattern comprehension, `REDUCE`, `EXISTS` subqueries, map projection
- **Variable-length paths** — `[:TYPE*1..3]`, unbounded `*`, zero-hop, cycle avoidance, path binding, `shortestPath()`, `allShortestPaths()`
- **Aggregation** — `count`, `sum`, `avg`, `min`, `max`, `collect`, `stdev`, `stdevp`, `percentileCont`, `percentileDisc`; grouped aggregation and `WITH`-based HAVING
- **Temporal types** — `Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`, `Duration`; arithmetic, truncation, `duration.between`
- **Spatial types** — 2D and 3D `Point` values in Cartesian (SRID 7203 / 9157) and WGS-84 geographic (SRID 4326 / 4979) reference systems; `point()` constructor with CRS/SRID inference and validation; `distance()` with Euclidean (2D + 3D) and Haversine on WGS-84 (height ignored — see limitations)
- **60+ built-in functions** — string, math (incl. full trigonometry), list, type conversion, entity introspection, path, temporal, spatial (see [docs/functions.md](docs/functions.md))
- **Parameter binding** — `$name` and `$1` parameters with typed values (string, int, float, bool, list, map, temporal, spatial) via the Rust API (`Database::execute_with_params`)
- **Result formats** — `rows`, `rowArrays`, `graph`, `combined`
- **HTTP server** — Axum-based JSON API (`POST /query`, `GET /health`)
- **Query optimizer** — filter push-down below projections (extensible)
- **Semantic analysis** — variable scoping, label / type / property validation against live graph state in read contexts, unknown-function detection

## Using Lora

### Over HTTP

```bash
# Execute a query
curl -s http://127.0.0.1:4747/query \
  -H 'Content-Type: application/json' \
  -d '{"query": "CREATE (n:User {name: \"Alice\", age: 32}) RETURN n"}'

# Health check
curl http://127.0.0.1:4747/health
# => {"status":"ok"}
```

Choose a result format via the `"format"` field in the request body. Options: `"rows"`, `"rowArrays"`, `"graph"` (default), `"combined"`.

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'Content-Type: application/json' \
  -d '{"query": "MATCH (n:User) RETURN n", "format": "combined"}'
```

### Embedded (Rust API)

```rust
use lora_database::Database;

let db = Database::in_memory();
db.execute("CREATE (:User {name: 'Alice'})", None)?;

let result = db.execute("MATCH (n:User) RETURN n.name", None)?;
```

With parameters:

```rust
use std::collections::BTreeMap;
use lora_database::{Database, LoraValue};

let mut params = BTreeMap::new();
params.insert("minAge".into(), LoraValue::Int(18));

let db = Database::in_memory();
let result = db.execute_with_params(
    "MATCH (n:User) WHERE n.age >= $minAge RETURN n",
    None,
    params,
)?;
```

Parameter binding is currently **only available through the Rust API** — the HTTP server does not yet forward a `params` body field.

### Other bindings

- **Node.js / TypeScript** — `crates/lora-node` (napi-rs), with shared types in `crates/shared-ts`
- **Python** — `crates/lora-python` (PyO3 / maturin)
- **WebAssembly** — `crates/lora-wasm`

## Graph data model

Lora implements the **property graph model**:

- **Nodes** have an auto-incremented `NodeId` (`u64`), zero or more labels, and a property map
- **Relationships** have an auto-incremented `RelationshipId` (`u64`), a source node, a destination node, exactly one type, and a property map
- **Properties** are `BTreeMap<String, PropertyValue>` where values can be null, bool, int, float, string, list, map, any temporal type, or a spatial point

See [docs/schema-and-entities.md](docs/schema-and-entities.md) and [docs/graph-architecture.md](docs/graph-architecture.md).

## Running `lora-server`

The server binds to `127.0.0.1:4747` by default and logs the resolved address to stdout. Because the default host is `127.0.0.1`, it is only reachable from the local machine. Use `--host 0.0.0.0` (or `::`) to listen on all interfaces.

### Configuration

CLI flags take precedence over environment variables, which take precedence over the built-in defaults.

| Option          | Env var               | Default       | Description                          |
| --------------- | --------------------- | ------------- | ------------------------------------ |
| `--host <HOST>` | `LORA_SERVER_HOST`    | `127.0.0.1`   | Bind address (IPv4, IPv6, or name).  |
| `--port <PORT>` | `LORA_SERVER_PORT`    | `4747`        | TCP port (1–65535, or 0 for any).    |
| `--help`        | —                     | —             | Print help and exit.                 |
| `--version`     | —                     | —             | Print version and exit.              |

Port `0` asks the OS for an ephemeral port; the resolved address is printed on startup.

### Examples

```bash
# Defaults
./lora-server

# Listen on all interfaces, custom port
./lora-server --host 0.0.0.0 --port 8080

# Same via environment
LORA_SERVER_HOST=0.0.0.0 LORA_SERVER_PORT=8080 ./lora-server

# Built from source
cargo run --bin lora-server -- --host ::1 --port 8080
```

On Windows (PowerShell):

```powershell
.\lora-server.exe --host 0.0.0.0 --port 8080
# or
$env:LORA_SERVER_HOST = "0.0.0.0"
$env:LORA_SERVER_PORT = "8080"
.\lora-server.exe
```

Logs go to stdout; fatal errors to stderr. There is no log file — pipe the process output if you need persistence.

## Architecture

### Repository layout

```
lora/
  Cargo.toml                  # workspace root
  rust-toolchain.toml         # stable toolchain with rustfmt + clippy
  crates/
    lora-ast/                 # AST type definitions (Span, Document, Expr, Pattern, ...)
    lora-parser/              # PEG grammar (pest) + AST lowering
    lora-store/               # Storage traits + BTreeMap-backed in-memory store, temporal and spatial value types
    lora-analyzer/            # Semantic analysis, variable resolution, scope management
    lora-compiler/            # Logical plan, optimizer, physical plan
    lora-executor/            # Physical plan interpreter, expression evaluator, result projection
    lora-database/            # Orchestration: Database entry point that owns the store and drives the pipeline
    lora-server/              # Axum HTTP server, QueryRunner wiring
    lora-wasm/                # WebAssembly bindings
    lora-node/                # Node.js / TypeScript bindings (napi-rs)
    lora-python/              # Python bindings (PyO3 / maturin)
    shared-ts/                # Canonical TypeScript types shared by lora-node and lora-wasm
  docs/                       # Architecture and developer documentation
```

### Crate dependency graph

```
lora-ast
  ^
lora-parser ───────────> lora-ast
  ^
lora-store ────────────> lora-ast (temporal/spatial types)
  ^
lora-analyzer ─────────> lora-ast, lora-store, lora-parser
  ^
lora-compiler ─────────> lora-ast, lora-analyzer
  ^
lora-executor ─────────> lora-ast, lora-analyzer, lora-compiler, lora-store
  ^
lora-database ─────────> lora-parser, lora-analyzer, lora-compiler, lora-executor, lora-store
  ^
lora-server ───────────> lora-database
```

### Where Cypher queries live

- **PEG grammar** — `crates/lora-parser/src/cypher.pest`
- **AST types** — `crates/lora-ast/src/ast.rs`
- **Parser** — `crates/lora-parser/src/parser.rs`
- **Integration tests** — `crates/lora-database/tests/` (one file per feature area)
- **HTTP integration tests** — `crates/lora-server/tests/http.rs`

## Building, testing, contributing

```bash
cargo build
cargo test --workspace          # 1698 passing, 0 failing, 58 ignored
cargo clippy --workspace
cargo fmt --all --check
```

### Adding a new node label or relationship type

No schema migration needed — labels and relationship types are created dynamically:

```cypher
CREATE (n:NewLabel {prop: "value"})
MATCH (a:User), (b:User) CREATE (a)-[:NEW_REL_TYPE]->(b)
```

The analyzer validates labels and types against the current graph state for `MATCH` queries, but allows any label or type in `CREATE` and `MERGE`.

### Adding support for a new Cypher clause

See [docs/lora-development-guide.md](docs/lora-development-guide.md). The typical flow is:

1. Add AST node in `lora-ast/src/ast.rs`
2. Add grammar rule in `lora-parser/src/cypher.pest`
3. Add parser lowering in `lora-parser/src/parser.rs`
4. Add semantic analysis in `lora-analyzer/src/analyzer.rs` and resolved IR in `lora-analyzer/src/resolved.rs`
5. Add logical and physical plan nodes in `lora-compiler/`
6. Implement execution in `lora-executor/src/executor.rs`
7. Add tests in `lora-database/tests/`

## Performance benchmarks

Numbers below come from `cargo bench` (Criterion) on the `engine_benchmarks`, `advanced_benchmarks`, and `scale_benchmarks` binaries under `crates/lora-database/benches/`. Run on Apple Silicon (`aarch64-apple-darwin`) in release mode on 2026-04-17.

- **Simple scan, 1 000 nodes** — ~3 500 000 nodes/sec projection, ~16 800 000 nodes/sec with `count(*)` only
- **Full scan, 50 000 nodes** — ~7 400 000 nodes/sec (6.78 ms per query)
- **Single-hop traversal, 1 000 edges** — ~1 900 000 edges/sec (chain), ~3 800 000 edges/sec (star)
- **Grouped aggregation, 1 000 rows** — ~3 300 000 rows/sec (`GROUP BY`), ~2 200 000 rows/sec (4× aggregators)
- **Sort, 1 000 rows single key** — ~1 170 000 rows/sec
- **Single-entity writes** — ~85 000 – 160 000 ops/sec; batch `CREATE` via `UNWIND` — ~900 000 nodes/sec
- **Parse-only simple `MATCH`** — ~282 000 parses/sec; full parse+compile+execute — ~124 000 queries/sec
- **Realistic 500-person friend-of-friend** — ~2 500 queries/sec (399 µs per query)

Reproduce:

```bash
cargo bench --bench engine_benchmarks
cargo bench --bench advanced_benchmarks
cargo bench --bench scale_benchmarks
cargo bench --bench temporal_spatial_benchmarks
```

See [docs/benchmarking.md](docs/performance-benchmarks.md) for more.

## Releases

Pushing a version tag like `v0.1.0` triggers the release workflow, which builds `lora-server` for Windows, Linux, and macOS and attaches every package as a **GitHub Release asset**. Download the binary for your platform from the [Releases page](../../releases).

| Platform              | Target triple                 | Release asset                                                |
| --------------------- | ----------------------------- | ------------------------------------------------------------ |
| Linux x86_64          | `x86_64-unknown-linux-gnu`    | `lora-server-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz`         |
| Windows x86_64        | `x86_64-pc-windows-msvc`      | `lora-server-vX.Y.Z-x86_64-pc-windows-msvc.zip`              |
| macOS Intel (x86_64)  | `x86_64-apple-darwin`         | `lora-server-vX.Y.Z-x86_64-apple-darwin.tar.gz`              |
| macOS Apple Silicon   | `aarch64-apple-darwin`        | `lora-server-vX.Y.Z-aarch64-apple-darwin.tar.gz`             |

Each archive ships with a matching `.sha256` file, and every release also attaches an aggregated `lora-server-vX.Y.Z-SHA256SUMS.txt` covering all archives. See [RELEASING.md](RELEASING.md) for the full naming scheme and recovery steps.

### Download and run

1. Download the asset for your platform from the **Assets** section of the latest [release](../../releases), e.g. `lora-server-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`.
2. Verify the checksum:
   ```bash
   # Linux — single archive
   sha256sum -c lora-server-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
   # Linux — everything at once via the aggregated file
   sha256sum --ignore-missing -c lora-server-v0.1.0-SHA256SUMS.txt
   # macOS
   shasum -a 256 -c lora-server-v0.1.0-x86_64-apple-darwin.tar.gz.sha256
   # Windows (PowerShell)
   (Get-FileHash .\lora-server-v0.1.0-x86_64-pc-windows-msvc.zip -Algorithm SHA256).Hash.ToLower()
   ```
3. Extract the archive. Each archive contains a single top-level directory named after the archive (without the extension) holding the `lora-server` binary plus `README.md` and `RELEASING.md`.
4. Run the binary — see [Running `lora-server`](#running-lora-server).

## Known limitations

- **In-memory only** — all data is lost when the process exits
- **Global mutex** — queries serialize; no read concurrency
- **No `CALL` / procedures** — parsed but rejected by the analyzer
- **No `FOREACH`** — not in the grammar
- **No `CREATE INDEX` / `CREATE CONSTRAINT`** — no DDL
- **No persistence** — no WAL, snapshots, or disk storage
- **No authentication or TLS** — the HTTP server has no auth layer; bind to `0.0.0.0` only in trusted networks
- **No transactions** — each query holds the mutex for its duration
- **No HTTP parameter forwarding** — `execute_with_params` works in the Rust API only
- **Property filters are scans** — no property indexes
- **ASCII-only case conversion** — `toLower()` / `toUpper()` use ASCII rules; `normalize()` is a placeholder
- **Spatial `Point`** — no WKT parsing / output, no CRS transformation between SRIDs, no bounding-box predicates (`point.withinBBox`)
- **`distance()` on WGS-84-3D ignores `height`** — the returned value is the great-circle surface distance; a full 3D geodesic (ellipsoid + altitude) is not implemented

See [docs/known-gaps-and-risks.md](docs/known-gaps-and-risks.md) for the full list and [docs/lora-support-matrix.md](docs/lora-support-matrix.md) for the feature matrix.

## Documentation

| Document | Description |
|----------|-------------|
| [docs/INDEX.md](docs/INDEX.md) | Documentation index |
| [docs/lora-support-matrix.md](docs/lora-support-matrix.md) | Feature support matrix with test evidence |
| [docs/functions.md](docs/functions.md) | Complete function reference |
| [docs/architecture-overview.md](docs/architecture-overview.md) | System architecture and pipeline |
| [docs/graph-architecture.md](docs/graph-architecture.md) | In-memory graph engine design |
| [docs/schema-and-entities.md](docs/schema-and-entities.md) | Property graph model and data types |
| [docs/lora-development-guide.md](docs/lora-development-guide.md) | How to add Cypher features |
| [docs/query-patterns.md](docs/query-patterns.md) | Supported query patterns with examples |
| [docs/data-flow.md](docs/data-flow.md) | Query execution pipeline |
| [docs/testing-strategy.md](docs/testing-strategy.md) | Testing approach and coverage |
| [docs/known-gaps-and-risks.md](docs/known-gaps-and-risks.md) | Limitations and open questions |
| [docs/glossary.md](docs/glossary.md) | Terminology reference |

## Usage model

At a high level:

- **You can** use the core for development, testing, evaluation, internal business use, and internal production systems
- **You can** embed it in your own applications, services, and tooling
- **You can't** offer Lora as database-as-a-service, a hosted API for third parties, a managed database platform, or a competing hosted resale offering

For full terms and plain-English guidance, read [License Usage](docs/license/usage.md) and [License Strategy](docs/license/strategy.md).

## License

The LoraDB core database engine in this repository is licensed under the [Business Source License 1.1](LICENSE).

You may use the core for development, testing, evaluation, internal business use, and internal production systems. You may not use the core to offer LoraDB as database-as-a-service, a hosted API for third parties, a managed database platform, or a competing hosted resale offering.

Each covered release converts to the Apache License 2.0 on the Change Date listed in the root license. For this release policy, the Change Date is April 19, 2029.

The `apps/loradb.com` documentation website is a separate MIT-licensed exception. See [apps/loradb.com/LICENSE](apps/loradb.com/LICENSE).
