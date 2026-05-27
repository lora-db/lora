<div align="center">

<a href="https://loradb.com">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://loradb.com/logos/loradb-mark-dark.svg">
    <img alt="LoraDB" src="https://loradb.com/logos/loradb-mark.svg" width="96" height="96">
  </picture>
</a>

# LoraDB

**The graph database for connected systems.**

An in-process graph store with a Cypher-like query engine — small enough to embed in an agent, a robot, or a stream processor.

<p>
  <a href="https://github.com/lora-db/lora/actions/workflows/workspace-quality.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/lora-db/lora/workspace-quality.yml?branch=main&label=CI&logo=github"></a>
  <a href="https://github.com/lora-db/lora/actions/workflows/workspace-quality.yml"><img alt="Tests" src="https://img.shields.io/github/actions/workflow/status/lora-db/lora/workspace-quality.yml?branch=main&label=tests&logo=rust"></a>
  <a href="https://github.com/lora-db/lora/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/lora-db/lora?label=release&color=5b8def"></a>
  <a href="https://crates.io/crates/lora-database"><img alt="crates.io" src="https://img.shields.io/crates/v/lora-database?label=crates.io&logo=rust"></a>
  <a href="https://www.npmjs.com/package/@loradb/lora-node"><img alt="npm" src="https://img.shields.io/npm/v/@loradb/lora-node?label=npm&logo=npm"></a>
  <a href="https://pypi.org/project/lora-python/"><img alt="PyPI" src="https://img.shields.io/pypi/v/lora-python?label=pypi&logo=pypi&logoColor=white"></a>
  <a href="LICENSE"><img alt="License: BUSL-1.1" src="https://img.shields.io/badge/license-BUSL--1.1-blue"></a>
  <a href="https://loradb.com"><img alt="Docs" src="https://img.shields.io/badge/docs-loradb.com-5b8def"></a>
</p>

<sub>Embedded · Cypher-like &nbsp;·&nbsp; Rust · Node · Python · WASM · Go · Ruby · HTTP &nbsp;·&nbsp; Zero daemons · runs in your process &nbsp;·&nbsp; Source-available · readable end-to-end</sub>

</div>

---

## Overview

LoraDB is an embeddable property-graph database written in Rust. It parses, analyzes, compiles, and executes a Cypher-like query language against an in-process graph store - with no daemons, no clusters, and no schema migrations. Optional `CREATE INDEX` / `DROP INDEX` DDL lets you declare secondary indexes for hot predicates without changing the schema-free data model. Optional snapshots, WAL directories, and named `.loradb` archives provide local durability on filesystem-backed surfaces. `VECTOR` is a first-class value type, so embeddings live next to the graph they describe.

The graph belongs inside your process. Reach for LoraDB when you're building:

- **AI agents & LLM pipelines** — context, memory, and tool graphs that live with the agent, with embeddings and similarity search on the same nodes
- **Robotics & scene graphs** — local reasoning over typed relationships
- **Event pipelines & streams** — graph-shaped state inside a stream processor
- **Real-time reasoning** — read/write Cypher without standing up a database server
- **Embedded graph storage** — ship graph queries in a single static binary or WASM module

Every stage of the pipeline — parser, analyzer, compiler, executor, store — is implemented in this workspace. No external query engine, readable end-to-end.

## Start here

If you're new to LoraDB, take this path:

1. Try the browser playground at [play.loradb.com](https://play.loradb.com) to learn the query language without installing anything.
2. Pick the binding for the application you are already building: Node, Python, WASM, Go, Ruby, Rust, or HTTP.
3. Run the quick start below, then read the matching guide on [loradb.com/docs](https://loradb.com/docs).
4. Decide whether your first prototype can stay in memory or needs persistence. Plain in-memory handles start empty on every process run; use snapshots, a named `.loradb` archive, or a WAL-backed open when data should survive restarts.

Use the HTTP server for demos, `curl`, and polyglot integration. Use an in-process binding when you want the strongest host-language types, lower call overhead, or direct embedding.

## Installation

LoraDB ships a single Rust engine with bindings for the major application runtimes, plus a standalone HTTP server. Pick the surface that matches your host.

### Rust (crates.io)

```toml
# Cargo.toml
[dependencies]
lora-database = "0.14"
```

&nbsp;→ [crates.io/crates/lora-database](https://crates.io/crates/lora-database)

### Node.js / TypeScript (npm)

```bash
npm install @loradb/lora-node
```

&nbsp;→ [npmjs.com/package/@loradb/lora-node](https://www.npmjs.com/package/@loradb/lora-node)

### Python (PyPI)

```bash
pip install lora-python
```

&nbsp;→ [pypi.org/project/lora-python](https://pypi.org/project/lora-python/)

### WebAssembly (npm)

```bash
npm install @loradb/lora-wasm
```

&nbsp;→ [npmjs.com/package/@loradb/lora-wasm](https://www.npmjs.com/package/@loradb/lora-wasm)

### Go (Go modules)

```bash
go get github.com/lora-db/lora/crates/bindings/lora-go
```

The Go binding is a thin cgo layer over `lora-ffi`; builds require the
`liblora_ffi` static library on disk. See
[`crates/bindings/lora-go/README.md`](crates/bindings/lora-go/README.md) for the local
checkout path and the prebuilt-archive path.

### Ruby (RubyGems)

```bash
gem install lora-ruby
# or in a Gemfile
gem "lora-ruby"
```

&nbsp;→ [rubygems.org/gems/lora-ruby](https://rubygems.org/gems/lora-ruby)

### Standalone server (GitHub Releases)

Prebuilt `lora-server` binaries for Linux, macOS (Intel + Apple Silicon), and Windows are attached to every tagged release.

&nbsp;→ [github.com/lora-db/lora/releases](https://github.com/lora-db/lora/releases)

## Quick start

### Node.js

```js
import { createDatabase } from "@loradb/lora-node";

const db = await createDatabase();

await db.execute(`
  CREATE (a:User {name: 'Alice'}),
         (b:User {name: 'Bob'}),
         (a)-[:FOLLOWS {since: 2024}]->(b)
`);

const result = await db.execute(`
  MATCH (a:User)-[:FOLLOWS]->(b:User)
  RETURN a.name AS follower, b.name AS followee
`);

console.log(result.rows);
```

### Index hot predicates

```cypher
CREATE INDEX user_email FOR (u:User) ON (u.email);
CREATE TEXT INDEX user_name FOR (u:User) ON (u.name);
CREATE POINT INDEX venue_location FOR (v:Venue) ON (v.location);

SHOW INDEXES;
```

The optimizer can use declared RANGE, TEXT, and POINT indexes for matching
node or relationship predicates such as equality/range comparisons, string
prefix/substring/suffix filters, and `geo.within_bbox(...)` /
`geo.distance(...) <= radius`.

### Python

```python
from lora_python import Database

db = Database.create()
db.execute("CREATE (:User {name: 'Alice'})")

result = db.execute("MATCH (n:User) RETURN n.name AS name")
print(result["rows"])
```

### Go

```go
package main

import (
    "fmt"
    "log"

    lora "github.com/lora-db/lora/crates/bindings/lora-go"
)

func main() {
    db, err := lora.New()
    if err != nil { log.Fatal(err) }
    defer db.Close()

    if _, err := db.Execute(
        "CREATE (:User {name: $n})",
        lora.Params{"n": "Alice"},
    ); err != nil { log.Fatal(err) }

    r, err := db.Execute("MATCH (n:User) RETURN n.name AS name", nil)
    if err != nil { log.Fatal(err) }

    fmt.Println(r.Columns, r.Rows)
}
```

### Ruby

```ruby
require "lora_ruby"

db = LoraRuby::Database.create
db.execute("CREATE (:User {name: $n})", { n: "Alice" })

result = db.execute("MATCH (n:User) RETURN n.name AS name")
puts result["rows"]
```

### Rust

```rust
use lora_database::Database;

let db = Database::in_memory();
db.execute("CREATE (:User {name: 'Alice'})", None)?;

let result = db.execute("MATCH (n:User) RETURN n.name", None)?;
```

### HTTP (standalone server)

```bash
cargo run -p lora-server
# => LoraDB server running at http://127.0.0.1:4747

curl -s http://127.0.0.1:4747/query \
  -H 'Content-Type: application/json' \
  -d '{"query": "CREATE (:User {name: $name}) RETURN *", "params": {"name": "Alice"}}'
```

Result formats: `rows`, `rowArrays`, `graph` (default), `combined`.
The HTTP body accepts JSON `params` for scalar, list, and map values:

```bash
curl -s http://127.0.0.1:4747/query \
  -H 'Content-Type: application/json' \
  -d '{"query": "MATCH (u:User) WHERE u.name = $name RETURN u.name AS name", "format": "rows", "params": {"name": "Alice"}}'
```

See [loradb.com/docs/api/http](https://loradb.com/docs/api/http) for the full API.

## Documentation

**📖 [loradb.com](https://loradb.com)** — language reference, cookbook, function catalogue, and API guides.

If you're new, read in this order:

1. [What is LoraDB](https://loradb.com/docs/) — fit, boundaries, and first query
2. [Installation](https://loradb.com/docs/getting-started/installation) — pick a host language
3. [Tutorial](https://loradb.com/docs/getting-started/tutorial) — ten-minute guided tour
4. [Cheat sheet](https://loradb.com/docs/queries/cheat-sheet) — query syntax at a glance
5. [Limitations](https://loradb.com/docs/limitations) — current edges before you commit

In-repo references:

| Area | Link |
|------|------|
| Architecture overview | [docs/architecture/overview.md](docs/architecture/overview.md) |
| Graph engine internals | [docs/architecture/graph-engine.md](docs/architecture/graph-engine.md) |
| Cypher support matrix | [docs/reference/cypher-support-matrix.md](docs/reference/cypher-support-matrix.md) |
| Query indexes | [apps/loradb.com/docs/queries/indexes.md](apps/loradb.com/docs/queries/indexes.md) |
| Value model | [docs/internals/value-model.md](docs/internals/value-model.md) |
| Adding Cypher features | [docs/internals/cypher-development.md](docs/internals/cypher-development.md) |
| Known limitations | [docs/design/known-risks.md](docs/design/known-risks.md) |
| Release process | [RELEASING.md](RELEASING.md) |

## Development setup

LoraDB is a Cargo workspace with Node, Python, WASM, Go, and Ruby bindings hanging off dedicated crates, plus a shared `lora-ffi` C ABI that the Go binding links against.

**Prerequisites**

- Rust 1.87+ through `rustup` (the stable channel is pinned in [`rust-toolchain.toml`](rust-toolchain.toml); `rustfmt` + `clippy` are installed automatically)
- Node.js 20+ for repo tooling, `lora-wasm`, packages, the docs site, and the playground. The published `@loradb/lora-node` runtime supports Node.js 18+.
- Python 3.8+ with `maturin` (only for `lora-python`)
- Go 1.21+ and a C toolchain with cgo enabled (only for `lora-go`)
- Ruby 3.1+ with `bundler` (only for `lora-ruby`)

**Clone and bootstrap**

```bash
git clone https://github.com/lora-db/lora.git
cd lora
cargo build --workspace
```

For JavaScript workspaces, enable the pinned Yarn release before running
workspace scripts:

```bash
corepack enable
yarn install --immutable
```

First-time sanity checks:

```bash
cargo test --workspace
yarn workspace @loradb/lora-query build     # needed by docs + playground
yarn workspace loradb-docs validate-cypher  # parser-check public Cypher examples
yarn workspace loradb-docs build            # validates docs links/routes
```

If setup fails, check [Troubleshooting](https://loradb.com/docs/troubleshooting)
first; the common fixes are a missing C toolchain, an old Node version, or a
stale workspace dependency build.

**Repository layout**

```
lora/
├── crates/
│   ├── lora-ast/         AST types
│   ├── lora-parser/      PEG grammar + lowering
│   ├── lora-builtins-meta/
│   │                     Generated metadata for functions/operators
│   ├── lora-analyzer/    Semantic analysis
│   ├── lora-compiler/    Logical + physical planning
│   ├── lora-executor/    Plan interpreter
│   ├── lora-store/       In-memory graph, value types, mutation events
│   ├── lora-io/          Filesystem/container path handling
│   ├── lora-snapshot/    Columnar snapshot codec
│   ├── lora-wal/         Write-ahead log segments and replay
│   ├── lora-database/    Pipeline entry point, transactions, durability
│   ├── lora-server/      Axum HTTP server
│   └── bindings/
│       ├── lora-binding-buffer/
│       │                 Shared binary buffer helpers for bindings
│       ├── lora-ffi/     C ABI over lora-database (used by lora-go)
│       ├── lora-node/    Node.js bindings (napi-rs)
│       ├── lora-python/  Python bindings (PyO3 / maturin)
│       ├── lora-wasm/    WebAssembly bindings
│       ├── lora-go/      Go bindings (cgo over lora-ffi)
│       ├── lora-ruby/    Ruby bindings (Magnus / rb-sys)
│       └── shared-ts/    Shared TypeScript types for lora-node + lora-wasm
├── apps/loradb.com/      Documentation site (Docusaurus)
└── docs/                 Design docs and internals
```

## Building

```bash
# Full workspace
cargo build --workspace

# Release build of the HTTP server
cargo build --release -p lora-server

# Node.js bindings
yarn workspace @loradb/lora-node build

# Python bindings (produces a wheel)
cd crates/bindings/lora-python && maturin build --release

# WebAssembly bindings
yarn workspace @loradb/lora-wasm build

# Shared FFI (static library consumed by lora-go)
cargo build --release -p lora-ffi

# Go bindings (requires lora-ffi built above)
cd crates/bindings/lora-go && go test -race ./...

# Ruby bindings (native extension via rb-sys)
cd crates/bindings/lora-ruby && bundle install && bundle exec rake compile
```

## Testing

```bash
cargo test --workspace        # Rust unit + integration tests
cargo clippy --workspace      # Lints
cargo fmt --all --check       # Formatting
cargo bench                   # Criterion benchmarks
```

Integration coverage lives in `crates/lora-database/tests/` (one file per feature area) and `crates/lora-server/tests/http.rs`. Benchmarks are Criterion-driven and tracked by the `benchmarks` workflow — see [docs/performance/benchmarks.md](docs/performance/benchmarks.md).

## CI/CD

LoraDB ships via GitHub Actions. Every push and pull request runs the full quality gate; tagged releases fan out to crates.io, npm, PyPI, and GitHub Releases.

| Workflow | Purpose |
|----------|---------|
| [`workspace-quality`](.github/workflows/workspace-quality.yml) | `cargo fmt`, `clippy`, `test --workspace` on every PR |
| [`lora-node`](.github/workflows/lora-node.yml) | Build + test Node.js bindings across platforms |
| [`lora-python`](.github/workflows/lora-python.yml) | Build + test Python wheels across platforms |
| [`lora-wasm`](.github/workflows/lora-wasm.yml) | Build + test WebAssembly bindings |
| [`lora-go`](.github/workflows/lora-go.yml) | Build `lora-ffi`, run `go vet` + `go test -race` on the Go binding |
| [`lora-ruby`](.github/workflows/lora-ruby.yml) | Compile the Ruby native extension + run `rake test` across Ruby versions |
| [`lora-server`](.github/workflows/lora-server.yml) | Build standalone server binaries |
| [`benchmarks`](.github/workflows/benchmarks.yml) | Criterion performance regression tracking |
| [`release`](.github/workflows/release.yml) | Tag-driven release of server binaries |
| [`packages-release`](.github/workflows/packages-release.yml) | Tag-driven publish of npm / PyPI / RubyGems + verify-only path for the Go module |
| [`cargo-release`](.github/workflows/cargo-release.yml) | crates.io publish orchestration |
| [`loradb-docs`](.github/workflows/loradb-docs.yml) | Deploys [loradb.com](https://loradb.com) |
| [`commitlint`](.github/workflows/commitlint.yml) | Conventional-commit enforcement |

Conventional Commits are enforced on every PR via `commitlint` + Husky. Local Husky commits also run `cargo fmt --all --check` and `cargo clippy --workspace -- -D warnings` before commitlint. Releases are driven by `git-cliff` — see [RELEASING.md](RELEASING.md).

## Contributing

Contributions are welcome. Before opening a PR, please read [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).

- Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, …) — enforced by commitlint
- Run `cargo fmt --all --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` before pushing
- Open an issue first for anything larger than a bug fix or docs change

## License

LoraDB is licensed under the [Business Source License 1.1](LICENSE). Each covered release converts to Apache 2.0 on its Change Date (April 19, 2029 for the current release line).

You **can** use LoraDB for development, testing, evaluation, internal business use, internal production, and embedded in your own applications. You **can't** offer LoraDB as database-as-a-service, a hosted API for third parties, or a competing resale offering. See [docs/license/usage.md](docs/license/usage.md) for plain-English guidance.

The [`apps/loradb.com`](apps/loradb.com) documentation site is separately [MIT-licensed](apps/loradb.com/LICENSE).

---

<div align="center">
  <sub>
    <a href="https://loradb.com">Website</a> ·
    <a href="https://loradb.com/docs">Docs</a> ·
    <a href="https://play.loradb.com">Playground</a> ·
    <a href="https://github.com/lora-db/lora">GitHub</a> ·
    <a href="https://github.com/lora-db/lora/issues">Issues</a>
  </sub>
  <br>
  <sub>© LoraDB, Inc. — Built in Rust. Embeddable by design.</sub>
</div>
