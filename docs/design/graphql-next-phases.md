# GraphQL: Next Phases

Follow-up to [graphql-implementation-plan.md](graphql-implementation-plan.md).
Phases 0 to 9 of that plan and a parity pass with `@neo4j/graphql` 7.5 are
implemented in `packages/lora-graphql`. This document orders what comes
next, numbered on from the plan's Phase 10.

It is built from three investigations run on 2026-09-29: remaining gaps
against `@neo4j/graphql` (source in `tmp/graphql`), performance and engine
leverage (measured on the bench graph: 20k festivals, 100k `FOLLOWS`), and
developer experience, operations and security.

## Principles

- **Measure before and after.** Every performance phase lands with a
  benchmark in `bench/` and, where a plan shape matters, a plan check in
  `check()` so the regression cannot come back silently.
- **Package first, engine second.** A package-side fix that ships this
  week beats an engine fix that ships next release. When the engine fix
  lands, the workaround is removed and its row in the engine table goes.
- **Generate less, not more.** New surface is opt-in per type or field,
  like the rest of the vocabulary.
- **Security defaults are production defaults.** Anything that can leak
  (errors, introspection, ad-hoc documents) is safe by default in
  production.

## Summary

| Phase | Theme                                 | Size   | Engine work                   |
| ----- | ------------------------------------- | ------ | ----------------------------- |
| 11    | Mutation statements seek, then expand | S      | Optional (condition pushdown) |
| 12    | Security hardening                    | S to M | None                          |
| 13    | CI, release and observability         | S to M | None                          |
| 14    | Query surface gaps                    | M      | None                          |
| 15    | Compile caching and read overheads    | S to M | None                          |
| 16    | Engine fixes that unlock the package  | M      | Yes                           |
| 17    | Extensibility and typed tooling       | M to L | None                          |
| 18    | Subscriptions at scale                | M to L | Change feed                   |
| 19    | Docs site, examples and migration     | M      | None                          |

Phases 11, 12 and 13 are independent and can run in parallel. Phase 16 can
start any time in `crates/`; each fix removes a workaround in the package.

## Phase 11: Mutation Statements Seek, Then Expand

**Status: done.** Measured on the bench graph (`bench/mutate.bench.ts`):
a create with a genre and two followers connected takes 0.78 ms (was
262 ms) in 3 statements (was 6). A follower disconnect takes 0.7 ms (was
63 ms). The pre-delete as seek then expand is 1989x faster than the
single pattern. `scanExpands()` in `src/analyze/plans.ts` flags an
`Expand` fed by a full scan, and `test/mutation-plans.test.ts` runs it
over every kind of mutation statement. On the old code it reports 12
findings. The engine follow-up in Phase 16 still stands.

The largest measured problem. A create with one genre connect and two
follower connects takes **262 ms**; a plain create takes 0.53 ms.

**Cause.** The statement that removes an existing edge before reconnecting
a pair (the E14 workaround) is one pattern with the key test in the WHERE:

```cypher
MATCH (a:Festival)<-[r:FOLLOWS]-(b:User) WHERE a.key = row.from AND b.key = row.to
```

The optimizer only turns a filter into an index seek when it sits directly
on a node scan (`use_indexed_node_scans`,
`crates/lora-compiler/src/optimizer.rs`), so this expands every `FOLLOWS`
edge first: 130 ms. Written as a seek and then an expand it takes 0.064 ms:

```cypher
MATCH (a:Festival) WHERE a.key = row.from
MATCH (a)<-[r:FOLLOWS]-(b:User) WHERE b.key = row.to
```

**Work.**

1. Rewrite every multi-hop MATCH in `src/execute/mutate.ts` (connect
   pre-delete, disconnect, edge updates, nested node updates, delete
   neighbour lookups, cardinality checks) as key seek, then expand.
   The read compiler already does this for roots; the write paths do not.
2. Skip the pre-delete when the owner was created in the same plan: a
   fresh node has no edges.
3. Skip the cardinality check when a single relationship on a fresh node
   gets exactly one connect.
4. Put the mutation statement templates through the S2 plan checks, with
   a test that fails on an `Expand` below a scan without a seek. This
   check would have caught the problem.

**Expected gain.** 300x to 2000x per nested connect or disconnect. A
nested create goes from 7 statements to 4.

**Engine follow-up (Phase 16).** Push single-variable conditions down to
their own scan and start from whichever end can seek. That also protects
hand-written `@cypher` statements.

## Phase 12: Security Hardening

**Status: done.** `maskErrors` (default in production) with `onError` and
an `id`; `cursorSecret` signs cursors with HMAC-SHA-256 (pure JS, so it
works in browsers), and without it the docs say "tagged"; claim and
context paths read own properties, and `eq` / `in` / `includes` compare
structurally; `guards` (depth, aliases, root fields, tokens,
introspection) in `execute()` and `persist()`, `persistedOnly`, and
`validationRules()` / `envelopPlugin()` for other servers;
`test/auth-properties.test.ts` (fast-check, reference evaluator) and
[graphql-threat-model.md](graphql-threat-model.md). The property tests
found an engine crash: an empty index range (`x > 5 AND x < 5`) panicked
in `CowOrdMap::range` and aborted the process. Fixed in `lora-store`.

**1. Error redaction.** `DATABASE_ERROR` carries the engine's message and
`originalError`, which leak Cypher text, labels and property names. Add a
`maskErrors` option, on by default when `NODE_ENV=production`: the client
gets the `code` and a correlation id, and an `onError` hook gets the
detail.

**2. Cursors.** The README and `src/compile/cursor.ts` call cursors
"signed", but they are base64 JSON tagged with their sort. A forged
cursor can only move a page's start within rows the caller may read
(sorting on fields with row rules is refused), so the risk is low, but
the wording overstates it. Either add an HMAC (`cursorSecret` option,
cursors rejected when the signature fails) or change the wording to
"tagged". Recommendation: HMAC when a secret is configured, "tagged"
otherwise.

**3. Claim and context lookups.**

- `$context.<path>` walks inherited properties; restrict it to own
  properties.
- `eq` on claims compares `JSON.stringify` output, so key order matters.
  `in` uses `===`, so object claims never match. Use one structural
  equality for both.

**4. Document guards.** `maxCost` bounds rows, not parser or validator
work. Ship validation rules for depth, alias count, root field count and
token count, an `introspection: false` rule, and a `persistedOnly` option
that rejects ad-hoc documents in `execute()`. Package them as an Envelop /
Yoga plugin so the server enforces them before execution.

**5. Property-based tests.** Use fast-check to generate models, claim sets
(missing, null, prototype-like keys) and nested AND / OR / NOT where trees.
Compare each compiled result against a reference evaluator in JavaScript
over an in-memory graph, and assert that no row outside the rules is
returned through lists, connections, aggregates, counts, search, cursors
or subscriptions. Fuzz cursors and global ids as well. This is the
systematic version of the two adversarial reviews, which found 9 and then
10 real defects by hand.

**6. Threat model.** A short `docs/design/graphql-threat-model.md`:

- The server verifies the JWT, and the library trusts the context.
- `@cypher` statements are trusted code that can read `$jwt`.
- What `onWrite` and subscription payloads may contain relative to READ
  rules.
- Which checks run at compile time and which run in the database.

## Phase 13: CI, Release and Observability

**Status: done.** `.github/workflows/lora-graphql.yml` runs lint,
typecheck, tests, build and a pack check against lora-node built from the
tree, plus a non-blocking job against the published binding the peer
range names. `packages-release.yml` builds and publishes the package
(after `publish-node`), and `sync-versions.mjs` keeps its version and
peer range in lockstep. `onStatementEnd`, `tracer` / `traceStatements`,
`metrics`, `budget` / `onCost`, and `extensions.cost` from `execute()`.

**1. CI.** No workflow runs the package's typecheck, lint, tests and build,
and `packages-release.yml` has no job for it (lora-query and
lora-graph-canvas both have one). Add both. Run the tests against the
pinned `@loradb/lora-node` and against the workspace binding built from
source: that also catches the stale `dist/` gotcha.

**2. Versioning.** Release in lockstep with lora-node (0.15 today), with
the peer range documented.

**3. Statement events.** `onStatement` fires before a statement runs and
carries only the field and the text. Add an `onStatementEnd` event with
duration, row count, error, mode, cost estimate, operation name and
persisted id.

**4. Tracing.** Add an optional OpenTelemetry helper: a `lora.graphql.field`
span containing `lora.cypher` spans, with `db.statement` behind a flag.
Add a small metrics interface for counters and histograms.

**5. Budgets.** Add an `onCost(context, cost)` / `budget(context)`
callback, so a budget can be set per user instead of one global
`maxCost`. Also expose the estimate in `extensions` so clients can tune
their queries.

## Phase 14: Query Surface Gaps

**Status: done.** All ten items, in `test/query-surface.test.ts`. Choices
worth knowing: `count { nodes edges }` applies to relationship
connections, which now get their own types (root connections keep
`count: Int!`); `@cypher` filters and sorts work in root fields only;
vector search connections page within a fixed candidate pool; `@groupBy`
is a field directive with a `<plural>Grouped(by:)` root; field-level
rules on relationship and `@cypher` fields take READ rules only. Durations
aggregate with `reduce` because of E25.

Remaining differences from `@neo4j/graphql` that real apps hit, in
order of value over effort.

1. **Sort connections by edge properties** (`sort: [{ edge: { since: DESC } }]`).
   Edge `where` exists; edge sort does not. The keyset cursor must carry
   the edge value and the target key. Relationship properties have no
   index, so this is a sort per parent, bounded by the page.
2. **Relationship fields on interfaces** (neo4j's `@declareRelationship`),
   so `events { venue { name } }` works at interface level. The
   per-member `CALL` design fits this directly.
3. **`nestedOperations` and `@relationship(aggregate: false)`.** These trim
   the nested input and aggregate surface per relationship, in line with
   "generate less".
4. **Richer aggregates.** Add String `shortest` / `longest`, filters on
   string length, `count { nodes edges }`, and Duration aggregates.
5. **Filter and sort on `@cypher` fields**, opt-in. They can never use an
   index, so `check()` reports each one as a scan. That keeps S2 honest.
6. **Search results as connections**, with cursors, for full-text and
   vector search.
7. **Nested delete** (`update: { rel: { delete: { where } } }`), bounded
   like bulk deletes. `onDelete: CASCADE` covers part of the need today.
8. **`@groupBy`** (neo4j 7.5), written as `WITH key, collect(x)` because
   of E16.
9. **`@cypher` returning interfaces, unions and plain object types.**
10. **Field-level `@authorization` on relationship and `@cypher` fields.**
    Today these are refused at startup rather than ignored. Enforcing
    them means a rule check per projected related row.

Deliberately left out: `@coalesce`, which hides predicates from indexes
and interacts with E19, and disconnect by `where`, since key-only
disconnects are intentional.

## Phase 15: Compile Caching and Read Overheads

**Status: done, with one item measured and left.**

1. Compiles are cached per field node, exact variables, claims and the
   `$context` values they read: `festivals(limit: 20)` through
   `execute()` goes from 0.144 ms to 0.060 ms, the nested bench operation
   from 1.37 ms to 1.20 ms (its time is in the engine). A parameter binder
   that reuses text across different variable values was not built:
   values change statement structure (a `where` present or null, cursors,
   limits), so it could not be done safely without tracking where each
   value flows.
2. Measured and left: the nested page statement takes 0.97 ms; its
   relationship connection 0.375 ms with row maps, 0.318 ms with separate
   node, cursor and property lists, 0.234 ms with node maps alone. The
   per-parent `CALL` dominates, so the projection change is worth about 6%
   of the statement and was not made. Edge properties were already
   projected only when selected.
3. Unnecessary after the E13 fix: `totalCount` in a transaction stops at
   `LIMIT` too.
4. EXPLAIN now estimates range, text and point seeks; plan reports carry
   `estimatedRows`, and `check({ rowBudget })` turns large estimates into
   findings.

5. **Cache compiled operations.** Every resolver recompiles today:
   0.14 ms for the nested bench operation. An end-to-end
   `festivals(limit: 20)` takes 0.15 ms against 0.03 ms for the raw
   statement.
   - Cache by field node (documents are already cached in `execute`) and
     by variable shape.
   - Store the statement text and a parameter binder.
   - Expected gain: 2x to 4x on point and simple list reads.
6. **Lighter connection projection.** The per-parent `CALL` that collects
   `{ node, __cursor, properties }` maps takes 0.47 ms against 0.17 ms
   for a minimal collect. Build `__cursor` in JavaScript from fields
   already projected, and project edge properties only when they are
   selected.
7. **`totalCount` without the transaction penalty.** Until E13 is fixed,
   run the count and the page as two parallel `stream()` calls instead of
   one transaction under a deadline: 11.4 ms goes to about 0.3 ms. Trade-off: the
   count and the page may come from different snapshots. Make this an
   option and document the trade-off.
8. **Engine estimates.** `explain()` now returns `estimatedRows` on some
   operators, so E11 in the plan is partly out of date. Feed these
   estimates into the S2 row budgets and the S6 cost model.

## Phase 16: Engine Fixes That Unlock the Package

**Status: done in the engine.** Condition pushdown (any end of a chain
seeks; conditions sit on the scan that binds their variables), E13, E14,
E15, E17, E18, E19, E20, E21, `count(n)` from label counts, and
`tx.executeMany` in lora-node all landed, each with tests in
`crates/lora-database/tests/`. Measured: the single-pattern connect
pre-delete 175 ms to 0.007 ms, `IN` over 20 keys 14 ms to 0.02 ms,
`count(n)` 1.9 ms to 0.0002 ms, a `LIMIT 10` read in a transaction 21 ms
to 0.1 ms, a newest-first temporal feed 8.1 ms to 0.014 ms, and 200
statements in one transaction 0.10 ms to 0.03 ms each with `executeMany`.
Package side: `IN` filters and relationship anchors are plain `IN` seeks
(no `UNWIND`), generated `MATCH` clauses no longer add label tests,
temporal fields get RANGE indexes again, and connect is one `MERGE` per
relationship field instead of delete then create. Mutation statements
keep their seek-then-expand form. The E13 fix makes the Phase 15
`totalCount` item unnecessary. The property-based tests found one more
package bug on the way: `NOT` of an empty filter was ignored, and
`OR: []` matched everything.

Each fix is in `crates/`. When it lands, the package drops the workaround
and the engine table in the plan loses a row.

| Fix                                                                                     | Where                                                                            | What it unlocks                                                                    | Size   |
| --------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | ------ |
| Condition pushdown: push single-variable conditions to their scan; seek from either end | `crates/lora-compiler/src/optimizer.rs`                                          | Phase 11 without the rewrite; fast hand-written `@cypher`                          | M      |
| E13: early LIMIT under a deadline and in transactions                                   | `crates/lora-database/src/database/execute.rs`, `occ.rs`                         | Reads in `begin()`: 9.8 ms to 0.15 ms; `totalCount` in one snapshot                | S to M |
| E17: temporal values in RANGE indexes                                                   | `PropertyIndexKey::from_value`, `crates/lora-store/src/memory/property_index.rs` | Index-backed "newest first" feeds (about 7 ms to 0.03 ms at 20k, growing linearly) | S to M |
| E14: MERGE honours a bound end node                                                     | planner / executor                                                               | Connect in one statement instead of delete, then create                            | M      |
| E15: writes inside `CALL { }`                                                           | executor                                                                         | A whole nested write in one statement                                              | M to L |
| `count(n)` from label counts                                                            | planner                                                                          | O(1) unfiltered `totalCount`                                                       | S      |
| E18: `IN` seeks                                                                         | `property_equality_for_var` in `optimizer.rs`                                    | Removes the UNWIND anchor (already 122x faster, so low priority)                   | S      |
| `tx.executeMany` / pipelining                                                           | `crates/bindings/lora-node/src/tasks.rs`                                         | About 0.1 ms fixed cost per statement in a transaction                             | M      |

Measured and not worth fixing for the package: the E16 wrapper (no cost),
E21 label tests (negligible), batching across root fields (five aliased
lookups take 0.21 ms against 0.10 ms).

## Phase 17: Extensibility and Typed Tooling

**Status: done.** `@customResolver(requires:)` with a `resolvers` option;
custom scalars with `@storedAs(type:)` and a `scalars` option;
`lora-graphql compile` writing `manifest.json` and `operations.d.ts`, with
`loadManifest()`; `check` with `--variables`, `--baseline`,
`--row-budget`, `--database` and unused-index reporting; schema lint; and
`lora-graphql analyze`. Two deviations from the plan: the manifest holds
validated documents, not statements per variant (statement text depends
on variable values and claims, so it is compiled per request and cached),
and `check` plan-checks queries only, since mutation statements need the
data they write against.

1. **Custom resolvers.** Add a `resolvers` option plus `@customResolver(requires:)`
   for fields computed in JavaScript. The `requires` selection is fetched
   in the same statement, and the fields it names are validated at
   startup. This is the escape hatch real apps reach for first.
2. **Custom scalars**, pass-through with a declared storage type.
3. **Finish Phase 6: `lora-graphql compile`.**
   - Input: a persisted operations manifest.
   - Output: one entry per hash (statements per variant, parameter map,
     read-sets and write-sets, plan snapshot) and `.d.ts` types for
     results, variables and `@cypher` parameters.
   - Add `loadManifest()` at runtime, with a test proving the translator
     never runs.
   - Until then, document a graphql-codegen recipe over `print`.
4. **Harden `check`.**
   - Check mutations and subscriptions too (today only queries).
   - Accept a `--variables` fixtures file instead of placeholder values.
   - Add a `--baseline` plan snapshot so plan changes show up in review.
   - Report unused indexes (promised in S1).
5. **Schema linting** in `check`:
   - operators whose index kind is expensive;
   - types with mutations but no `@authorization`;
   - relationships without `@cardinality` when no statistics exist.
6. **An `analyze` CLI command**: listed in the plan's CLI table, not built.

## Phase 18: Subscriptions at Scale

1. **Shared visibility checks.** Each write costs every subscriber a
   visibility query and a node read, O(subscribers × events). Group
   subscribers by type, `where` shape and claims fingerprint. Run one
   visibility query and one batched read (UNWIND keys) per group.
2. **Richer events.** Add `previousState` and `timestamp`, which cost one
   pre-image read per write. Also add relationship events for connect and
   disconnect: the write-sets already carry them, and neo4j 7 does not
   offer them.
3. **Engine change feed.** Expose committed changes from the engine in
   lora-node, for example `db.changes({ fromLsn })` with resume tokens.
   The pieces exist: `MutationRecorder` / `MutationEvent` in
   `crates/lora-store/src/mutation.rs` and the WAL's `TxCommit` /
   `MutationBatch` records with LSNs in `crates/lora-wal/src/record.rs`.
   With it, subscriptions work across processes and see `@cypher` and
   outside writes, and the `broad: true` events go away.

## Phase 19: Docs Site, Examples and Migration

1. **loradb.com pages:**
   - getting started;
   - directive reference;
   - the generated API;
   - the smart layer;
   - authorization, with the Phase 12 guidance up front;
   - translation rules;
   - migrating from `@neo4j/graphql`.

   The docs site deploys on every push to main, so these pages land with
   the first npm release, or unlisted until then.

2. **Examples.** GraphQL Yoga (with response cache invalidation through
   `onWrite`, as Phase 7 promised), Apollo Server 4 and graphql-http.
3. **Testing helper.** `createTestLoraGraphQL({ typeDefs, seed })` in
   memory with `assertSchema`, plus an `expectSeeks()` matcher.
4. **Migration tool.** `lora-graphql migrate neo4j <schema>`:
   - rewrites `@id` to `@key(generate: true)`;
   - adds `@query`, `@mutation` and `@filterable` from observed usage;
   - leaves TODOs for what has no equivalent (federation, CDC
     subscriptions).
5. **Later.** `lora-graphql introspect` to draft SDL from an existing
   database; editor support through lora-query.

## Not Planned

These `@neo4j/graphql` features are left out on purpose:

- Offset pagination and `arrayconnection` cursors. Keyset only.
- `update` and `delete` with an optional, unbounded `where`.
- Every operation on every type, every filter on every field, and the
  deprecated flat filter aliases.
- `unsafeEscapeOptions`.
- JWT verification in the library. The server verifies the token.
- Vector providers that call external embedding APIs from the database
  layer. An `embed(text)` callback is the alternative, if wanted.
- A single relationship that silently returns the first of several.
  Cardinality is enforced instead.
- `connectOrCreate`. neo4j 7 removed it, and `upsert` covers the need.
- Federation, unless a consumer asks for it.
