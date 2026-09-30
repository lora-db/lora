# Testing Strategy

## Test suite summary

The workspace has Rust unit/integration tests, binding tests, server tests, and
Criterion benches. Run `cargo test --workspace` before publishing exact counts;
this file tracks where coverage lives rather than freezing a count that changes
on nearly every feature branch.

## Test locations

| Crate | Test type | Location | What it covers |
|-------|-----------|----------|---------------|
| `lora-store` | Unit tests | `src/memory/tests.rs` and module `#[cfg(test)]` blocks | Node / relationship CRUD, label normalization, adjacency, property mutation, delete semantics, index catalog helpers |
| `lora-analyzer` | Unit tests | `src/analyzer/tests.rs` | Semantic validation (scoping, unbound variables, builtin signatures) |
| `lora-parser` | Unit + integration tests | `src/parser/` (`#[cfg(test)]`), `tests/error_messages.rs` | Grammar rules and parse-error messages. End-to-end parse coverage lives in `lora-database/tests/parser.rs` |
| `lora-compiler`, `lora-executor` | Unit tests | module `#[cfg(test)]` blocks (`optimizer.rs`, `eval/binops.rs`, `eval/builtins/`, `executor/optional.rs`, …) | Optimizer rewrites, operators, builtin functions, cancellation |
| `lora-wal`, `lora-snapshot` | Unit + integration tests | module `#[cfg(test)]` blocks, `tests/error_messages.rs` | Segment and LSN handling, codec round-trips, error-message baselines |
| `lora-database` | Integration tests | `tests/*.rs` | Full pipeline (parse → analyze → compile → execute) for all Cypher features, plus transactions, WAL, snapshots and the change feed |
| `lora-server` | HTTP tests | `tests/{http,admin,concurrency,error_messages}.rs` | Axum routing, health, query endpoint, parse-error response, create-then-match flow, opt-in admin snapshot endpoints, concurrent writes |
| `lora-ffi` | Integration tests | `crates/bindings/lora-ffi/tests/pool.rs` | Streams and writers pulled from a thread pool |
| `lora-node` | Vitest | `crates/bindings/lora-node/test/` | Execute, transactions, interactive `begin()`, 64-bit integers, timeouts, directory locking, the libuv pool, change feed, explain/profile |
| `lora-wasm` | Vitest | `crates/bindings/lora-wasm/test/` | Database API, single-thread writer-lock behaviour, the worker client |
| `lora-python` | pytest | `crates/bindings/lora-python/tests/` | Sync and async APIs, concurrency (GIL release), explain/profile |
| `lora-go` | Go tests | `crates/bindings/lora-go/*_test.go` | cgo round-trip over `lora-ffi`, execute + params, typed value shapes, error codes, context cancellation semantics, snapshots, concurrency. CI: `.github/workflows/lora-go.yml` (`go vet` + `go test -race` + `go run ./examples/basic`) |
| `lora-ruby` | Ruby tests | `crates/bindings/lora-ruby/test/` (minitest) | rb-sys / Magnus round-trip, execute + params, typed value shapes, error classes, GVL release, writer lock. CI: `.github/workflows/lora-ruby.yml` (`rake compile` + `rake test` across Ruby 3.1/3.2/3.3) |
| `@loradb/lora-graphql` | Vitest | `packages/lora-graphql/test/` | Model, TCK snapshots, integration, authorization (including a property-based reference evaluator), mutations, subscriptions, CLI, driver routing. CI: `.github/workflows/lora-graphql.yml` |
| `@loradb/lora-query`, `@loradb/lora-graph-canvas` | Vitest | `packages/*/test/` | Package-level behaviour |

## Integration test files (`lora-database/tests/`)

One file per feature area. Most areas are listed by file; the regression
files added with each engine fix are grouped by what they pin, and each
opens with a `//!` comment saying what it guards.

| Area | Files | Coverage |
|------|-------|----------|
| Reading | `match.rs`, `optional_match_correlated.rs`, `paths.rs`, `where_clause.rs`, `unknown_names.rs` | Node and relationship matching, direction, cross-products, multi-hop, `OPTIONAL MATCH` anchored on bound nodes, variable-length and shortest paths, predicates; unknown labels / types / properties match nothing or read `null` |
| Projection and ordering | `projection.rs`, `with.rs`, `with_predicates.rs`, `ordering.rs`, `order_limit_semantics.rs`, `union.rs` | `RETURN` and `WITH` piping, star, distinct, map projection, `ORDER BY` / `SKIP` / `LIMIT` after projection, aggregation and `DISTINCT`, `UNION [ALL]` |
| Expressions and values | `expressions.rs`, `functions_extended.rs`, `builtin_namespaces.rs`, `cypher_compat.rs`, `comparison_ordering.rs`, `types_advanced.rs`, `binary.rs`, `parameters.rs` | Operators, `CASE`, comprehensions, `EXISTS` / `COUNT` subqueries, namespaced and standard-name builtins, list ordering and null comparisons, lists / maps / null semantics, binary values, named and numeric parameters |
| Pattern subqueries | `pattern_subquery_scope.rs`, `pattern_subquery_rel_properties.rs`, `pattern_subquery_var_length.rs` | Pattern comprehensions and `EXISTS { }` read their bindings, honour relationship property maps and variable-length ranges |
| Aggregation | `aggregation.rs` | `count`, `sum`, `avg`, `min`, `max`, `collect`, `stdev`, `stdevp`, `percentileCont`, `percentileDisc`; grouped, distinct, empty set, null handling |
| Temporal, spatial, vector | `temporal.rs`, `temporal_constructors.rs`, `spatial.rs`, `vectors.rs` | Temporal construction, components, comparison and arithmetic; 2D/3D points, SRIDs; `VECTOR` values, similarity / distance / norm, exhaustive kNN |
| Writes | `create.rs`, `merge.rs`, `merge_bound_end.rs`, `update.rs`, `foreach.rs`, `call_subquery_writes.rs`, `unwind_ingestion.rs`, `null_properties_and_deferred_existence.rs`, `write_results.rs` | `CREATE`, `MERGE` (including bound end nodes), `SET` / `REMOVE` / `DELETE`, `FOREACH`, writes inside `CALL { }`, bulk `UNWIND` ingestion, `null` in property maps, deferred existence checks, write statements without `RETURN` |
| Schema and indexes | `schema.rs`, `schema_in_transaction.rs`, `constraints.rs`, `index_acceleration.rs`, `temporal_range_index.rs`, `order_by_index.rs`, `empty_ranges.rs`, `fulltext_index.rs`, `vector_index.rs`, `procedure_yield.rs` | Index and constraint DDL (also inside transactions), RANGE / TEXT / POINT rewrites, temporal range indexes, index-ordered `ORDER BY ... LIMIT`, full-text and vector index procedures, `CALL ... YIELD` |
| Planner and execution | `planner_pushdown.rs`, `early_limit.rs`, `explain_profile.rs`, `explain_estimates.rs`, `timeouts.rs` | Condition push-down and index seeks, early `LIMIT`, `explain()` / `profile()` and their estimates, deadlines and cancellation on every path |
| Allocation and scaling | `borrowed_variable_reads.rs`, `projection_shares_variables.rs`, `row_value_sharing.rs`, `write_scaling.rs`, `scale.rs` | Reads and projections share values instead of copying; a write's cost does not grow with graph size; a large-database correctness run (ignored by default) |
| Transactions and durability | `transactions.rs`, `invariants.rs`, `wal.rs`, `snapshot.rs`, `managed_snapshots.rs`, `change_feed.rs` | Explicit transactions, graph integrity after mutations, WAL recovery / checkpoints / DDL replay, snapshot round-trip and format gating, managed snapshots with an LSN fence, the committed-change feed |
| Errors | `errors.rs`, `error_messages.rs` | Parse and semantic errors, unbound variables, unknown functions, arity checks; error-message baselines |
| Storage traits | `backend_stub.rs` | The storage trait surface on a backend that cannot hand out long-lived borrows |
| Docs and roadmap | `docs_examples.rs`, `parser.rs`, `advanced_queries.rs` | Every query example in the site docs runs; parse-to-AST coverage via `Database::parse`; complex multi-clause queries (most `#[ignore]`d) |
| Helpers | `test_helpers.rs`, `seeds.rs` | `TestDb` with `run` / `assert` / `column` / `scalar`; shared seed graphs (social, org, transport, knowledge, …) |

## Server integration test files (`lora-server/tests/`)

| File | Coverage area |
|------|--------------|
| `http.rs` | Core HTTP surface — routing, `/health`, `/query`, `/explain`, happy / parse-error paths, params, and create-then-match |
| `admin.rs` | Snapshot and WAL admin routes — `POST /admin/snapshot/{save,load}`, `/admin/checkpoint`, `/admin/wal/status`, `/admin/wal/truncate`, body handling, default-path behavior, `path` override, opt-in 404s, and round-trips against a live server |
| `concurrency.rs` | Concurrent writes against the writer lock without runtime stalls |
| `error_messages.rs` | Error-message baselines for server errors |

## Ignored tests

All ignored tests carry an explicit reason via `#[ignore = "..."]`. Most
(`pending implementation` and a handful of specific reasons such as
`CALL db.labels()`, type mismatch detection, or parameters as labels) are
forward-looking specifications: queries the engine did not support when
they were written. Two more are slow runs (`scale.rs`, a 100k-row
`UNWIND`) that are ignored only for time.

The list is not current: a good share of the specification tests now
pass and are waiting to be un-ignored. Run them to see where they stand:

```bash
cargo test -p lora-database -- --ignored
```

Triage (un-ignore what passes, give the rest a specific reason) is tracked
in [`docs/TODO.md`](../TODO.md).

## How to run tests

```bash
# Full workspace
cargo test --workspace

# Specific crate
cargo test -p lora-database
cargo test -p lora-server
cargo test -p lora-parser
cargo test -p lora-store

# Specific file (under lora-database/tests/)
cargo test -p lora-database --test aggregation
cargo test -p lora-database --test temporal

# With output
cargo test --workspace -- --nocapture

# Include ignored tests
cargo test --workspace -- --include-ignored

# Only ignored tests
cargo test --workspace -- --ignored
```

### Cargo artifact lock waits

If `cargo test` prints `Blocking waiting for file lock on artifact directory`,
another Cargo process is compiling into the same `target/` directory. The most
common local source is rust-analyzer checking the workspace in the editor while
a terminal test run starts.

The checked-in VS Code setting at `.vscode/settings.json` points
rust-analyzer at `target/rust-analyzer`, so editor checks and terminal tests use
separate artifact locks. Reload the editor after pulling that setting. If a
test command is already blocked, wait for the current Cargo build to finish or
stop the older Cargo task, then rerun the test. When running several focused
tests locally, prefer one Cargo command with multiple `--test` arguments over
parallel terminal commands so Cargo can compile once and schedule the test
binaries itself.

## Benchmarks

Located under `crates/lora-database/benches/`:

| File | Focus |
|------|-------|
| `query_implementations.rs` | Query feature coverage aligned with integration tests: parser, explain/profile, MATCH, paths, filtering, projection, ordering, aggregation, WITH, UNION, UNWIND, writes, expressions, functions, typed values, and advanced query shapes |
| `index_acceleration.rs` | Before/after comparisons for RANGE/TEXT index rewrites on node and relationship predicates |
| `scale.rs` | Scalability across tiny / small / medium graphs |
| `realistic.rs` | Domain-shaped workloads that combine multiple operators |
| `wal.rs` | Durability and recovery overhead |
| `concurrent.rs` | Concurrent read/write workload behavior |
| `concurrency_guard.rs` | Focused concurrency guardrail suite — see [perf-smoke docs](../performance/perf-smoke.md#concurrency-guard) |
| `engine.rs`, `advanced.rs`, `temporal_spatial.rs` | Older deep-dive suites retained for historical comparison; prefer `query_implementations.rs` for new query-feature coverage |
| `perf_smoke.rs` | CI canary for large (2–3×) regressions across read, stream, transaction and WAL paths — see [perf-smoke docs](../performance/perf-smoke.md) |
| `memory.rs` | Retained-heap regression gate, run by `.github/workflows/memory-bench.yml` |
| `fixtures.rs` | Shared graph patterns (chains, social, org, dependency) |

Run with `cargo bench --package lora-database`.

The `perf_smoke` suite also runs automatically on PRs that touch the engine
via [`.github/workflows/perf-smoke.yml`](../../.github/workflows/perf-smoke.yml),
comparing against `crates/lora-database/benches/perf_smoke_baseline.json`
using `scripts/check-perf-smoke.mjs`. It is intentionally a canary, not
authoritative performance tooling.

## Test organization conventions

- Each test file covers one feature area
- Tests use `TestDb::new()` for isolated in-memory graph instances
- Shared seed graphs live in `tests/seeds.rs`
- Ignored tests use `#[ignore = "reason"]` to document what they would test
- Tests exercise the full pipeline (parse → analyze → compile → execute) through `Database`

## Recommended testing improvements

1. **Optimizer tests** — continue expanding plan-transformation coverage beyond the index acceleration suite
2. **Concurrency tests** — keep extending writer-lock coverage under parallel requests beyond `lora-server/tests/concurrency.rs` and the binding pool tests
3. **Property-based testing** — generate random Cypher queries to stress the parser / executor
4. **Property-based snapshot round-trips** — generate random graphs, save, load, assert structural equality
5. **HTTP parameter coverage** — keep expanding JSON param conversion cases beyond the current success / invalid-shape tests
6. **Temporal / spatial edge cases** — leap years, UTC offsets at boundaries, antipodal Haversine, cross-SRID comparisons
