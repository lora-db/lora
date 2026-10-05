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
| Type mismatch detection between comparable types | Accepted | Compared without error; `<`, `>`, `<=`, `>=` give `null`, as in Cypher | Low — 1 ignored test |
| Parameter as a label or relationship type | N/A | Not implemented | Low — not standard Cypher |
| Vector ANN execution is opt-in | N/A | Vector indexes use exact flat scans over the indexed scope by default; an HNSW index is built with ``OPTIONS {indexConfig: {`vector.indexProvider`: 'hnsw'}}`` (`lora-store/src/memory/hnsw.rs`, `tests/vector_index.rs`) | Low — flat is exact and fine for small corpora; choose HNSW for production-scale semantic retrieval |
| List-of-`VECTOR` as a property | Parsed | Rejected at write time (`PropertyConversionError::NestedVectorInList`) | Low — loud error; shape decision to keep future indexing viable |

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
| Constraint/index coverage is scoped | Observed | **Medium** — uniqueness, existence, type, key, RANGE, TEXT, POINT, LOOKUP, VECTOR, and FULLTEXT surfaces exist; composite multi-property seeks are still absent, and ANN vector execution (HNSW) is opt-in per index |
| Transaction isolation is conservative | Observed | **Medium** — auto-commit writes publish optimistically under a writer mutex; explicit read-write transactions (including interactive `db.begin()` transactions in the Node binding) hold the writer lock for their full lifetime, so other writers wait; read-only transactions pin snapshots |
| A second in-process open of a database directory shares the open engine | Observed | Low — both handles talk to one engine and one WAL writer, never two; a second *process* gets `LORA_LOCKED` |
| Node / relationship IDs are never reused | Observed | Low — `u64` counter will not overflow in practice |
| Tombstones and clone-heavy compatibility APIs | Observed | **Low–Medium** — deleted IDs leave slot gaps; hot executor paths use borrow closures, but `all_nodes()` and other record-returning scans still allocate |

---

## Query correctness

| Issue | Classification | Risk |
|-------|---------------|------|
| `toLower` / `toUpper` are not locale-aware | Observed | Low — Unicode case mapping is supported, locale-specific folding is host-side |
| `round()` returns an integer for integral results and rounds half away from zero | Observed | Low — Neo4j returns a float |
| Float comparison uses IEEE 754 | Observed | Low — `NaN != NaN` is standard |
| lora-graphql: a `@unique` (non-key) value held by a node the caller cannot read is reported as taken on create and update | Observed | Medium for fields whose use must stay private (an email). Keys are covered: a create under a hidden key answers as under a free one (G-20, `docs/design/graphql-threat-model.md`). Key such a field, or leave `@unique` off it |
| `<`, `<=`, `>`, `>=` between two temporals of different types (a `DATE` and a `DATETIME`) is an error, including when a RANGE index answers the predicate | Observed | Low. A deliberate divergence: Cypher gives `null`, which made `WHERE d >= date()` over `DATETIME` values drop every row silently (E-1). A query that relied on the `null` now fails and names both types; a guard earlier in the `AND` (`type.of(x) = 'DATE' AND …`) avoids the error, since `AND` short-circuits and an index scan hands other-kind values to the filter |
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
| Writers waiting on the Node pool | Fixed | A read-write `begin()` transaction waited for the writer lock on a libuv worker, and each of its calls went through one; with more waiting transactions than pool threads, the lock holder could not run its next statement and the process stopped. Interactive transactions now wait and run on their own threads and settle their promises from there (`crates/bindings/lora-node/src/interactive.rs`, `test/pool.test.ts`). Auto-commit and batched writes waiting for the lock still occupy a pool thread (a latency, not a deadlock) |
| Node transaction lifecycle | Fixed | After the pool fix, a command queued behind a failed statement, a commit or a rollback never settled and kept the process alive; `begin()` after `dispose()` leaked a deferred; `dispose()` joined an actor still waiting for the writer lock, deadlocking the JS thread when two handles shared a directory; and a mutating `stream()` opened on the JS thread while a transaction held the lock deadlocked it. Uncalled completions now settle as closed, only idle actors are joined, and mutating streams run on their own thread (`lora-node/src/actor.rs`, `src/stream.rs`, `test/pool.test.ts`) |
| Mutating streams across bindings | Fixed | A mutating stream holds the writer lock until it is drained or closed, and its lock guard must be released on the thread that took it. In lora-ffi (and so lora-go) and lora-python, streams were pulled and freed on any thread, and every binding but Node dropped the database before a stream borrowing it (use-after-free: SIGSEGV when a stream outlived its database). Mutating streams now live on their own thread and drop before the database (`lora-ffi/src/stream.rs`, `lora-python/src/stream.rs`). That thread sends rows in chunks (up to 256 rows or about 50 µs) and commits only when the consumer asks past the last row, via `QueryStream::pull`/`finish`; a stream read to its last row but closed before the end rolls back |
| Waiting for the writer lock with the host lock held | Fixed | lora-python `stream()`/`clear()` waited for the writer lock with the GIL held, and `AsyncDatabase.stream` on the event loop, so a second writer froze the thread that could finish the first (process hang). lora-ruby `clear`/`close` held the GVL (a stall). Both now release it (`lora-python/tests/test_concurrency.py`, `lora-ruby/test/test_writer_lock.rb`) |
| lora-wasm writes during a write stream | Fixed | A write started while a write stream held the writer lock panicked (`RuntimeError: unreachable`: wasm32's mutex cannot wait). It now fails fast with `LORA_TRANSACTION`; reads keep working (`lora-wasm/src/writer.rs`, `test/single-thread.test.ts`) |
| Parked mutating streams in fixed pools | Observed (caller contract) | The libraries never wait for another caller thread, but a host that parks an open mutating stream and queues its next pull behind writers in a fixed pool (a C thread pool, a saturated Python executor) can starve it. Documented in `lora_ffi.h`, the Go and Python READMEs |
| Go waiting writers | Observed | Each write waiting for the lock holds an OS thread, not a P, so writers cannot deadlock at `GOMAXPROCS=1`; more waiting writers than `debug.SetMaxThreads` abort the process (`lora-go/concurrency_test.go`) |
| lora-server slow requests | Observed | A current-thread runtime calling the engine synchronously cannot deadlock on the writer lock, but one slow query or admin snapshot blocks every request, `/health` included, with no server-side timeout (`lora-server/tests/concurrency.rs`) |
| Some predicates still scan | Observed | Vector similarity, regex, non-indexed properties, nested map paths, and unsupported composite seek shapes scan candidate records |
| Clone-heavy read API | Observed | Allocation overhead proportional to result set. Inside a query, a row slot holds a large value (a list, map or path with 8+ entries or a nested container, a long string, a binary or a vector) behind a shared pointer, so an operator that makes a row per candidate (expand, OPTIONAL MATCH, UNWIND, FOREACH, `CALL { }`, list constructs) no longer copies a list the row carries, and a lone variable argument (`size(big)`) is read in place (`tests/carried_value_sharing.rs`). A function with two or more arguments or an operator over the list still copies it per call |
| Query timeout coverage | Observed | Cooperative deadlines and cancellation cover eager, streaming and write execution in Rust and the Node binding (`execute`, `transaction`, `stream`, `begin`), including loops inside one expression (pattern/list comprehensions, `reduce`, list quantifiers): a three-level nested relationship filter once ran 5-15 s past a 1 s deadline on the buffered read path and returned rows; it now fails at the deadline (`lora-database/tests/timeouts.rs`). A timed-out scan frees the rows it built on the rayon pool, so the error is not delayed by the deallocation (a cartesian read used to return 2-3x past its deadline). A `stream()` checks between rows, so one blocking pull (a large aggregation) finishes before the check. HTTP and the other bindings do not expose timeouts yet |
| Optimizer is still local | Observed | Cost-based index selection exists for scan/filter sites, and a single-key `ORDER BY` over a range-indexed property with a range predicate streams from the index. WHERE conditions are pushed down to the scan of the variable they test (so `n.key = $k` and `n.key IN $list` seek on any node of a pattern), a chain starts from its cheaper end, and an unfiltered `count(n)` over one label reads the label count. `ORDER BY … LIMIT` (literal or parameter) keeps only `skip + limit` rows instead of sorting every match. No join ordering or global cardinality search; an index-ordered scan is not yet chosen when the filter is not selective |
| Staged writes share structure | Observed | Writes the in-place fast path cannot prove failure-free (relationship `CREATE`, `MERGE`, `SET` from an expression, anything with a deadline) run on a copy of the graph so a failure leaves nothing behind. The copy is structural: node and relationship slots, adjacency lists and label indexes are two-level copy-on-write radix trees (`ChunkedVec`), the index and constraint catalogs and every secondary index (hash, range, text, point, full-text) are shared and copied on write, and a posting list past 64 ids is a table of copy-on-write chunks. Cloning the graph is O(1): about 0.3 µs at 2M nodes / 8M relationships, against 218 µs when the slabs were flat chunk tables. A write copies only the path it touches. At that size a staged `SET` of an unindexed property takes about 11 µs, a `CREATE` on a constrained label about 24 µs and a relationship `CREATE` about 55 µs (240–300 µs before; `tests/write_scaling.rs`). Still O(N): a staged write that inserts into a hash property index copies the whole shard of that key it lands in (1/256 of the key's values), and one into a RANGE index copies the partition table (one pointer per ~1,000 values), so `graph_create_node` of a node with an indexed unique `id` takes about 200 µs at 2M nodes |

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

4. Expand timeout coverage to HTTP and the Python, Go, Ruby and WASM bindings (Rust and Node are covered)
5. Add authentication middleware to the HTTP server
6. Broader composite-index optimizer rewrites (ANN vector execution ships as the opt-in HNSW provider)
7. ~~Introduce borrowing iterators on `GraphStorage`~~ — partially addressed: `with_node` / `with_relationship` closures now cover the executor's hot paths without requiring `&NodeRecord` access from every backend. Still open: streaming iterators for bulk scans

### Long term (capability)

8. ~~Persistence (WAL and/or snapshots)~~ — partially addressed:
   snapshots ship across surfaces, WAL-backed opens ship on
   filesystem-backed surfaces, and Rust / `lora-server` expose explicit
   checkpoint/admin controls. Remaining work is operational polish,
   scheduled checkpoints, and richer multi-process/process-manager guidance.
9. Richer optimizer: join ordering, multi-key sorted-index ORDER BY, broader cardinality estimation
10. More `CALL` procedures (starting with `db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()`); the index queries already compose with `YIELD`
11. Quantified path patterns

## Next steps

- Operational implications of the security and storage gaps: [Deployment](../operations/deployment.md), [Security](../operations/security.md)
- Measured impact of the performance items: [Benchmarks](../performance/benchmarks.md), [Performance Notes](../performance/notes.md)
- How change proposals are evaluated and landed: [Change Management](change-management.md)
- Evaluating whether the self-hosted core fits a production workload? Compare against the [LoraDB managed platform](https://loradb.com)
