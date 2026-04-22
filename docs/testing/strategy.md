# Testing Strategy

## Test suite summary

**1698 passing tests, 0 failing, 58 ignored** across the workspace (as of the most recent `cargo test --workspace` run).

## Test locations

| Crate | Test type | Location | What it covers |
|-------|-----------|----------|---------------|
| `lora-store` | Unit tests | `src/memory.rs` (`#[cfg(test)]`) | Node / relationship CRUD, label normalization, adjacency, property mutation, delete semantics, schema helpers |
| `lora-analyzer` | Unit tests | `src/analyzer.rs` (`#[cfg(test)]`) | Semantic validation (for example, unknown rel type in MATCH vs CREATE) |
| `lora-parser` | Unit tests | `tests/parser.rs` *(plus the file in lora-database)* | Grammar rules: match, where, return, create, delete, set, remove, merge, unwind, union, call, with, case, literals, parameters, relationships, ranges, star, order / skip / limit, string escapes, operators |
| `lora-database` | Integration tests | `tests/*.rs` | Full pipeline (parse → analyze → compile → execute) for all Cypher features |
| `lora-server` | HTTP tests | `tests/http.rs` | Axum routing, health, query endpoint, parse-error response, create-then-match flow |
| `lora-go` | Go tests | `crates/lora-go/*_test.go` | cgo round-trip over `lora-ffi`, execute + params, typed value shapes, error codes, context cancellation semantics. CI: `.github/workflows/lora-go.yml` (`go vet` + `go test -race` + `go run ./examples/basic`) |
| `lora-ruby` | Ruby tests | `crates/lora-ruby/test/` (minitest) | rb-sys / Magnus round-trip, execute + params, typed value shapes, error classes, GVL release. CI: `.github/workflows/lora-ruby.yml` (`rake compile` + `rake test` across Ruby 3.1/3.2/3.3) |

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
| `paths.rs` | Variable-length traversal, fixed / unbounded ranges, zero-hop, direction, cycles, chains, diamonds, fan patterns, `shortestPath`, `allShortestPaths` |
| `projection.rs` | `RETURN` expressions, aliases, star, distinct, literals, computed columns, map projection |
| `temporal.rs` | `Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`, `Duration` — construction, component access, comparison, arithmetic |
| `types_advanced.rs` | List indexing / slicing / concatenation / equality, map operations, null semantics, type coercion, mixed types |
| `union.rs` | `UNION`, `UNION ALL`, deduplication, multi-branch, `ORDER BY` on result |
| `update.rs` | `SET` property / label / replace / merge, `REMOVE` property / label, `DELETE`, `DETACH DELETE` |
| `where_clause.rs` | Comparison, boolean, string predicates, null checks, `IN`, regex, list predicates, arithmetic, relationship properties |
| `with.rs` | Variable piping, renaming, filtering, aggregation, star, ordering, pagination |
| `seeds.rs` | Shared seed-graph builders (social, org, transport, knowledge, …) |
| `test_helpers.rs` | `TestDb` helper with `run` / `assert` / `column` / `scalar` utilities |
| `advanced_queries.rs` | Complex multi-clause queries and forward-looking features (most are `#[ignore]`) |
| `parser.rs` | Parse-to-AST coverage exercised via `Database::parse` |

## Ignored tests (58)

All ignored tests carry an explicit reason via `#[ignore = "..."]`. Categories:

| Reason | Count | Category |
|--------|-------|----------|
| `pending implementation` | ~45 | Forward-looking: `CALL { … }` subqueries, `FOREACH`, DDL, some pattern / aggregation edge cases |
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

## Benchmarks

Located under `crates/lora-database/benches/`:

| File | Focus |
|------|-------|
| `engine_benchmarks.rs` | Core engine: MATCH, traversal, filtering, aggregation, ordering, writes, functions, realistic workloads |
| `scale_benchmarks.rs` | Scalability across tiny / small / medium graphs |
| `advanced_benchmarks.rs` | Complex queries: joins, sub-patterns, deeply nested paths |
| `temporal_spatial_benchmarks.rs` | Temporal and spatial type operations |
| `fixtures.rs` | Shared graph patterns (chains, social, org, dependency) |

Run with `cargo bench --package lora-database`.

## Test organization conventions

- Each test file covers one feature area
- Tests use `TestDb::new()` for isolated in-memory graph instances
- Shared seed graphs live in `tests/seeds.rs`
- Ignored tests use `#[ignore = "reason"]` to document what they would test
- Tests exercise the full pipeline (parse → analyze → compile → execute) through `Database`

## Recommended testing improvements

1. **Optimizer tests** — verify plan transformations (filter push-down has no dedicated test)
2. **Concurrency tests** — exercise mutex behavior under parallel requests
3. **Property-based testing** — generate random Cypher queries to stress the parser / executor
4. **HTTP parameter tests** — parameters work through the Rust API but the HTTP server does not yet forward them
5. **Temporal / spatial edge cases** — leap years, UTC offsets at boundaries, antipodal Haversine, cross-SRID comparisons
