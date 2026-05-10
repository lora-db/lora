# Testing Strategy

## Test suite summary

The workspace has Rust unit/integration tests, binding tests, server tests, and
Criterion benches. Run `cargo test --workspace` before publishing exact counts;
this file tracks where coverage lives rather than freezing a count that changes
on nearly every feature branch.

## Test locations

| Crate | Test type | Location | What it covers |
|-------|-----------|----------|---------------|
| `lora-store` | Unit tests | `src/memory/` (`#[cfg(test)]`) | Node / relationship CRUD, label normalization, adjacency, property mutation, delete semantics, index catalog helpers |
| `lora-analyzer` | Unit tests | `src/analyzer.rs` (`#[cfg(test)]`) | Semantic validation (for example, unknown rel type in MATCH vs CREATE) |
| `lora-parser` | Unit tests | `tests/parser.rs` *(plus parser module tests)* | Grammar rules: match, where, return, create, delete, set, remove, merge, unwind, union, call, with, schema/index DDL, case, literals, parameters, relationships, ranges, star, order / skip / limit, string escapes, operators |
| `lora-database` | Integration tests | `tests/*.rs` | Full pipeline (parse → analyze → compile → execute) for all Cypher features, plus snapshot save / load / format-version compatibility |
| `lora-server` | HTTP tests | `tests/{http,admin}.rs` | Axum routing, health, query endpoint, parse-error response, create-then-match flow, opt-in admin snapshot endpoints |
| `lora-go` | Go tests | `crates/bindings/lora-go/*_test.go` | cgo round-trip over `lora-ffi`, execute + params, typed value shapes, error codes, context cancellation semantics. CI: `.github/workflows/lora-go.yml` (`go vet` + `go test -race` + `go run ./examples/basic`) |
| `lora-ruby` | Ruby tests | `crates/bindings/lora-ruby/test/` (minitest) | rb-sys / Magnus round-trip, execute + params, typed value shapes, error classes, GVL release. CI: `.github/workflows/lora-ruby.yml` (`rake compile` + `rake test` across Ruby 3.1/3.2/3.3) |

## Integration test files (`lora-database/tests/`)

| File | Coverage area |
|------|--------------|
| `aggregation.rs` | `count`, `sum`, `avg`, `min`, `max`, `collect`, `stdev`, `stdevp`, `percentileCont`, `percentileDisc`; grouped, distinct, empty set, null handling |
| `create.rs` | Node / relationship creation, labels, properties, patterns, batch, unicode |
| `errors.rs` | Parse errors, semantic errors, unknown labels / types / properties / variables / functions, arity checks |
| `expressions.rs` | Arithmetic, boolean, comparison, string ops, `CASE`, functions, `UNWIND`, list comprehension, regex, `EXISTS` subquery, pattern comprehension |
| `functions_extended.rs` | String, math (incl. trig), list, type conversion, entity introspection, path, temporal, spatial, list predicates, map projection |
| `invariants.rs` | Graph integrity after mutations, node / relationship consistency, isolation |
| `match.rs` | Node matching, labels, properties, relationships, direction, cross-products, multi-hop, optional match, variable binding |
| `merge.rs` | `MERGE` node / relationship, `ON MATCH SET`, `ON CREATE SET`, idempotency |
| `ordering.rs` | `ORDER BY` asc / desc, multi-key sort, null ordering, computed expressions |
| `parameters.rs` | Named / numeric parameters, all value types, parameters in WHERE / CREATE / RETURN |
| `schema.rs` | `CREATE INDEX`, `DROP INDEX`, `SHOW INDEXES`, `IF [NOT] EXISTS`, conflict errors, parameterized index names |
| `index_acceleration.rs` | Optimizer rewrites and result correctness for node/relationship RANGE, TEXT, and POINT index scans |
| `paths.rs` | Variable-length traversal, fixed / unbounded ranges, zero-hop, direction, cycles, chains, diamonds, fan patterns, `shortestPath`, `allShortestPaths` |
| `projection.rs` | `RETURN` expressions, aliases, star, distinct, literals, computed columns, map projection |
| `temporal.rs` | `Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`, `Duration` — construction, component access, comparison, arithmetic |
| `vectors.rs` | `VECTOR` construction, storage, `toIntegerList` / `toFloatList`, similarity / distance / norm functions, exhaustive kNN via `ORDER BY … LIMIT k` |
| `types_advanced.rs` | List indexing / slicing / concatenation / equality, map operations, null semantics, type coercion, mixed types |
| `union.rs` | `UNION`, `UNION ALL`, deduplication, multi-branch, `ORDER BY` on result |
| `update.rs` | `SET` property / label / replace / merge, `REMOVE` property / label, `DELETE`, `DETACH DELETE` |
| `where_clause.rs` | Comparison, boolean, string predicates, null checks, `IN`, regex, list predicates, arithmetic, relationship properties |
| `with.rs` | Variable piping, renaming, filtering, aggregation, star, ordering, pagination |
| `snapshot.rs` | Snapshot round-trip, atomic rename + `.tmp` cleanup, format-version gating, checksum failure, catalog trailer, `MutationRecorder` replay shape |
| `wal.rs` | WAL recovery, sync/checkpoint behavior, catalog DDL replay |
| `seeds.rs` | Shared seed-graph builders (social, org, transport, knowledge, …) |
| `test_helpers.rs` | `TestDb` helper with `run` / `assert` / `column` / `scalar` utilities |
| `advanced_queries.rs` | Complex multi-clause queries and forward-looking features (most are `#[ignore]`) |
| `parser.rs` | Parse-to-AST coverage exercised via `Database::parse` |

## Server integration test files (`lora-server/tests/`)

| File | Coverage area |
|------|--------------|
| `http.rs` | Core HTTP surface — routing, `/health`, `/query` happy / parse-error paths, create-then-match |
| `admin.rs` | `POST /admin/snapshot/{save,load}` — body handling, default-path behavior, `path` override, opt-in 404 when `--snapshot-path` is unset, round-trip against a live server |

## Ignored tests (58)

All ignored tests carry an explicit reason via `#[ignore = "..."]`. Categories:

| Reason | Count | Category |
|--------|-------|----------|
| `pending implementation` | ~45 | Forward-looking: `CALL { … }` subqueries, `FOREACH`, constraints, some pattern / aggregation edge cases |
| `stored procedures: CALL db.labels() not yet implemented` | 1 | Procedures |
| `FOREACH clause not yet in grammar` | 1 | Clause |
| `temporal types: date/time functions not yet implemented` | 2 | Historical — most temporal tests now pass |
| `duration type: duration arithmetic not yet implemented` | 1 | Specific duration edge case |
| `APOC utilities: apoc-like utility functions not yet implemented` | 1 | Functions |
| `constraint violation rollback: rollback on constraint error not yet implemented` | 1 | Transactions |
| `type validation: type mismatch in comparison not yet detected` | 1 | Validation |
| `parameter as label: dynamic labels via parameters not standard Cypher` | 1 | Parameters |
| `parameter validation: type checking at parse time not yet implemented` | 1 | Parameters |

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
| `concurrency_guard.rs` | Focused concurrency guardrail suite |
| `engine.rs`, `advanced.rs`, `temporal_spatial.rs` | Older deep-dive suites retained for historical comparison; prefer `query_implementations.rs` for new query-feature coverage |
| `perf_smoke.rs` | 4-bench CI canary for ≥3× regressions — see [perf-smoke docs](../performance/perf-smoke.md) |
| `fixtures.rs` | Shared graph patterns (chains, social, org, dependency) |

Run with `cargo bench --package lora-database`.

The `perf_smoke` suite also runs automatically on every PR
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
2. **Concurrency tests** — exercise store lock behavior under parallel requests
3. **Property-based testing** — generate random Cypher queries to stress the parser / executor
4. **Property-based snapshot round-trips** — generate random graphs, save, load, assert structural equality
5. **HTTP parameter tests** — parameters work through the Rust API but the HTTP server does not yet forward them
6. **Temporal / spatial edge cases** — leap years, UTC offsets at boundaries, antipodal Haversine, cross-SRID comparisons
