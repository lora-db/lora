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

## Engine — Cypher correctness

- [ ] P3 A map or list literal (`{l: big}`), a function with two or more arguments
      other than `coalesce`, and every returned row still copy a carried list: a
      `LoraValue` list owns its elements, so sharing one inside another value needs
      a shared list representation. Row clones, grouping, `DISTINCT` and `UNION`
      keys, comparisons, `coalesce`, `CASE`, quantifiers, comprehensions, `reduce`
      and a lone function argument share or read it in place (`value.rs`
      `SlotValue`, `eval/expr.rs` `eval_source`).
- [ ] P2 General procedures: `CALL db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()` ("unknown procedure").
- [ ] P3 A failing expression in a seek or range bound of a read is dropped, not raised (`MATCH (n {id: 1/0})` matches nothing): raw `eval_expr` in `lora-executor/src/executor/helpers.rs:432,713,1356` and `pull/scan.rs:413,703`. Writes raise since 2026-10-11.
- [ ] P3 Subtraction between values of unrelated types is `null`, not an error (`sub_values` fallback, `eval/binops.rs`); addition raises.
- [ ] P2 `UNION` inside `CALL {}`, `CALL (x) {}` scope clause, `IN TRANSACTIONS` (`analyzer/state.rs:209`).
- [ ] P3 Any `RETURN` inside `COUNT { }` or `EXISTS { }` is a parse error (`cypher.pest:372-374` takes one pattern plus `WHERE`); `COLLECT { }` subquery not in the grammar.
- [ ] P3 A bare pattern predicate in `WHERE` is a parse error (`WHERE NOT (p)-[:MANAGES]->()`, `where_clause.rs` `where_pattern_predicate_not_exists`); `EXISTS { }` works.
- [ ] P3 Quantified path patterns; inline `WHERE` in relationship and node patterns, single-hop included (`match.rs:1427,1455`; the test at `:1455` is not valid Cypher as written).
- [ ] P3 `EXISTS { (a)-[:R]->(b) WHERE … }` (a bare pattern followed by `WHERE`) is a parse error; `EXISTS { MATCH … WHERE … }` works.
- [ ] P3 After a plain projection, a bare `ORDER BY` key that is both an alias and an earlier variable (`RETURN n.name AS n ORDER BY n`) reads the earlier variable, where Cypher reads the alias. After an aggregating or `DISTINCT` projection it reads the alias (`analyzer/expressions.rs` `analyze_expr_with_aliases`).
- [ ] P3 Comparison type-mismatch errors (`errors.rs:536`, duplicated at `:749`): `1 < 'a'` is null.
- [ ] P3 `util.text.join` is an unknown function (`string.join` exists): alias it or delete the ignored test (`expressions.rs:1397`).
- [ ] P3 Standalone `CALL … YIELD` vector procedures reject `$param` option maps (`database/procedures.rs:308-323`); the in-query form accepts them.
- [ ] P3 3D geodesic distance ignores height; no WKT / CRS transform.

## Engine — performance

- [ ] P2 Composite multi-property seeks (`optimizer.rs:1085-1088`); composite RANGE indexes are catalog-only.
- [ ] P2 Index-ordered scan for an order-only or unbounded range (`ORDER BY key LIMIT k` over a bounded range already streams, `optimizer.rs:70-124`); count a range from the index.
- [ ] P2 Join ordering and global cardinality estimation (`estimatedRows` exist for scans and seeks, not for Expand, Filter, joins or aggregates, `database/explain.rs:61-110`).
- [ ] P2 Streaming iterators for relationship scans, multi-label scans and the write path (`helpers.rs:2613,2618`, `mutable.rs:1190`); node scans on the read path are paged (`scan_node_ids`).
- [ ] P2 Pattern subqueries (comprehensions, `EXISTS { }`, `COUNT { }`) are matched by the expression evaluator, not planned (`eval/expr.rs` `match_pattern_part`). An unbound node is sought for an inline map and for `=`, `IN` and `STARTS WITH` in the `WHERE`; a range comparison or any other predicate scans its label once per row, no statistics pick between two pinned ends, and `explain()` shows the seeks only as a `pattern_seeks` detail.
- [ ] P3 CSE and redundant-scan elimination (no pass exists).
- [ ] P3 Snapshot save/load benchmark (no `benches/snapshot.rs`).

## Engine — memory

- [ ] P1 A small node costs ~1 KB resident and the property map's capacity doubles at 4, 8 and 16 properties (a fifth integer costs ~270 B, not ~16 B). Target: a node with a key, a 16-char hash and two integers under ~300 B including its unique-index entry; maps sized to their properties; short repeated strings interned or inline (Festimap G-64, `apps/ingest/tools/probe-record-layouts.ts` in Festimap). Measure reads and writes before and after.

## Writer lock and durability

- [ ] P2 An auto-commit `execute()` write still waits for the writer lock on a libuv worker (`lora-node/src/tasks.rs`); it has to know it mutates before it is dispatched. `db.transaction()` write batches run on their own thread since 2026-10-12.
- [ ] P2 A transaction's lifetime is unbounded: one that is opened and never committed holds the writer lock until it is disposed. `begin()` bounds only the wait for the lock (`timeoutMs`, `signal`).
- [ ] P1 No online snapshot of a directory-backed graph: a copy taken while the owner writes is not a database, and the cross-process lock keeps a second process out. Wanted: `db.snapshot(targetDir)` (checkpoint plus a copy of the immutable files, or hard-linked segments plus the WAL tail) consistent with one committed transaction, readers and writers kept running, `lora snapshot <dir> <target>` for a closed directory, exposed through lora-graphql; `kill -9` during it must leave both valid (Festimap G-62; today a backup stops the container).
- [ ] P2 Managed checkpoints run inline on commit, under the writer mutex (`write_guard.rs:173-186`, `snapshot/store.rs:135-151`); move to a worker.
- [ ] P3 Time-based checkpoint scheduler (only `checkpoint_every_commits`).
- [ ] P3 Consolidate torn-tail detection (`lora-wal/src/segment.rs:335-343`, ADR 0004).
- [ ] P3 Binary vector-index trailer in snapshot v5 (JSON today, `lora-snapshot/src/format.rs:14-18`).
- [ ] P3 Remove the unused `LockTable` / `WriteSetLocks` and the dead `lock_table` field (`lora-store/src/lock_table.rs`, `database/mod.rs:114-118`, `builder.rs:239,258`): the single writer is the design.

## lora-graphql

- [ ] P3 Input validation, what is left (`docs/design/graphql-input-validation.md`): a `regexEngine` hook, normalising filter operands and lookup keys of a `@normalize`d field, mapping a third-party `@constraint` in `migrate neo4j`.
- [ ] P3 Triggers, what is left (`docs/design/graphql-triggers.md`): the relationship's properties in a `DISCONNECT` trigger, a `deleted` column for trigger and `@cypher` mutation statements (one that deletes stays broad), previous values in an `UPDATE` trigger, `lora.publish()` with custom `Subscription` fields for events that are not stored, a trigger and a worker scenario in `bench/load`, the notification flow in `examples/tutorial`.
- [ ] P3 A relationship property's rule that tests the relationship's ends is refused when a field using the property type points at an interface or union (`propertyEnds` in `src/model/build.ts`); rules on the relationship field itself hold per member.
- [ ] P2 Single-call mutations cover a create (with its nested creates and connects: `bench:load --scenario createConnect` at 32 in flight gave about 1,960 req/s with a 9.5 ms median against 1,540 and 13.5 with `LOAD_NO_SINGLE=1`, on a machine that was busy with other builds, so confirm on a quiet one) and an update of one node by key that sets or adjusts scalars and connects or creates related nodes (`--scenario connect`, same conditions: about 2,260 req/s with an 8.1 ms median against 1,830 and 11) (one `driver.run()` with `assert()` statements for the rules; at 32 in flight plain creates +24%/s with a 3.8 ms median against 6.4, updates +68%/s with 1.7 ms against 7.6). Still in a transaction, three hops or more under the lock: an update that disconnects, replaces a single relationship, deletes or updates connected nodes or edges (each reads back what it found), a create that names one relationship from both sides or sets a relationship property a rule refuses, an `adjust` of a field with a value rule (checked in JavaScript on what was stored), upserts, bulk updates and deletes, any schema with triggers, a schema with a READ validate rule (the payload read can refuse). Each needs its decision moved into an `assert()` statement (`src/execute/mutate/single-call.ts`, `runner.ts`); `test/single-call-differential.test.ts` holds both paths to the same answer.
- [ ] P3 `@populatedBy` callbacks of upserts, bulk updates and nested `update` entries still run inside the transaction, one at a time (`src/execute/mutate/runner.ts`, `#prepareRow`): the operation or key is known only after a read. A create's (nested and generated keys included) and a keyed update's run before the transaction begins, which is at its first statement.
- [ ] P3 `bench:load --subscribers N` load tests fan-out by key over HTTP (server-sent events). Not exercised: subscriptions with a `where` filter or SUBSCRIBE rules, where each event is tested by a statement, and `changeFeed: true`.
- [ ] P3 `check()` does not plan-check subscriptions (the statements that test a change for a subscriber). Queries are compiled and planned; mutations are run in a rolled-back transaction and planned.
- [ ] P2 Worker threads sharing one engine: confirm `bench:load --workers 0,1,2,4` on a quiet machine (the 2x for keyed reads was measured on a loaded one), then put the numbers in `limitations.md`. Serving from several threads is documented in `serving.md`.
- [ ] P3 `onWrite` listeners run synchronously on the request path (`src/subscriptions/change-hub.ts`).
- [ ] P3 S8 types for `@cypher` parameters in `compile` output.
- [ ] P3 Load-test `@authorization` rules: the bench schema has none, so it needs a ruled schema and `--claims` wired to it. `bench:load` has `createConnect`, `bulkUpdate` and `delete` scenarios (a first run at 32 in flight: 2,360 and 1,770 req/s for the last two); a cascading delete (`onDelete: CASCADE`) is not in the bench schema yet.
- [ ] P3 CI bench regression gate (>15%, like `scripts/check-bench-delta.mjs`) in `lora-graphql.yml`.
- [ ] P3 Ideas, not committed to: `lora-graphql introspect`; `@cypher` editor support via lora-query; Rust crate; GraphQL endpoint in lora-server; S5 write-set conflict reasoning.

## Server and bindings

- [ ] P1 HTTP server: no auth, TLS or rate limiting.
- [ ] P1 HTTP `/query` has no timeout/deadline; Python, Go and Ruby pass none either.
- [ ] P1 `lora-server` runs each query synchronously inside an async handler on a current-thread runtime (`main.rs:27`, `app/routes.rs:41`): one slow query blocks every request, `/health` included.
- [ ] P2 `tracing-subscriber` + configurable log level in `lora-server` (logs are invisible today).
- [ ] P2 Graceful SIGTERM shutdown in `lora-server`.
- [ ] P2 Query-size and result-size limits on HTTP (only axum's JSON body default).
- [ ] P2 Confirm on a quiet machine that chunked write streams close the Python/Go per-row gap to 0.18.0 (chunking is `729ffe13`); only 0.19.0 vs patched was measured.
- [ ] P3 Node: read streams via `ThreadsafeFunction`; column-major TypedArray decoding (`lora-node/src/stream.rs:3`).
- [ ] P3 lora-graph-canvas: internalise the kapsule renderer deps (`engines/3d-force-graph/kapsule.ts:11`).

## Tests

- [ ] P3 Property-based tests for queries and snapshot round-trips (no proptest/quickcheck).

## Code hygiene

- [ ] P3 `anyhow::Result` in public API (`lora-database/src/stream.rs:260,274,282`, `snapshot/json.rs:38`); every binding that calls streams changes with it.
- [ ] P3 Replace `smallvec = "2.0.0-alpha.12"` when 2.0 is stable.

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

- One writer at a time: no multi-writer commit, write-write conflict detection or group-fsync coordination (decided 2026-10-11; the phased plan was deleted). Revisit only if a write benchmark shows the single writer, not the lock hold time, is the limit.
- A non-key `@unique` value held by a hidden node reads as taken (threat model).
- No cross-process change feed; no GraphQL variant cap; lighter connection projection measured and left (6%).
- `LOAD CSV` and `USE <graph>` out of scope (host-side `import_rows` covers it).
- Admin routes have no auth/sandbox — managed-platform territory.
- Flat is the default vector index; HNSW is opt-in (revisit the default separately).
