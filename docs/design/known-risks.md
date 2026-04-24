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
| `CALL` (standalone) | Parsed to AST | Analyzer returns `SemanticError::UnsupportedFeature` | Low — clear error |
| `CALL ... YIELD` (in-query) | Parsed to AST | Analyzer returns `SemanticError::UnsupportedFeature` | Low — clear error |
| `FOREACH` | Not in grammar | N/A | Low — parse error |
| `CREATE INDEX` / `DROP INDEX` | Not in grammar | N/A | Low |
| `CREATE CONSTRAINT` | Not in grammar | N/A | Low |
| `LOAD CSV` | Not in grammar | N/A | Low |
| `USE <graph>` (multi-database) | Not in grammar | N/A | Low |
| `PROFILE` | Not in grammar | N/A | Low |
| Quantified path patterns | Not in grammar | N/A | Low — future openCypher syntax |
| Inline `WHERE` inside variable-length relationship | Parsed | Not evaluated | Low — 1 ignored test |
| Type mismatch detection between comparable types | Accepted | Compared without error | Low — 1 ignored test |
| Parameter as a label or relationship type | N/A | Not implemented | Low — not standard Cypher |
| HTTP `POST /query` with a `params` field | N/A | Not wired up | **Medium** — Rust API supports parameters, HTTP layer does not |
| Vector indexes / approximate kNN | N/A | Not implemented — `vector.similarity.*` / `vector_distance` run as exhaustive scans | **Medium** — fine for demos and small corpora; blocks production-scale semantic retrieval |
| List-of-`VECTOR` as a property | Parsed | Rejected at write time (`PropertyConversionError::NestedVectorInList`) | Low — loud error; shape decision to keep future indexing viable |

### Implemented since initial audit

The following features were listed as gaps in earlier revisions of this document but are now implemented and verified by tests:

| Feature | Evidence |
|---------|----------|
| Temporal types (`Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`, `Duration`) | 89 passing tests in `tests/temporal.rs` |
| Spatial types (`Point`: Cartesian 2D/3D and WGS-84 2D/3D — SRIDs 7203, 9157, 4326, 4979) | Tests in `tests/functions_extended.rs` and `tests/types_advanced.rs` |
| `VECTOR` value type (six coordinate tags; `vector()` constructor; property storage on nodes and relationships; similarity / distance / norm functions; exhaustive kNN via `ORDER BY … LIMIT k`) | Tests in `tests/vectors.rs` |
| `shortestPath()` / `allShortestPaths()` | Tests in `tests/paths.rs` |
| Advanced aggregates: `stdev`, `stdevp`, `percentileCont`, `percentileDisc` | Tests in `tests/aggregation.rs` |
| Trigonometry: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `degrees`, `radians` | Tests in `tests/functions_extended.rs` |
| `UNION` / `UNION ALL` | 59 passing tests in `tests/union.rs` |
| Variable-length paths (`*`, `*1..3`, `*0..1`, …) | Tests in `tests/paths.rs` |
| `EXISTS { pattern }` subquery | Tests in `tests/where_clause.rs` and `tests/expressions.rs` |
| Pattern comprehension | Tests in `tests/expressions.rs` |
| Map projection | Tests in `tests/projection.rs` |
| Parameter binding (`$name`, `$1`) via the Rust API | Tests in `tests/parameters.rs` |

---

## Storage and data integrity

> 🚀 **Production note** — The gaps in this section (persistence, uniqueness, indexes, transaction isolation) are by design for the in-memory core. They are addressed in the [LoraDB managed platform](https://loradb.com) — use the table below to decide whether a self-hosted deployment is viable, or whether the managed option is the better starting point.

| Gap | Classification | Risk |
|-----|---------------|------|
| No persistence — all data is lost on process exit | Observed | **High** for any non-ephemeral use case |
| No uniqueness constraints | Observed | **Medium** — duplicate data can be created silently |
| No property indexes | Observed | **Medium** — property filters are scans (filtered by label when possible) |
| No transaction isolation beyond single-query atomicity | Observed | **Medium** — global mutex serializes everything |
| Node / relationship IDs are never reused | Observed | Low — `u64` counter will not overflow in practice |
| `BTreeMap` cloning on bulk reads | Observed | **Low–Medium** — hot executor paths now go through `with_node` / `with_relationship` closures (zero-clone on in-memory); `all_nodes()` and other record-returning scans still allocate |

---

## Query correctness

| Issue | Classification | Risk |
|-------|---------------|------|
| `toLower` / `toUpper` use ASCII only | Observed | **Medium** — non-ASCII strings are unchanged |
| `normalize` is a no-op placeholder (no Unicode NFC) | Observed | Low |
| Integer overflow not explicitly handled | Inferred | Low — Rust panics in debug, wraps in release |
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
| Global `Mutex` held for the whole query | Observed | No concurrent reads; writes block everything |
| Full scans for property filters without a label | Observed | `O(n)` per property-filtered MATCH |
| Clone-heavy read API | Observed | Allocation overhead proportional to result set |
| No query timeout | Inferred | A pathological query can block the mutex indefinitely |
| Optimizer has only filter push-down | Observed | No join ordering, no index selection, no cardinality estimation |

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

1. Wire HTTP `params` body field through to `Database::execute_with_params`
2. Add `tracing-subscriber` so the existing `tracing` instrumentation produces output
3. Make bind address / port / log level configurable
4. Add query length and result-size limits in the HTTP layer

### Medium term (robustness)

5. Replace `Mutex` with `RwLock` so reads can run concurrently
6. Add authentication middleware to the HTTP server
7. Add property indexes for common filters
8. ~~Introduce borrowing iterators on `GraphStorage`~~ — partially addressed: `with_node` / `with_relationship` closures now cover the executor's hot paths without requiring `&NodeRecord` access from every backend. Still open: streaming iterators for bulk scans

### Long term (capability)

10. Persistence (WAL and/or snapshots)
11. Richer optimizer: join ordering, limit push-down, index selection
12. `CALL` / procedures (starting with `db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()`)
13. `FOREACH`
14. Quantified path patterns

## Next steps

- Operational implications of the security and storage gaps: [Deployment](../operations/deployment.md), [Security](../operations/security.md)
- Measured impact of the performance items: [Benchmarks](../performance/benchmarks.md), [Performance Notes](../performance/notes.md)
- How change proposals are evaluated and landed: [Change Management](change-management.md)
- Evaluating whether the self-hosted core fits a production workload? Compare against the [LoraDB managed platform](https://loradb.com)
