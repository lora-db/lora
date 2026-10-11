# Implementation plan: the open list and triggers

Date: 2026-10-11. Status: plan, nothing built. Written against `main` at
`5972d65` (v0.24.2).

This plan orders every open line of `docs/TODO.md` and the new triggers
and workers feature (`docs/design/graphql-triggers.md`) into milestones.
`docs/TODO.md` stays the one list of open work: this file says in what
order and why, and is deleted when the list is worked off.

Every line of the list was checked against the code on 2026-10-11 before
it was placed. **Observed** was read in the code or run as a probe
against the built Node binding, **Inferred** was deduced, **Unverified**
could not be checked without a build or a quiet machine.

Sizes: **S** under half a day, **M** one to three days, **L** a week or
more.

## How each item ships

- One TODO line, one commit (conventional commits, `fix(executor): ...`),
  with its regression test in the same commit.
- The commit deletes the TODO line and updates the doc that states the
  limitation: `docs/reference/cypher-support-matrix.md`,
  `apps/loradb.com/docs/limitations.md`, `docs/design/known-risks.md`.
- Tests run in the dev profile. The pre-commit hook stays lint only; CI
  owns the suites.
- A performance claim needs a before and after from the same machine
  (`bench:load` for lora-graphql, `benches/` for the engine).
- The docs site deploys on every push to `main`, so user-facing docs for
  an unreleased feature land with the release commit.

## Progress

Updated 2026-10-11. Work is on the branch `chore/todo-milestone-0`,
uncommitted.

- **Milestone 0: done**, except `util.text.join` (alias or delete) and
  `smallvec` (blocked upstream).
- **Milestone 1: done.** It surfaced one more gap, fixed with it:
  `LOCAL_DATETIME`, `TIME` and `LOCAL_TIME` had no `DURATION` arithmetic,
  and a write using it had been dropping the property in silence.
- **Milestone 2: done except one item.** lora-graphql: `executeMany`
  in the driver, batched statements, `@populatedBy` before `begin()`,
  `maxQueuedWrites` and `OVERLOADED`. Engine and Node: `begin()` takes
  `timeoutMs` and `signal`, an auto-commit write stops waiting for the
  lock at its deadline. Open: mutating auto-commit calls still wait on
  a libuv worker (line 45), a transaction lifetime cap, and a
  `bench:load` before and after.
- **Milestone 4: two small items done** (cursor HMAC through
  `node:crypto`, LRU document cache). The rest is open.
- **Milestone 3: built**, phases 1 to 4 and the docs page (`@trigger`
  statements, trigger functions with veto and a time limit, `@outbox`
  and `lora.worker()`). Open: the tutorial project, `bench/load`
  scenarios, the threat-model entry, `check()` for mutation and
  subscription plans (line 71, not folded in after all).
- **Decided since the plan was written:** breaking changes ship as
  breaks; one writer at a time is the design (milestone 8 shrank).

## Order at a glance

| # | Milestone | Why here | Rough effort |
| --- | --- | --- | --- |
| 0 | True up the list, free wins | Half a dozen lines are stale; cheap items clear noise | 1 week |
| 1 | Write correctness in the engine | Silent wrong writes; triggers add hand-written writes | 1 week |
| 2 | The writer-lock path | All remaining P1s of lora-graphql; reshapes the code triggers plug into | 2 weeks |
| 3 | Triggers and workers | The new feature, on the reshaped write path | 3 to 4 weeks |
| 4 | Cost and limits in lora-graphql | Last lora-graphql P1, plus its P2 neighbours | 1.5 weeks |
| 5 | Schema features and tooling in lora-graphql | P2 and P3 remainder | 3 weeks |
| 6 | Server and bindings | P1 server gaps, independent of the rest | 3 to 4 weeks |
| 7 | Engine features and performance | P2 and P3, largest items last | 8 to 10 weeks |
| 8 | Durability leftovers | Small and independent | 1.5 weeks |
| 9 | Release and measurement | After the performance work it measures | 2 weeks |

Milestones 0 to 3 are sequential. After that there are four tracks that
do not share files and can run side by side: lora-graphql (4, 5), server
and bindings (6), engine (7), durability (8).

## Milestone 0. True up the list, free wins

**Lines to delete or reword (no code):**

| TODO line | Finding |
| --- | --- |
| 31 `SHOW FULLTEXT INDEXES` | Works (Observed, `lora-database/src/database/schema.rs:228`, test `fulltext_index.rs:78`). Delete; fix the comment at `cypher.pest:76-77`. |
| 29 cast syntax | `::DATE`, `::DURATION`, `::DATETIME` work (Observed, probe). What is left: un-ignore three tests (`expressions.rs:1072, 1078, 1382`) and decide `util.text.join` (`string.join` exists). |
| 69 delete neighbour lookup | **Real, and fixed 2026-10-11.** A first reading said it was batched; a statement-count test showed one lookup per key (34 statements for 8 nodes against 13 for 1). Now one `UNWIND` statement per node type, folded with the relationship-id lookup (`src/execute/mutate/delete.ts`, test in `mutation-plans.test.ts`). |
| 64 `tx.executeMany` | The binding has it (Observed, `lora-node/ts/index.ts:1187`, `src/lib.rs:593`). The work is only in lora-graphql. |
| 46 `begin()` limits | lora-graphql bounds the lock wait (`driver.ts:218-270`). Open: lora-node `begin()` options, and a transaction lifetime cap everywhere. |
| 85 JS number | The range comes from napi's serde conversion, not `json.rs:48-62`. |
| 98 ignored tests | 60, not 59: 54 feature tests, 3 slow by design, 3 probes. |
| 107 `Unknown*` errors | `UnknownPropertyAt` is live (`analyzer/expressions.rs:51, 156`) and stays. |
| 26, 27, 28, 30 | Wider or narrower than written: any `RETURN` in `COUNT {}` or `EXISTS {}` fails; single-hop inline `WHERE` is unparsed too; a parse-time parameter type check has no types to check; only standalone `CALL ... YIELD` rejects `$param` options. |
| 36 to 39 | Line numbers stale; 37 and 39 are half done (ordered range scan with a bound; paged node scans on the read path). |
| 91 | Commit hash cites the wrong change (the chunking is `729ffe13`). |

**Add to the list (found while verifying, not on it):**

- An auto-commit write with `timeoutMs` does not bound its wait for the
  writer lock (`write_guard.rs:109-113, 229`). Goes with milestone 2.
- `lora-server` runs the synchronous query inside an async handler on a
  current-thread runtime (`main.rs:27`, `routes.rs:41`): one slow query
  blocks every request, `/health` included. Goes with milestone 6.

**Free wins (all S unless noted):**

- Triage the 60 ignored tests (M, line 98). Un-ignore the ones that
  pass, give each of the rest a specific reason, give the 3 bare probes
  a reason string, rewrite `match.rs:1455` (not valid Cypher). The
  support matrix is corrected from the result.
- Code hygiene, lines 107 to 111: remove three `Unknown*` errors, fix the
  error-style messages (`lora-executor/src/errors.rs:8-23`,
  `srid.rs:53, :67`), fix stale comments (`benches/concurrent.rs`,
  `temporal.rs:7-9`, `occ.rs:1-12`, `explain.rs:73-76`, the orphaned doc
  comment at `lora-node/ts/index.ts:869`). The `anyhow::Result` in the
  public stream API touches every binding that calls streams: it waits
  for milestone 8 only to change those call sites once, not because it
  is a break. `smallvec` 2.0 is blocked upstream.
- Delete the empty `remove_redundant_limit` (`optimizer.rs:126`, line 40).
  Real CSE moves to milestone 7.
- lora-query: delete `fallbackFromDocOverrides` (line 93) after
  confirming the built WASM exports `builtins()`.
- Integration tests for `+`, `-`, `*` integer overflow (line 102) in
  `expression_semantics.rs`.

## Milestone 1. Write correctness in the engine

Done first because these make stored data wrong without an error, and
because milestone 3 adds hand-written writes (trigger statements) whose
silent failure would be invisible.

| Item | TODO | Where | Size |
| --- | --- | --- | --- |
| A failing expression inside a write raises | 17 | `lora-executor/src/executor/mutable.rs`, 11 `eval_expr` sites to `eval_expr_result`; check the seek sites in `executor/helpers.rs` | M |
| The auto-commit fast path keeps no writes of a failing statement | 18 | `lora-database/src/database/occ.rs`: `live_fast_plan_safe` takes the fast path only when every expression in the plan is infallible | S |
| A JS number above 2^32 stays an INTEGER | 85 | `lora-node/src/json.rs` float branch; check whether lora-wasm shares it | S |
| `=~` with a bad pattern is an error, and the failure is cached | 19 | `lora-executor/src/eval/regex.rs::full_match` | S |
| HNSW int8 normalises before quantising | 23 | `lora-store/src/memory/hnsw.rs::quantize_to_int8`, insert and query | S |

- 17 and 18 ship together: once writes raise, more plans can fail
  midway, and the fast path must not keep half of them. Tests in
  `statement_atomicity.rs`, which already runs each case on all three
  write paths.
- **This is a breaking change, shipped as one.** Writes that succeeded
  silently will error, with no flag. It goes in the release notes, and
  `docs/design/graphql-input-validation.md` (which found it) is updated.
- Check `benches/perf_smoke.rs` and `tests/write_scaling.rs`: more plans
  take the staged path.
- Unverified: whether int8 vectors are persisted quantised in the
  snapshot trailer. If so, an index rebuild note is needed.

## Milestone 2. The writer-lock path

All of it is about how long the single writer lock is held and how
callers wait for it. It rewrites the commit path of
`execute/mutate.ts` and `mutate/runner.ts`, which is why triggers come
after it and not before.

**Engine and Node binding:**

| Item | TODO | Where | Size |
| --- | --- | --- | --- |
| `begin(mode, { timeoutMs, signal })` and a lifetime cap | 46 | `lora-database/src/database/stream.rs` (deadline variant; the try-lock loop exists in `write_guard.rs:132-171`), `lora-node/src/lib.rs:495`, `src/interactive.rs:106`, `ts/index.ts:889` | M |
| Auto-commit lock wait honours `timeoutMs` | new | `write_guard.rs:109-113, 229` | S |
| Mutating `execute` / `transaction` / `clear` leave the libuv pool | 45 | `lora-node/src/tasks.rs`, actor pattern from `src/interactive.rs` | M |

**lora-graphql:**

| Item | TODO | Where | Size |
| --- | --- | --- | --- |
| `DriverTransaction.executeMany`, feature-detected | 64 | `src/driver.ts:51-57, 119-128, 198-204` | S |
| The runner sends a mutation's statements in batches; validation and read-back fold into the last write | 64 | `src/execute/mutate/runner.ts`, `plan.ts`, `mutate.ts` | M to L |
| `@populatedBy` callbacks resolved in parallel before `begin()` | 68 | `src/execute/mutate/plan.ts:226`, `runner.ts:812` | S to M |
| Bounded write queue, `OVERLOADED` and 503 | 65 | new semaphore around `driver.begin` in `src/lora-graphql.ts`; new code in `src/errors.ts`; `maxQueuedWrites` option | S to M |
| Drop `bounded()` once `begin` takes limits | 46 | `src/driver.ts:218-277` | S |

- Order: `executeMany` in the driver, then the runner, then the queue.
  `@populatedBy` is independent.
- A statement that depends on the rows of an earlier one cannot share a
  batch. The runner batches between such points; the plan for where
  those points are is the first task (Inferred: creates, connects and
  the final read-back batch well; visibility checks do not).
- Measure with `yarn bench:load` before and after, and replace the
  numbers in `docs/design/graphql-load-audit.md` finding 4.
- Tests: `mutation-plans.test.ts` (statement counts per mutation),
  `driver.test.ts`, `error-codes.test.ts`, lora-node `timeouts.test.ts`
  and `locking.test.ts`.

## Milestone 3. Triggers and workers

The design is `docs/design/graphql-triggers.md`. Its five phases run
here unchanged, with three TODO lines folded in where they touch the
same code:

| Phase | Contents | Folded in |
| --- | --- | --- |
| 1 | `@trigger` statements for CREATE and UPDATE; startup plan check; write-set merge | Line 71: `check()` plan-checks mutations and subscriptions, since trigger statements need the same pass |
| 2 | DELETE triggers at the `beforeDelete` point | |
| 3 | JavaScript triggers, veto, `triggerTimeoutMs` | |
| 4 | `@outbox` types and `lora.worker()` | Line 77, second half: change listeners leave the request path (the worker registers on the same hub). Line 70: a subscription scenario and a trigger scenario in `bench/load` |
| 5 | Docs page with the notification flow as a worked example, threat model, tutorial | |

- Trigger statements join the mutation's last `executeMany` batch from
  milestone 2, so a trigger costs no extra round trip under the lock.
- Milestone 1 is a hard prerequisite: a trigger whose `SET` silently
  drops a failing expression would be undebuggable.

## Milestone 4. Cost and limits in lora-graphql

| Item | TODO | Where | Size |
| --- | --- | --- | --- |
| `maxCost` charges scans, `totalCount` and aggregates | 66 (P1) | `src/compile/cost.ts`, `compile/read/connection.ts:244`, `compile/aggregate.ts`; numbers from `analyze()` statistics | M |
| Opt-in `totalCount` cap | 74 | `compile/read/connection.ts`, option in `src/options.ts` | S |
| `@cypher(cost:)` and its per-row meaning | 72 | `src/model/directives.ts:284`, `model/build/cypher-field.ts`, `compile/cost.ts` | S to M |
| Keyed lookups hit the compile cache | 67 | `src/compile/cache.ts`: bind the key variable as a slot, as `$jwt` claims are | M |
| Cursor HMAC through `node:crypto` | 75 | `src/compile/cursor.ts:82`; keep `compile/hmac.ts` as the browser and WASM fallback; precompute the key per secret | S |
| Document cache is LRU | 77, first half | `src/runtime/documents.ts:167` | S |

- 66 does not wait for the engine's cardinality work (milestone 7). It
  uses the statistics `analyze()` already has; better engine estimates
  improve it later.
- Tests: `cost-limits.test.ts`, `compile-cache.test.ts`,
  `search-total-count.test.ts`. Cache and HMAC changes need a
  `yarn bench` before and after.

## Milestone 5. Schema features and tooling in lora-graphql

| Item | TODO | Size | Note |
| --- | --- | --- | --- |
| Relationship rules on interface and union targets | 62 | M to L | Refusal lives in `model/build/rule-checks.ts`; size Unverified until the compile side is read |
| FULLTEXT says when an index skips a value type | 63 | S to M | A lint in `analyze/lint.ts` plus a docs note |
| `regexEngine` hook | 60 | S | After the engine `=~` fix of milestone 1 |
| Normalise filter operands and lookup keys of a `@normalize`d field | 60 | M | `compile/filter.ts`, `compile/read/by-keys.ts` |
| Relationship properties in the stored-value scan | 60 | S | `analyze/stored.ts` |
| Map a third-party `@constraint` in `migrate neo4j`; `describeConstraints` | 60 | S | `src/migrate.ts` |
| Lint low-cardinality inferred indexes | 73 | S | `analyze/lint.ts`, `analyze/indexes.ts` |
| Types for `@cypher` parameters in `compile` output | 78 | M | `src/codegen.ts` |
| Load tests for `@authorization`, cascades, bulk mutations | 79 | M | `bench/load/scenarios.ts` |
| CI bench regression gate | 80 | S to M | `.github/workflows/lora-graphql.yml`, modelled on `scripts/check-bench-delta.mjs` |
| Worker threads on one engine: measure and document | 76 | S | Needs a quiet machine; include a `lora.worker()` claim test from milestone 3 |

The ideas on line 81 stay ideas.

## Milestone 6. Server and bindings

Independent of milestones 2 to 5. Order inside it matters:

1. **Queries leave the async thread** (new, S to M): `spawn_blocking`
   or a multi-thread runtime in `lora-server/src/main.rs:27`. Timeouts
   and graceful shutdown are meaningless before this.
2. **`/query` takes `timeoutMs`, with a server default** (line 87, S to
   M): a deadline method on `QueryRunner` (`database/mod.rs:46-76`), a
   field on `QueryRequest` (`app/types.rs:5-11`). The 408 mapping
   exists.
3. **Logs** (line 88, S): `tracing-subscriber` and a level in config.
4. **Graceful SIGTERM** with a final `db.sync()` (line 89, S).
5. **Limits** (line 90): request body S, result cap M.
6. **Auth, rate limiting, TLS** (line 86): bearer token S, rate limit S
   to M, TLS M.
7. **Timeouts in Python, Go and Ruby** (line 87, M): one FFI function
   in `lora-ffi`, three wrappers.
8. **Node read streams through a `ThreadsafeFunction`, column-major
   decoding** (line 92, L). Last: it shares the buffer format with
   `shared-ts/decode.ts`.
9. **lora-graph-canvas: internalise `three-forcegraph` and
   `three-render-objects`** (line 94, L).
10. **Measure the Python and Go write-stream gap** (line 91, S, quiet
    machine).

Tests: `lora-server/tests/http.rs`, `admin.rs`, `concurrency.rs`; each
binding's own suite.

## Milestone 7. Engine features and performance

Grouped so that items sharing machinery are built once.

**Features:**

| Group | TODO | Size | Note |
| --- | --- | --- | --- |
| `db.labels()`, `db.relationshipTypes()`, `db.propertyKeys()`; standalone `CALL` routed through the in-query path; `$param` vector options | 24, 30 | M | The store already has the three listings (`traits.rs:133-134, 417`). Routing standalone `CALL` through the in-query rewrite fixes line 30 for free |
| `CALL (x) { }` scope clause | 25 | S to M | Grammar and analyzer |
| `UNION` inside `CALL { }`; `RETURN` inside `COUNT { }` and `EXISTS { }`; `COLLECT { }` | 25, 26 | M to L | One piece of work: a real subquery expression reusing the CallSubquery plan |
| Inline `WHERE` in relationship patterns | 27 | M | Per-hop filter in `pull/expand.rs` |
| Quantified path patterns | 27 | L | Last of the features |
| Comparison type mismatch | 28 | S or M | A runtime error would break queries over mixed-type properties. Either a static literal-versus-literal check, or drop the line |
| 3D distance uses height; WKT input | 32 | S, S to M | Height changes returned distances |
| CRS transform | 32 | M to L | |
| `IN TRANSACTIONS` | 25 | L | Breaks one statement, one commit; last, and a candidate for "Accepted" |

**Performance:**

| Item | TODO | Size | Note |
| --- | --- | --- | --- |
| Snapshot save and load benchmark | 41 | S | First: the baseline for the binary vector trailer |
| Order-only index scan; count a range from the index | 37 | M + M | Nodes missing the property must still be returned. Feeds `totalCount` in lora-graphql |
| Paged relationship scans | 39 | M | Node scans are done; scans feeding a write keep copying ids |
| Multi-argument calls stop copying a carried list | 20 | M | `Cow` arguments across `eval/builtins/*` |
| Composite seeks and composite RANGE indexes | 36 | L | Constraints are backed by RANGE indexes: `constraint_enforce.rs` and restart index state are in scope |
| Join ordering and cardinality estimates past scans | 38 | L | Changes plans for every multi-pattern query; `planner_stats.rs`, `explain_estimates.rs` |
| Real CSE and redundant-scan elimination | 40 | L | Only if a measured query needs it |

## Milestone 8. Durability leftovers

Decided 2026-10-11: one writer at a time is sufficient. The multi-writer
phases (isolation and conflict semantics, the write-attempt object,
fine-grained commit, concurrent ID allocation, `FsyncCoord`, stress
tests) are out of the list, and `docs/design/concurrency-implementation-plan.md`
is deleted. The single writer is described in `docs/architecture/`.

What stays is independent and small:

- Managed checkpoints on a worker instead of inline under the writer
  mutex (M), then the time-based scheduler (line 54, S).
- Consolidate torn-tail detection (line 55, S to M) and update ADR 0004.
- Binary vector-index trailer (line 56, M). Old readers fail on a binary
  payload, so this is a format version or a tag byte.
- Remove the unused `LockTable` and the dead `lock_table` field (S).
- `anyhow::Result` out of the public stream API (line 108), with the
  bindings.
- Property-based tests for queries and snapshot round trips (line 103,
  M).

## Milestone 9. Release and measurement

- Re-run `comparisons/` and resync
  `apps/loradb.com/src/lib/benchmarks/data.js` (line 115, M), after
  milestones 7 and 8 so the public numbers are current.
- Release items of `RELEASING.md:521-533` (line 116): musl server
  target S, committed CHANGELOG S, signing M (needs certificates),
  package managers, bench gating (needs a low-noise runner).
- Strip "Recommended order" from `graphql-load-audit.md` once its
  findings are gone (line 122).

## Decisions that are yours

1. **Decided 2026-10-11: breaking changes are allowed.** No flags, no
   compatibility shims, no deprecation releases. A break is marked in
   its commit (`fix(executor)!: ...`) and named in the release notes.
   This covers the silent writes of milestone 1, `=~` raising, the
   integer conversion in the Node binding, the removed `Unknown*`
   errors, the `begin()` and `QueryRunner` signatures, `anyhow::Result`
   leaving the stream API, and a new snapshot format version for the
   binary vector trailer.
2. **Server auth (line 86, P1) against the accepted note** "Admin
   routes have no auth, managed-platform territory". The plan builds
   bearer auth for all routes; say if admin routes stay out.
3. **Comparison type mismatch (line 28):** static check only, or drop.
4. **The large P3s:** quantified path patterns, CRS transform, real
   CSE, kapsule internalising, `IN TRANSACTIONS`. Each is a week or
   more for little demand so far. Keeping them in the plan is the
   default; moving them to "Accepted" would cut roughly a third of
   milestone 7.

## What was not verified

- Whether the 29 ignored tests said to pass do (not run).
- Whether lora-wasm shares the number conversion or has `executeMany`.
- Whether a fast-path failure after WAL events poisons the recorder
  (`write_guard.rs:238-242`), with a WAL attached.
- Sizes of the relationship-rule work on abstract targets and of the
  runner batching: both are estimates from reading the entry points,
  not the whole path.
- Every measurement item (lines 76, 91, 115) needs a quiet machine.
