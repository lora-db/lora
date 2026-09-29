# Known Gaps and Risks

## Classification key

- **Observed**: directly verified in the codebase or test suite
- **Inferred**: reasonably deduced from code structure and patterns
- **Needs confirmation**: uncertain, requires investigation

---

## Language features

### Not yet implemented

| Feature | Parse status | Execution status | Risk |
|---------|-------------|-----------------|------|
| General-purpose `CALL` | Parsed to AST | Only documented `db.index.vector.*` and `db.index.fulltext.*` procedures are supported; other procedures return an unsupported-feature error | Low — clear error |
| General-purpose `CALL ... YIELD` | Parsed to AST | The index query procedures compose as clauses (`CALL ... YIELD ... MATCH ...`); other procedures return an unsupported-feature error | Low — clear error |
| `LOAD CSV` | Not in grammar | N/A | Low |
| `USE <graph>` (multi-database) | Not in grammar | N/A | Low |
| `EXPLAIN` / `PROFILE` (Cypher keywords) | Not in grammar | API-only | Low — exposed as `db.explain()` / `db.profile()` API methods rather than Cypher syntax. `PROFILE` runs the query for real (including writes); `EXPLAIN` is plan-only. |
| Quantified path patterns | Not in grammar | N/A | Low — future openCypher syntax |
| Inline `WHERE` inside variable-length relationship | Not in grammar | N/A | Low — parse error |
| Type mismatch detection between comparable types | Accepted | Compared without error | Low — 1 ignored test |
| Parameter as a label or relationship type | N/A | Not implemented | Low — not standard Cypher |
| Vector ANN execution | N/A | `CREATE VECTOR INDEX` and `db.index.vector.*` procedures are queryable, but they use flat scans over the indexed scope | **Medium** — fine for demos and small corpora; dedicated ANN execution is still needed for production-scale semantic retrieval |
| List-of-`VECTOR` as a property | Parsed | Rejected at write time (`PropertyConversionError::NestedVectorInList`) | Low — loud error; shape decision to keep future indexing viable |

### Implemented since initial audit

The following features were listed as gaps in earlier revisions of this document but are now implemented and verified by tests:

| Feature | Evidence |
|---------|----------|
| HTTP `POST /query` with a `params` field | `crates/lora-server/src/app/routes.rs` parses JSON params and forwards them to `QueryRunner::execute_with_params`; `crates/lora-server/tests/http.rs` covers success and invalid-param cases |
| Temporal types (`Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`, `Duration`) | 89 passing tests in `tests/temporal.rs` |
| Spatial types (`Point`: Cartesian 2D/3D and WGS-84 2D/3D — SRIDs 7203, 9157, 4326, 4979) | Tests in `tests/functions_extended.rs` and `tests/types_advanced.rs` |
| `VECTOR` value type (six coordinate tags; cast-based construction; property storage on nodes and relationships; similarity / distance / norm functions; exhaustive kNN via `ORDER BY … LIMIT k`; cataloged vector index procedures with flat-scan execution) | Tests in `tests/vectors.rs` |
| `shortestPath()` / `allShortestPaths()` | Tests in `tests/paths.rs` |
| Advanced aggregates: `stdev`, `stdevp`, `percentileCont`, `percentileDisc` | Tests in `tests/aggregation.rs` |
| Trigonometry: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `degrees`, `radians` | Tests in `tests/functions_extended.rs` |
| `UNION` / `UNION ALL` | 59 passing tests in `tests/union.rs` |
| Variable-length paths (`*`, `*1..3`, `*0..1`, …) | Tests in `tests/paths.rs` |
| `EXISTS { pattern }` subquery | Tests in `tests/where_clause.rs` and `tests/expressions.rs` |
| Pattern comprehension | Tests in `tests/expressions.rs` |
| Map projection | Tests in `tests/projection.rs` |
| Parameter binding (`$name`, `$1`) via the Rust API | Tests in `tests/parameters.rs` |
| Index and constraint DDL (`CREATE INDEX`, `CREATE TEXT INDEX`, `CREATE POINT INDEX`, `CREATE LOOKUP INDEX`, `CREATE VECTOR INDEX`, `CREATE FULLTEXT INDEX`, `CREATE CONSTRAINT`, `DROP`, `SHOW`) | Tests in `tests/schema.rs`, vector/full-text index tests, and constraint tests |
| Unknown labels, relationship types and property keys are not errors (match nothing / read `null`), independent of the stored data | `tests/unknown_names.rs` |
| `ORDER BY` on aliases and aggregates; `SKIP` / `LIMIT` after aggregation and `DISTINCT`. Previously the sort and limit ran before the projection, silently returning unsorted rows or counts over a truncated input | `tests/order_limit_semantics.rs` |
| `CALL db.index.* (...) YIELD ... [WHERE ...]` as a composable clause, and fully hydrated nodes from standalone calls | `tests/procedure_yield.rs` |
| Full-text ASCII folding and `term*` prefix queries | `tests/procedure_yield.rs` |
| `COUNT { pattern }`, `WHERE n:Label` label predicates, standard Cypher function names (`split`, `trim`, `replace`, `point.distance`, math and path names) | `tests/cypher_compat.rs` |
| Schema DDL inside transactions (atomic with data, durable through the WAL) | `tests/schema_in_transaction.rs` |
| Write statements without `RETURN` return no rows | `tests/write_results.rs` |
| Query deadlines and cancellation reach every execution path, including streaming writes; a bounded write that times out rolls back | `tests/timeouts.rs` |
| A read with an early `LIMIT` stops scanning once the limit is met, also under a deadline and inside an explicit transaction. Previously both ran the full executor and materialized every row first | `tests/early_limit.rs` |
| Node binding: exact 64-bit integers (`bigint`), per-call `timeoutMs` / `AbortSignal` and a database-wide `queryTimeoutMs`, interactive transactions (`db.begin()`), typed `LORA_LOCKED` for a directory held by another process, `{latitude, longitude}` point params, musl prebuilds | `crates/bindings/lora-node/test/{integers,timeouts,interactive,locking}.test.ts` |

---

## Storage and data integrity

> 🚀 **Production note** — The gaps in this section (operator controls,
> scoped index coverage, and transaction isolation behavior) are by design
> for the in-memory core. They are addressed in the [LoraDB managed
> platform](https://loradb.com) — use the table below to decide whether a
> self-hosted deployment is viable, or whether the managed option is the
> better starting point.

| Gap | Classification | Risk |
|-----|---------------|------|
| WAL/operator controls are not uniform across surfaces | Observed | **Low–Medium** — Rust and `lora-server` expose explicit checkpoint/status/truncate controls. Node, Python, Go, and Ruby can open filesystem-backed WAL databases; WASM remains snapshot-only. See [WAL](../operations/wal.md). |
| Constraint/index coverage is scoped | Observed | **Medium** — uniqueness, existence, type, key, RANGE, TEXT, POINT, LOOKUP, VECTOR, and FULLTEXT surfaces exist; composite multi-property seeks and ANN vector execution are still absent |
| Transaction isolation is conservative | Observed | **Medium** — auto-commit writes publish optimistically under a writer mutex; explicit read-write transactions (including interactive `db.begin()` transactions in the Node binding) hold the writer lock for their full lifetime, so other writers wait; read-only transactions pin snapshots |
| A second in-process open of a database directory shares the open engine | Observed | Low — both handles talk to one engine and one WAL writer, never two; a second *process* gets `LORA_LOCKED` |
| Node / relationship IDs are never reused | Observed | Low — `u64` counter will not overflow in practice |
| Tombstones and clone-heavy compatibility APIs | Observed | **Low–Medium** — deleted IDs leave slot gaps; hot executor paths use borrow closures, but `all_nodes()` and other record-returning scans still allocate |

---

## Query correctness

| Issue | Classification | Risk |
|-------|---------------|------|
| `toLower` / `toUpper` are not locale-aware | Observed | Low — Unicode case mapping is supported, locale-specific folding is host-side |
| Integer overflow not explicitly handled | Inferred | Low — Rust panics in debug, wraps in release |
| `round()` returns an integer for integral results and rounds half away from zero | Observed | Low — Neo4j returns a float |
| Float comparison uses IEEE 754 | Observed | Low — `NaN != NaN` is standard |
| Variable-length undirected traversal does not guard against reciprocal edges | Inferred | Low — visited-node tracking avoids repeats |

---

## Security

| Issue | Classification | Risk |
|-------|---------------|------|
| No authentication on HTTP API | Observed | **High** for any network-exposed deployment |
| No TLS | Observed | **High** — queries and data in plaintext |
| Bind address defaults to `127.0.0.1:4747` (configurable via `--host`/`--port`, `LORA_SERVER_HOST`/`LORA_SERVER_PORT`) | Observed | Low — localhost-only default mitigates exposure |
| No query / result size limits | Inferred | **Medium** — large inputs could cause OOM |
| No rate limiting | Observed | Medium — DoS risk |

---

## Performance

| Issue | Classification | Impact |
|-------|---------------|--------|
| Write publication still serializes | Observed | Read-only auto-commit queries load Arc snapshots without a store lock; write commits and explicit read-write transactions serialize through the database writer mutex |
| Some predicates still scan | Observed | Vector similarity, regex, non-indexed properties, nested map paths, and unsupported composite seek shapes scan candidate records |
| Clone-heavy read API | Observed | Allocation overhead proportional to result set |
| Query timeout coverage | Observed | Cooperative deadlines and cancellation cover eager, streaming and write execution in Rust and the Node binding (`execute`, `transaction`, `stream`, `begin`). A `stream()` checks between rows, so one blocking pull (a large aggregation) finishes before the check. HTTP and the other bindings do not expose timeouts yet |
| Optimizer is still local | Observed | Cost-based index selection exists for scan/filter sites, and a single-key `ORDER BY` over a range-indexed property with a range predicate streams from the index. WHERE conditions are pushed down to the scan of the variable they test (so `n.key = $k` and `n.key IN $list` seek on any node of a pattern), a chain starts from its cheaper end, and an unfiltered `count(n)` over one label reads the label count. No join ordering or global cardinality search; multi-key or unbounded `ORDER BY` still sorts |
| Staged writes share structure | Observed | Writes the in-place fast path cannot prove failure-free (relationship `CREATE`, `MERGE`, `SET` from an expression, anything with a deadline) run on a copy of the graph so a failure leaves nothing behind. The copy is structural: node and relationship slots, adjacency lists and label indexes are chunked copy-on-write vectors, and every secondary index (hash, range, text, point, full-text) is split into copy-on-write shards, so a write copies only the chunks and shards it touches. A single-row write costs tens of microseconds regardless of graph size (`tests/write_scaling.rs`) |

### Memory per element

Measured with a counting allocator (`cargo run --release -p lora-database
--example heap_probe`); live heap only, so process RSS adds allocator
overhead on top.

| Element | Bytes |
|---------|-------|
| Node, no properties, one label | ~170 |
| Each property on a node or relationship (inline value) | ~64, plus the value's own heap for strings, lists, maps |
| Relationship, no properties | ~115 |
| Unique-valued entry in a RANGE index or uniqueness constraint | ~290 |
| 20k festivals + 5k users + 100k relationships, no indexes | ~320 per element |
| Same, with two uniqueness constraints, a full-text and a point index | ~430 per element |

Declaring indexes and constraints before a bulk load keeps the load
linear: a uniqueness check looks the value up in its backing index rather
than scanning the graph. For the largest loads, creating indexes after
the load is still somewhat faster because the index is built in one pass.

---

## Developer experience

| Gap | Classification | Notes |
|-----|---------------|-------|
| No `tracing-subscriber` configured in `main.rs` | Observed | `tracing` macros exist but produce no output |
| No configurable log level | Observed | Host/port are configurable (see `lora-server --help`); log level is not. |
| CLI argument parsing is hand-rolled | Observed | `--host`, `--port`, `--help`, `--version`; no dependency on `clap`. |
| CI pipeline | Addressed | See `.github/workflows/lora-server.yml` and `.github/workflows/release.yml`. |

---

## Recommended priorities

### Short term (correctness / developer experience)

1. Add `tracing-subscriber` so the existing `tracing` instrumentation produces output
2. Add configurable log level
3. Add query length and result-size limits in the HTTP layer

### Medium term (robustness)

5. Expand timeout coverage to HTTP and the Python, Go, Ruby and WASM bindings (Rust and Node are covered)
6. Add authentication middleware to the HTTP server
7. Add ANN execution for vector indexes and broader composite-index optimizer rewrites
8. ~~Introduce borrowing iterators on `GraphStorage`~~ — partially addressed: `with_node` / `with_relationship` closures now cover the executor's hot paths without requiring `&NodeRecord` access from every backend. Still open: streaming iterators for bulk scans

### Long term (capability)

10. ~~Persistence (WAL and/or snapshots)~~ — partially addressed:
    snapshots ship across surfaces, WAL-backed opens ship on
    filesystem-backed surfaces, and Rust / `lora-server` expose explicit
    checkpoint/admin controls. Remaining work is operational polish,
    scheduled checkpoints, and richer multi-process/process-manager guidance.
11. Richer optimizer: join ordering, multi-key sorted-index ORDER BY, broader cardinality estimation
12. More `CALL` procedures (starting with `db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()`); the index queries already compose with `YIELD`
13. `FOREACH`
14. Quantified path patterns

## Next steps

- Operational implications of the security and storage gaps: [Deployment](../operations/deployment.md), [Security](../operations/security.md)
- Measured impact of the performance items: [Benchmarks](../performance/benchmarks.md), [Performance Notes](../performance/notes.md)
- How change proposals are evaluated and landed: [Change Management](change-management.md)
- Evaluating whether the self-hosted core fits a production workload? Compare against the [LoraDB managed platform](https://loradb.com)
