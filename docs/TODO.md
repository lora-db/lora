# LoraDB — what's left to do

The one list of open work. Built 2026-09-30 by checking every plan, risk,
limitation and TODO in `docs/`, the root docs, the package READMEs and the
website against the code at `0544a1a` (plus the uncommitted E-4 follow-up). Only
items that are still true are here. The docs were corrected and the finished plans
deleted the same day, so the other docs describe what exists; this file is the only
place that lists what doesn't.

When an item ships, delete its line. Priorities: **P0** blocks users now, **P1**
next, **P2** planned, **P3** when convenient.

---

## P0 — ship now

- [ ] **Release 0.19.0** with the E-3 and E-4 fixes and their follow-ups (every
      binding's writer-lock audit, named time zones, the temporal constructors).
      `sync-versions.mjs` raises `@loradb/lora-graphql`'s `lora-node` peer with it.

## Engine — Cypher correctness

- [ ] P1 **E-4 class, remaining sites.** A value the row carries for other reasons is
      copied per candidate in OPTIONAL MATCH, expand + filter, variable-length expand,
      shortestPath / allShortestPaths, FOREACH, UNWIND followed by WITH/WHERE, nested
      comprehensions / reduce / quantifiers, and `CALL { }` without an importing WITH:
      with a 20k-element list carried, ~0.05 ms becomes ~25-90 ms. Sharing large values
      behind an `Arc` in row slots fixed all of them but cost 10-16% on hot paths
      (withdrawn; patch kept out of tree). Next: `LoraValue::List` as `Arc<[..]>`, or
      scope rows per construct as pattern subqueries do (`2390fbe`).
- [ ] P3 `size(big)` and other function arguments still copy a list per call.
- [ ] P2 `head(collect(x))` returns null (E16).
- [ ] P2 `7/2` returns `3.5`; integer division should give `3` (E22).
- [ ] P2 `[1,2,3][..-1]` returns `[]` (E23).
- [ ] P2 `max` over durations returns the smallest; `sum`/`avg` of durations return null (E25).
- [ ] P2 `min`/`max` of LocalDateTime/Time wrong (E27; sorting is fixed).
- [ ] P2 `CREATE (r:R) DELETE r` and `SET q:R, q.x = …` fail the existence check (E20 residual).
- [ ] P2 `stdev`/`stdevp`/`percentile*` silently ignore `DISTINCT` (`executor/helpers.rs:945+`). Reject or implement.
- [ ] P2 `5 IN [1, null]` returns `false`, Cypher says `null` (pinned at `where_clause.rs:886`). Fix, or document in the matrix §19.
- [ ] P2 No rollback on constraint violation (`invariants.rs:605`).
- [ ] P2 `list.sum` still wraps on integer overflow (returns `i64::MIN`); the operators error now. Make it error too.
- [ ] P2 HNSW `quantization: 'int8'` clips coordinates outside [-1, 1] silently (un-normalised vectors all score ≈1.0). Reject or normalise on insert.
- [ ] P2 General procedures: `CALL db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()` ("unknown procedure").
- [ ] P2 `UNION` inside `CALL {}`, `CALL (x) {}` scope clause, `IN TRANSACTIONS` (`analyzer/state.rs:209`).
- [ ] P3 `COUNT { … RETURN DISTINCT }` is a parse error (E24 residual); `COLLECT { }` subquery not in the grammar.
- [ ] P3 Quantified path patterns; inline `WHERE` in variable-length relationships (`match.rs:1427,1455`).
- [ ] P3 Comparison type-mismatch errors (`errors.rs:536`); parse-time parameter type checks (`parameters.rs:383`).
- [ ] P3 `'…'::DATE` / `::DURATION` cast syntax (the real gap behind `expressions.rs:1067,1073,1377`); `util.text.join` (`:1392`).
- [ ] P3 Path bindings in `CREATE` patterns not materialised (`executor/mutable.rs:1789`).
- [ ] P3 Vector procedure options reject `$param` maps (`database/procedures.rs:314-318`).
- [ ] P3 `SHOW FULLTEXT INDEXES` parses but always returns empty (`cypher.pest:77`).
- [ ] P3 3D geodesic distance ignores height; no WKT / CRS transform.

## Engine — performance

- [ ] P1 Top-k never applies to generated reads: the optimizer only plans it for a
      literal `LIMIT`, lora-graphql emits `LIMIT $pN` (`lora-compiler/src/optimizer.rs:1322-1357`).
- [ ] P1 Indexed posting lists are cloned whole on write — O(nodes sharing the value)
      per write (`lora-store/src/memory/id_set.rs`, whole-bucket COW).
- [ ] P2 Composite multi-property seeks (`optimizer.rs:975`); composite RANGE indexes are catalog-only (`:484+`).
- [ ] P2 Index-ordered scan when the filter isn't selective; count a range from the index.
- [ ] P2 Join ordering and global cardinality estimation (`estimatedRows` exist only for seeks — E11).
- [ ] P2 Streaming iterators for bulk scans (`all_nodes()` / `nodes_by_label()` return `Vec`, `lora-store/src/traits.rs:161,168`).
- [ ] P3 CSE and redundant-scan elimination. `remove_redundant_limit` (`optimizer.rs:126`) is an empty placeholder — implement or delete.
- [ ] P3 Snapshot save/load benchmark (no `benches/snapshot.rs`).

## Concurrency and durability (`docs/design/concurrency-implementation-plan.md`)

- [ ] P1 Auto-commit and batched writes still hold a libuv thread while waiting for the writer lock (latency, not deadlock).
- [ ] P1 `begin()` takes no `timeoutMs`/`signal` — lock wait and transaction lifetime are unbounded (`lora-node/ts/index.ts:889`, lora-graphql `src/driver.ts:190`).
- [ ] P2 Phase 1 — define snapshot isolation and write-write conflict semantics, with tests (none exist).
- [ ] P2 Phase 2 — the "write attempt" object (rows, events, write-set, base snapshot); staging itself is done (`database/occ.rs:55-69`).
- [ ] P2 Phase 3 — fine-grained commit; `lock_table` is dead code (`database/mod.rs:113-117`).
- [ ] P2 Phase 4 — concurrent-safe ID allocation (`memory/graph.rs:48-49`).
- [ ] P2 Phase 5 — `FsyncCoord` / GroupSync waiter batching (`commit_tx_lsn` and the `bg_failure` latch exist).
- [ ] P2 Phase 6 — managed checkpoints run inline on commit (`write_guard.rs:178-184`); move to a worker.
- [ ] P2 Phase 7 — stress tests + fallback flag (after 1–6).
- [ ] P3 Time-based checkpoint scheduler (only `checkpoint_every_commits`).
- [ ] P3 Consolidate torn-tail detection (`lora-wal/src/segment.rs:335-343`, ADR 0004).
- [ ] P3 Binary vector-index trailer in snapshot v5 (JSON today, `lora-snapshot/src/format.rs:15`).

## lora-graphql

- [ ] P1 Mutations make several JS round trips under the writer lock — use `tx.executeMany`, fold validation and read-back into the last write (no `executeMany` in `src/`).
- [ ] P1 Bounded write queue: fail fast with `OVERLOADED`/503.
- [ ] P1 `maxCost` counts projected rows only — charge scans, `totalCount` and aggregates (from `explain()`/`analyze()`).
- [ ] P2 Compile cache: keyed lookups miss every request because the key variable is in the cache key; rebind variables into parameter slots the way `$jwt` claims are (`src/compile/cache.ts`).
- [ ] P2 Resolve `@populatedBy` callbacks in parallel before `begin()` (`src/execute/mutate.ts:1238`).
- [ ] P2 Batch a delete's neighbour lookup with `UNWIND` (`mutate.ts:1624,1670`).
- [ ] P2 Add a subscription scenario to `bench/load` (fan-out is indexed by type+key but not load tested over HTTP).
- [ ] P2 `check()` plan-checks queries only — extend to mutations and subscriptions.
- [ ] P2 `@cypher` `cost` argument; document its per-row semantics (`src/model/directives.ts:196`).
- [ ] P2 Schema lint: warn on low-cardinality inferred indexes (enum, boolean, `@default`).
- [ ] P2 Opt-in `totalCount` cap.
- [ ] P2 Cursor HMAC: use `node:crypto.createHmac`, precompute per secret (`src/compile/cursor.ts:82`).
- [ ] P2 Worker threads sharing one engine: confirm `bench:load --workers 0,1,2,4` on a quiet machine; document multi-worker serving (change feed for cache invalidation).
- [ ] P3 Document cache is FIFO, not LRU; `onWrite` listeners run synchronously on the request path (`src/lora-graphql.ts:986`).
- [ ] P3 S8 types for `@cypher` parameters in `compile` output.
- [ ] P3 Load-test `@authorization` rules, cascading and bulk mutations.
- [ ] P3 CI bench regression gate (>15%, like `scripts/check-bench-delta.mjs`) in `lora-graphql.yml`.
- [ ] P3 Ideas, not committed to: `lora-graphql introspect`; `@cypher` editor support via lora-query; Rust crate; GraphQL endpoint in lora-server; S5 write-set conflict reasoning.

## Server and bindings

- [ ] P1 HTTP server: no auth, TLS or rate limiting.
- [ ] P1 HTTP `/query` has no timeout/deadline; Python, Go and Ruby pass none either.
- [ ] P2 `tracing-subscriber` + configurable log level in `lora-server` (logs are invisible today).
- [ ] P2 Graceful SIGTERM shutdown in `lora-server`.
- [ ] P2 Query-size and result-size limits on HTTP (only axum's JSON body default).
- [ ] P3 Node: read streams via `ThreadsafeFunction`; column-major TypedArray decoding (`lora-node/src/stream.rs:3`).
- [ ] P3 lora-query: delete the dead `fallbackFromDocOverrides` ("TODO: enable after yarn build:wasm", `packages/lora-query/src/cypher/data.ts:429`) — the WASM already exports `builtins()`.
- [ ] P3 lora-graph-canvas: internalise the kapsule renderer deps (`engines/3d-force-graph/kapsule.ts:11`).

## Tests

- [ ] P1 **Triage the 59 `#[ignore]`d tests.** 29 now pass (e.g. `where_exists_negated`,
      `date_function`, `match_shortest_path`, `create_index`) — un-ignore them. Of the
      rest, fix outdated syntax or give each a specific reason; the support matrix calls
      these tests its source of truth.
- [ ] P3 Integration test for integer-overflow errors (`eval/binops.rs:290,424`).
- [ ] P3 Property-based tests for queries and snapshot round-trips (no proptest/quickcheck).

## Code hygiene

- [ ] P3 Remove the never-constructed `UnknownLabel` / `UnknownRelationshipType` / `UnknownProperty` (and probably `UnknownPropertyAt`) errors (`analyzer/errors.rs:12-21`) — the analyzer stopped reading the graph in `d004ba4e`.
- [ ] P3 Error-style violations: six `{node_id:?}` messages (`lora-executor/src/errors.rs:8-23`), `'{crs}'` quoting (`spatial/srid.rs:67`), `anyhow::Result` in public API (`stream.rs:255`, `snapshot/json.rs:38`).
- [ ] P3 Stale ArcSwap/CAS comments in `benches/concurrent.rs:4,35,88,102` (the store is `RwLock<Arc>`).
- [ ] P3 Replace `smallvec = "2.0.0-alpha.12"` when 2.0 is stable.
- [ ] P3 Fix the `temporal.rs:1-9` doc comment (says all tests are ignored; none are).

## Release and tooling

- [ ] P2 Re-run `comparisons/` and resync `apps/loradb.com/src/lib/benchmarks/data.js` (data is from May; the public /benchmarks page quotes it).
- [ ] P3 Unshipped release items (`RELEASING.md` "not done yet"): code signing, reproducible builds, Homebrew/winget/apt, musl + ARM Windows server targets, bench gating, a committed CHANGELOG (release notes exist only as the git-cliff release asset); crates.io OIDC once supported.

---

## Docs

- [ ] P3 Once the findings in `docs/design/graphql-load-audit.md` are worked off (they're
      all in this file), strip its "Recommended order" and keep it as a measurement reference.
- [ ] P3 When an item above ships, also update the doc that states the limitation — mainly
      `docs/reference/cypher-support-matrix.md`, `apps/loradb.com/docs/limitations.md`
      and `docs/design/known-risks.md`.

## Accepted — not to-dos

- A non-key `@unique` value held by a hidden node reads as taken (threat model).
- No cross-process change feed; no GraphQL variant cap; lighter connection projection measured and left (6%).
- `LOAD CSV` and `USE <graph>` out of scope (host-side `import_rows` covers it).
- Admin routes have no auth/sandbox — managed-platform territory.
- Flat is the default vector index; HNSW is opt-in (revisit the default separately).
