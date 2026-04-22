# Architecture Overview

## System summary

Lora is a Rust workspace implementing an **in-memory property graph database** with full **Cypher query language support**. The entire query pipeline and storage engine are implemented from scratch — there is no external graph database behind it.

The system is structured as eight crates that form a compiler-style pipeline plus an orchestration layer:

```
Cypher text
    |
    v
[lora-parser]    PEG grammar (pest) -> AST
    |
    v
[lora-analyzer]  semantic analysis -> ResolvedQuery
    |
    v
[lora-compiler]  logical plan -> optimizer -> physical plan
    |
    v
[lora-executor]    interpret physical plan against [lora-store]
    |
    v
[lora-database]    owns the store and drives the pipeline end-to-end
    |
    v
[lora-server]      Axum HTTP / JSON transport
    |
    v
JSON result
```

## Crate responsibilities

### lora-ast

Pure data definitions. All AST node types (`Document`, `Statement`, `Query`, `Expr`, `Pattern`, …) carry a `Span` for error reporting. Depends only on `smallvec`.

**Key file**: `src/ast.rs`

### lora-parser

Defines the Cypher grammar in PEG notation (pest) and lowers parse trees into the typed AST from `lora-ast`. Exposes `parse_query(&str) -> Result<Document>`.

**Key files**:
- `src/cypher.pest` — the grammar
- `src/parser.rs` — pest-pair-to-AST lowering

### lora-store

Defines the `GraphStorage` (read) and `GraphStorageMut` (write) traits and provides `InMemoryGraph`, a BTreeMap-backed implementation with secondary indexes for labels, relationship types, and adjacency. Also defines the temporal and spatial value types (`CypherDate`, `CypherDateTime`, `CypherDuration`, `CypherPoint`, …) shared between the store and the executor.

**Key files**:
- `src/graph.rs` — trait definitions, `NodeRecord`, `RelationshipRecord`, `PropertyValue`
- `src/memory.rs` — `InMemoryGraph`
- `src/temporal.rs` — temporal value types and parsing
- `src/spatial.rs` — `CypherPoint` and distance functions

### lora-analyzer

Semantic analysis pass. Takes an AST `Document` plus a `&dyn GraphStorage` reference, resolves variable scoping, validates labels / types / properties against the live graph for read contexts, and produces a `ResolvedQuery` with `VarId`-based bindings.

**Key files**:
- `src/analyzer.rs` — main analysis logic
- `src/resolved.rs` — resolved IR types
- `src/scope.rs` — `ScopeStack` for variable scoping
- `src/symbols.rs` — `VarId` and `SymbolTable`
- `src/errors.rs` — `SemanticError` enum

### lora-compiler

Two-phase compilation:

1. **Planner** — converts `ResolvedQuery` into a `LogicalPlan` (a vector of `LogicalOp` nodes with a root index)
2. **Optimizer** — applies rewrite rules (currently: push filters below projections)
3. **Lowering** — converts `LogicalPlan` into `PhysicalPlan` (for example, `NodeScan` with a label becomes `NodeByLabelScan`, `Aggregation` becomes `HashAggregation`)

**Key files**:
- `src/logical.rs` — logical operator definitions
- `src/physical.rs` — physical operator definitions
- `src/planner.rs` — logical plan construction
- `src/pattern.rs` — pattern-specific planning (node scans, expands, inline property filters, shortest-path flag propagation)
- `src/optimizer.rs` — rewrite rules and physical lowering

### lora-executor

Interprets a `PhysicalPlan` against a `GraphStorage` (read-only) or `GraphStorageMut` (writes). Uses a row-at-a-time Volcano-style model. Contains expression evaluation, value types, and result projection into multiple output formats.

**Key files**:
- `src/executor.rs` — `Executor` (read-only) and `MutableExecutor`
- `src/eval.rs` — expression evaluator and function dispatch
- `src/value.rs` — `LoraValue` enum, `Row`, `QueryResult`, projection logic
- `src/errors.rs` — `ExecutorError` enum

### lora-database

Orchestration layer. Owns `Arc<Mutex<S: GraphStorage + GraphStorageMut>>` and exposes a single `Database` entry point with `execute` / `execute_with_params`. Drives the full parse → analyze → compile → execute pipeline so callers (HTTP server, benchmarks, examples, embedded consumers) don't depend on the individual pipeline crates.

**Key files**:
- `src/database.rs` — `Database` struct, `QueryRunner` trait
- `src/lib.rs` — re-exports `Database`, `QueryRunner`, `InMemoryGraph`, `LoraValue`, `ExecuteOptions`, `QueryResult`, `ResultFormat`, `parse_query`

The integration test suite for the full pipeline lives here under `tests/`.

### lora-server

Thin Axum-based HTTP transport. Wraps any `QueryRunner` implementation — by default `Arc<Database<InMemoryGraph>>`. No pipeline logic of its own.

**Key files**:
- `src/main.rs` — entry point; parses `--host`/`--port` (with `LORA_SERVER_HOST`/`LORA_SERVER_PORT` env fallbacks, default `127.0.0.1:4747`) and serves a `Database::in_memory()` instance
- `src/config.rs` — CLI/env parser for bind address and port
- `src/app.rs` — `build_app`, route handlers, request / response types

## Architecture diagram

```mermaid
graph TD
    Client[HTTP Client] -->|POST /query| Server[lora-server<br/>Axum Router]
    Server -->|QueryRunner::execute| DB[lora-database<br/>Database]
    DB --> P[lora-parser<br/>pest grammar]
    P --> AST[lora-ast<br/>Document]
    AST --> A[lora-analyzer<br/>Semantic analysis]
    A --> RQ[ResolvedQuery]
    RQ --> C[lora-compiler<br/>Planner + Optimizer]
    C --> PP[PhysicalPlan]
    PP --> E[lora-executor<br/>Interpreter]
    E -->|read/write| S[lora-store<br/>InMemoryGraph]
    E --> R[QueryResult<br/>JSON]
    R --> Client
```

## Design principles (observed)

1. **Compiler-style pipeline** — each stage has a well-defined input and output type
2. **Trait-based storage** — `GraphStorage` / `GraphStorageMut` allow alternative backends
3. **ID-based references** — `VarId`, `NodeId`, `RelationshipId` are simple numeric types; string names are resolved once during analysis
4. **Read / write separation** — the executor has distinct `Executor` and `MutableExecutor` structs
5. **Plan-based execution** — queries compile to explicit plans; the executor never interprets the AST directly
6. **Transport-agnostic core** — `lora-database` exposes a `QueryRunner` trait so HTTP, benches, examples, and embedded callers share one pipeline
7. **Zero external runtime dependencies** — no database, no JVM, pure Rust

> 💡 **Tip** — The transport-agnostic `QueryRunner` trait means the same pipeline drives HTTP (`lora-server`), embedded Rust consumers (`lora-database`), the language bindings (`lora-node`, `lora-python`, `lora-wasm`, `lora-ruby`), and the `lora-ffi` C ABI that `lora-go` cgo-links against. If you need a custom transport, implement `QueryRunner` — you don't need to touch any pipeline crate.

## Next steps

- Walk through a query in detail: [Data Flow](data-flow.md)
- Understand the storage internals: [Graph Engine](graph-engine.md)
- Add a new clause or function: [Cypher Development](../internals/cypher-development.md)
- Run and operate the server: [Deployment](../operations/deployment.md)
- For managed, multi-node deployments with persistence and replication: [LoraDB platform](https://loradb.com)
