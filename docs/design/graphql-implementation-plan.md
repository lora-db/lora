# GraphQL Implementation Plan

This plan adds a schema-first GraphQL layer to LoraDB: one annotated
`schema.graphql` describes the graph **and** the public API, and a library turns it
into an executable GraphQL schema whose operations run as Cypher against LoraDB —
the model `@neo4j/graphql` popularised, built for LoraDB's engine and dialect, and
isolated in this repository as `@loradb/lora-graphql`.

What sets it apart is that the library **reasons about the queries it generates**:
it derives the indexes an API needs from the filters it exposes, proves with
`explain()` that every generated query seeks instead of scans, compiles known
operations ahead of time, knows exactly what each operation reads and writes, and
checks hand-written `@cypher` before anything runs.

> **Using this document with Claude:** it is self-contained. Work phase by phase;
> each phase lists its goal, tasks and exit criteria, and every phase must leave
> `yarn typecheck`, `yarn test` and `yarn lint` green. Behaviours of the engine this
> plan depends on are listed under [Engine prerequisites](#engine-prerequisites) —
> check each against the current code before relying on it; several are being worked
> on. Facts marked _verified_ were run against `@loradb/lora-node` 0.15.0.

## Status

Implemented in `packages/lora-graphql` (see its README for the user-facing
reference):

| Phase                             | State                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0: skeleton, driver               | Done. One structural driver for both bindings; interactive transactions for mutations. The offline `lora-query` parser build (E12) was not needed: `@cypher` is linted with a small lexer offline and planned with `explain()` online.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 1: model, S1, `assertSchema`      | Done                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 2: reads, variants, S2            | Done. Variants are the specialisation itself: statement text depends only on the input's shape, so LoraDB's plan cache serves repeats.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 3: `@cypher`, S4                  | Done: object, Query and Mutation fields; parameter and write-clause checks at startup; `explain()` checks in `check()`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 4: mutations, write-sets          | Done: create / update / delete by `@key`, nested connect / create / disconnect, cardinality and connect-target checks, `@default`, `@timestamp`, `@readonly`, `@key(generate:)`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 5: authorization                  | Done: claim checks folded at compile time; node rules compiled; filter and validate (BEFORE / AFTER / READ)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 6: AOT, S8 types                  | Partly: `persist()` validates operations at startup and runs them by id. No emitted manifest or TypeScript types (graphql-codegen over `printPublicSchema()` covers S8).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| 7: change tracking                | Done: `onWrite`, `changes()`, `affects()`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 8: statistics                     | Done: related-node key anchoring, cost estimates with `maxCost`, `analyze()` degrees                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 9: evolution                      | Done: `diffSchemas` and `lora-graphql diff`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 10: hardening                     | Benchmarks (`yarn bench`) and CLI done; loradb.com pages and release not started                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Beyond the plan                   | Backward pagination (`last` / `before`), `upsert` mutations, `@fulltext` search (E4 is fixed), `@vector` similarity search, generated `@subscription` fields.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Parity pass with `@neo4j/graphql` | Interfaces and unions (root fields, relationships, filters, nested writes; one `CALL` per implementing type, merged by sort key); connection aggregates and aggregate / connection filters; `includes`, `isNull`, case-insensitive filters; `adjust` math and list operators; relationship property updates in place; bulk `update<Types>` / `delete<Types>` by `where`; `onDelete: CASCADE / RESTRICT`; required relationships; `@settable`, `@selectable`, `@populatedBy`; `@jwt` / `@jwtClaim`, `$context` values, field-level `@authorization`, `@authentication(jwt:)`, CREATE_RELATIONSHIP / DELETE_RELATIONSHIP / SUBSCRIBE rules; caller-owned transactions (`begin()`); cost limit per operation; bounded subscription queues. |

Open decisions, as resolved: nested operator filters; no offset
pagination; authorization in the library; no variant cap (specialisation
is per request, and the engine caches plans by text).

Engine prerequisites, re-checked against the current tree: E1 fixed (empty
labels return no rows), E4 fixed (full-text via `CALL … YIELD`), E6 fixed
(bigint parameters and results), E7 fixed (`begin()`), E5 partly (a single
sort key streams from a RANGE index under a range predicate). E9 and E10
remain. New findings, each worked around in the package:

| #   | Behaviour                                                                                                             | Where                                                                                                   |
| --- | --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| E13 | Reads under a deadline or in a transaction do not stop early at `LIMIT` (the pull path requires `deadline.is_none()`) | `crates/lora-database/src/database/execute.rs`, read-only branch of `execute_rows_with_params_deadline` |
| E14 | `MERGE (a)-[r:T]->(b)` and `[(a)-[:T]->(b) \| …]` ignore an already-bound `b` and reuse any `T` edge of `a`           | planner / executor                                                                                      |
| E15 | Writes inside `CALL { }` fail with `LORA_READ_ONLY`, though `explain()` says `mutating`                               | executor                                                                                                |
| E16 | An aggregate nested in another call (`head(collect(x))`, `collect(x)[0..2]`) is not aggregated                        | analyzer                                                                                                |
| E17 | RANGE indexes skip temporal values, yet range predicates on temporals are planned through them and return no rows     | `PropertyIndexKey::from_value`, `crates/lora-store/src/memory/property_index.rs`                        |
| E18 | `x IN $list` plans a label scan; equality seeks                                                                       | optimizer                                                                                               |
| E19 | `null` values in a property map are stored as properties (`keys(n)` includes them)                                    | store                                                                                                   |
| E20 | Existence constraints are checked at `CREATE`, before a following `SET`                                               | executor                                                                                                |
| E21 | `MATCH (a:A)-[:T]->(b:B)` ignores the labels of every node but the first                                              | planner (the package adds `WHERE b:B`)                                                                  |
| E22 | Integer `/` integer returns a float                                                                                   | expression evaluator (the package wraps `toInteger`)                                                    |
| E23 | Negative list slice bounds (`l[..-1]`) return wrong results                                                           | expression evaluator (the package writes `l[..size(l) - n]`)                                            |
| E24 | `COUNT { … RETURN DISTINCT }`, `EXISTS { }` and `UNION` inside `CALL { }` are not supported                           | parser / analyzer (the package uses `reduce` and per-type `CALL`s)                                      |

## Why

Applications embedding LoraDB (the first is Festimap, a festival-discovery app with a
GraphQL API) currently hand-write three layers per entity: the GraphQL SDL, resolvers,
and Cypher repositories — plus DataLoaders to avoid N+1, plus the knowledge of which
Cypher constructs are fast or safe on LoraDB, which indexes each filter needs, and
which cache entries a write invalidates. A declarative layer removes the boilerplate;
a _reasoning_ layer removes the expertise tax, and keeps it correct as the schema
evolves.

## Goals

1. **One annotated SDL** (`typeDefs`) defines node labels, properties, relationships
   and which API operations exist.
2. **Generated public API**: object types, filter and sort inputs, Relay connections,
   aggregates, and — opt-in — mutations, printed as a clean public SDL.
3. **`@cypher` fields**: any field can be backed by a hand-written, parameterised
   statement — statically checked.
4. **One Cypher statement per GraphQL operation**, specialised to the variables
   actually supplied, so every variant can use an index. No N+1.
5. **Provably index-backed**: the indexes an API needs are inferred from it, and every
   generated query shape is checked with `explain()` — a full scan behind a filter
   fails the build.
6. **Know what each operation touches**: read-sets and write-sets drive precise cache
   invalidation and change events.
7. **Secure by default**: reads only unless mutations are opted in; bounded work,
   measured against real cardinalities; parameters only; authorization compiled into
   the query.
8. **Server-agnostic**: a standard `graphql-js` `GraphQLSchema` for Yoga, Apollo or
   `graphql-http`.

## Non-goals (for this plan)

- A GraphQL endpoint inside `lora-server`; federation; subscriptions from writes the
  library didn't make (needs engine change-data capture).
- Mirroring `@neo4j/graphql`'s full API. Its generated surface is large and couples
  clients to storage; this plan generates less and makes the rest opt-in.

## Current State

- The Node binding (`crates/bindings/lora-node`) exposes `createDatabase`,
  `execute(query, params, { timeoutMs, signal })`, `transaction(…)`, streams,
  snapshots, WAL persistence, and `explain()` / `profile()`.
- `explain()` returns the plan `shape` (`readOnly` / `mutating`), the
  `resultColumns`, and an operator tree; `estimatedRows` is reserved for a future cost
  model (`null` today). _Verified_: the leaf operator shows exactly how rows are found —

  | Filter                            | Declared index | Leaf operator                                                         |
  | --------------------------------- | -------------- | --------------------------------------------------------------------- |
  | `n.p = $v`                        | none needed    | `NodeByPropertyScan` (lazy exact-match index, ADR-0001)               |
  | `n.p > $v` (range)                | RANGE          | `NodeByPropertyRangeScan` — without it: `NodeByLabelScan` (full scan) |
  | `n.p CONTAINS $v` / `STARTS WITH` | TEXT           | `NodeByTextScan` — without it: `NodeByLabelScan`                      |
  | `geo.within_bbox(n.p, …)`         | POINT          | `NodeByPointScan`                                                     |
  | `ORDER BY n.p LIMIT k`            | RANGE          | `Sort{top_k}` over `NodeByLabelScan` either way (E5)                  |

  `profile()` adds per-operator row counts and timings — enough to check how much of
  the graph a query shape touches, not just which operators it uses.

- `packages/lora-query` wraps a WASM build of the pest Cypher parser with `parse`,
  `validate`, `outline` and `analyse` — static analysis of Cypher without a database.
  It is built with `wasm-pack --target bundler`; a Node-loadable build is a Phase 0
  task.
- `docs/reference/cypher-support-matrix.md` and `docs/design/known-risks.md` describe
  the dialect; consumer testing found the behaviours under
  [Engine prerequisites](#engine-prerequisites).

## Package

`packages/lora-graphql` → **`@loradb/lora-graphql`** (BUSL-1.1), Vite + Vitest like
`lora-query`. `graphql` (`^16 || ^17`) is a peer dependency; `@loradb/lora-node` /
`@loradb/lora-wasm` are peers of their driver adapters only.

```
packages/lora-graphql/
  src/
    index.ts                    public API
    lora-graphql.ts             entry class: typeDefs + driver → GraphQLSchema
    driver/                     LoraDriver interface + lora-node / lora-wasm adapters
    model/                      annotated SDL → validated GraphModel
    schema/                     GraphModel → public SDL (types, inputs, connections, ops)
    compile/
      cypher/                   typed Cypher AST + printer (no string building)
      read/  write/  auth/      operation → statement
      variants.ts               specialisation by supplied variables
      plan-cache.ts             compiled-statement cache for ad-hoc operations
    analyze/
      indexes.ts                index inference from the API (S1)
      plans.ts                  explain()-based plan checks (S2)
      cypher-lint.ts            @cypher static analysis via lora-query (S4)
      statistics.ts             cardinality sampling + cost estimates (S6)
    track/                      read/write-sets, invalidation, change events (S5)
    evolve/                     SDL diff → migration + API breaking changes (S7)
    execute/                    run statements; decode LoraDB values
    cli/                        lora-graphql print · check · compile · diff · analyze
  test/
    tck/  integration/  perf/
```

### Public API

```ts
import { LoraGraphQL } from "@loradb/lora-graphql";
import { LoraNodeDriver } from "@loradb/lora-graphql/driver/lora-node";

const lora = new LoraGraphQL({ typeDefs, driver: new LoraNodeDriver(db) });
await lora.assertSchema({ create: true }); // inferred constraints + indexes, catalog
const schema = await lora.getSchema(); // executable GraphQLSchema
lora.printPublicSchema(); // client-facing SDL, no directives
lora.onWrite((change) => cache.invalidate(change.entities)); // S5
```

The CLI wraps the build-time capabilities:

| Command                             | Does                                                                         |
| ----------------------------------- | ---------------------------------------------------------------------------- |
| `lora-graphql print`                | Public SDL                                                                   |
| `lora-graphql check`                | Model validation, `@cypher` lint, index inference, plan checks — the CI gate |
| `lora-graphql compile <operations>` | Ahead-of-time compilation of persisted operations (S3)                       |
| `lora-graphql diff <old> <new>`     | Database migration + API breaking-change report (S7)                         |
| `lora-graphql analyze`              | Cardinality statistics from a live database (S6)                             |

## Directive Vocabulary

Model:

| Directive                                                                | On                   | Meaning                                                                                     |
| ------------------------------------------------------------------------ | -------------------- | ------------------------------------------------------------------------------------------- |
| `@node(labels: [String!])`                                               | OBJECT               | A node label (default: the type name)                                                       |
| `@key`                                                                   | FIELD                | Natural key: required, unique; used for `connect` and global ids                            |
| `@unique`                                                                | FIELD                | Uniqueness constraint                                                                       |
| `@index(kind: RANGE \| POINT \| TEXT \| FULLTEXT)`                       | FIELD                | An explicit index — usually **unnecessary**: inferred from `@filterable` / `@sortable` (S1) |
| `@relationship(type: String!, direction: IN \| OUT, properties: String)` | FIELD                | An edge; the target type is the field's type                                                |
| `@relationshipProperties`                                                | OBJECT               | Properties carried by a relationship type                                                   |
| `@alias(property: String!)`                                              | FIELD                | API name differs from the stored property                                                   |
| `@default(value: …)` · `@timestamp(operations: [CREATE, UPDATE])`        | FIELD                | Set on write                                                                                |
| `@private`                                                               | FIELD                | Stored, never exposed                                                                       |
| `@cardinality(max: Int!)`                                                | FIELD (relationship) | Declared upper bound, used by cost estimates until statistics exist (S6)                    |

API shape (restrictive by default):

| Directive                                                          | On            | Meaning                                                                         |
| ------------------------------------------------------------------ | ------------- | ------------------------------------------------------------------------------- |
| `@query(read: Boolean = true, aggregate: Boolean = false)`         | OBJECT        | Generated reads                                                                 |
| `@mutation(operations: [CREATE, UPDATE, DELETE])`                  | OBJECT        | Generated mutations — **none by default**                                       |
| `@filterable(byValue: [EQ, IN, CONTAINS, STARTS_WITH, LT, GT, …])` | FIELD         | Filter operators (default `EQ` / `IN`); each implies an index kind (S1)         |
| `@sortable`                                                        | FIELD         | Sort/keyset-paginate on this field; implies a RANGE index                       |
| `@limit(default: Int, max: Int)`                                   | OBJECT        | Page size bounds (max capped globally)                                          |
| `@relayId`                                                         | FIELD         | Opaque global `id` derived from `@key`                                          |
| `@cypher(statement: String!, columnName: String!)`                 | FIELD         | Custom statement; `this` = parent node; arguments are `$name`; `$jwt` available |
| `@fulltext(indexes: [{ name, fields }])`                           | OBJECT        | Full-text search (needs E4)                                                     |
| `@authentication` / `@authorization(filter:, validate:)`           | OBJECT, FIELD | Rules over `$jwt`, compiled into predicates (Phase 5)                           |

## Generated API

For each `@node` type with `@query(read: true)`:

- `festivals(where, sort, limit)` and `festivalsConnection(where, sort, first, after)` —
  Relay with keyset cursors over the sort key + `@key` (never `SKIP`).
- `festivalsAggregate(where)` when `aggregate: true`; `node(id)` when any `@relayId`.
- Relationship fields with `where` / `sort`; list relationships also get a
  `…Connection` whose edges carry the relationship properties.
- Filters as nested operator objects with `AND` / `OR` / `NOT` and
  `some` / `all` / `none` / `single`:
  `where: { name: { contains: "land" }, genre: { name: { eq: "Techno" } } }`.
- Mutations only with `@mutation`: `create…` / `update…` / `delete…` with nested
  `connect` / `disconnect` / `create`, each one atomic batch.

## Translation Rules

One operation → one parameterised statement, built as a typed Cypher AST and printed.
Every rule reflects measured LoraDB behaviour:

1. **No `OPTIONAL MATCH`**: optional and list relationships are **pattern
   comprehensions** — `head([(this)-[:IN_GENRE]->(g:Genre) | g { .name }])`
   (_verified_; ~17–130× faster than `OPTIONAL MATCH`, E2). `head()`, not `first()`
   (E10).
2. **No `COUNT { … }` / `CALL { … }` in generated reads**: counts are
   `size([pattern | 1])`.
3. **Map projections** (`n { .a, .b }`, _verified_) — never `RETURN n`.
4. **Everything bounded**: every list has a `LIMIT`; no variable-length patterns;
   every statement runs with a `timeoutMs`.
5. **Keyset predicates written out**: `s > $s OR (s = $s AND key > $key)` — never
   `[s, key] > $after`, which silently returns no rows (E9, _verified_).
6. **Parameters for all values**; labels, types and property names only from the
   validated model.
7. **Specialised, not generic, predicates**: an absent optional filter is left out of
   the statement, not written as `($p IS NULL OR n.p = $p)` — a generic predicate hides
   the property from the planner, so no index is used. See S3.
8. **Names are declared before they're queried** (E1): until the engine can declare
   labels and properties, `assertSchema()` maintains a catalog, and a MATCH on an empty
   label returns no rows.

Example — this operation (only `name.contains` supplied, `after` absent):

```graphql
{
  festivalsConnection(
    first: 2
    where: { name: { contains: "land" } }
    sort: [{ name: ASC }]
  ) {
    edges {
      node {
        key
        name
        genre {
          name
        }
        followerCount
      }
    }
    pageInfo {
      hasNextPage
      endCursor
    }
  }
}
```

compiles to (shape, not final text):

```cypher
MATCH (this:Festival)
WHERE this.name CONTAINS $p0
WITH this ORDER BY this.name ASC, this.key ASC LIMIT $first
RETURN this {
  .key, .name,
  genre: head([(this)-[:IN_GENRE]->(g:Genre) | g { .name }]),
  followerCount: size([(this)<-[:FOLLOWS]-() | 1])
} AS node, this.name AS cursorName, this.key AS cursorKey
```

— and because `@filterable(byValue: [CONTAINS])` on `name` inferred a TEXT index, its
plan starts with `NodeByTextScan`, which the plan check (S2) asserts.

## The Smart Layer

### S1 — Indexes inferred from the API

The public API already says how each property will be queried, so the library derives
the indexes instead of trusting the author to declare them:

| API declares                                      | Required index          | Plan it produces (_verified_) |
| ------------------------------------------------- | ----------------------- | ----------------------------- |
| `@key` / `@unique`                                | uniqueness constraint   | `NodeByPropertyScan`          |
| `@filterable(EQ, IN)`                             | none (lazy exact-match) | `NodeByPropertyScan`          |
| `@filterable(LT, LTE, GT, GTE)`, `@sortable`      | RANGE                   | `NodeByPropertyRangeScan`     |
| `@filterable(CONTAINS, STARTS_WITH, ENDS_WITH)`   | TEXT                    | `NodeByTextScan`              |
| `Point` with `@filterable(WITHIN_BBOX, DISTANCE)` | POINT                   | `NodeByPointScan`             |
| `@fulltext`                                       | FULLTEXT                | (E4)                          |

`assertSchema()` creates or verifies exactly this set (explicit `@index` adds to it);
`lora-graphql check` lists indexes that exist but nothing uses, and filters that would
need an index the database lacks.

### S2 — Plans are checked, not hoped for

For every generated operation _shape_ — each TCK case, each persisted operation, and a
sampled set of variants per type — `check` runs `explain()` against a database with the
asserted schema and fails on:

- a `NodeByLabelScan` (full scan) feeding a filter that has an index kind in S1;
- a `mutating` plan behind a `Query` field, or a `readOnly` plan behind a mutation;
- `resultColumns` that don't match what the compiler projects;
- (with `profile()` on the seeded check database) an operator touching more rows than the
  shape's budget — e.g. a relationship filter that wasn't anchored (S6);
- any construct the Translation Rules forbid.

Each plan's operator sequence is snapshotted, so a change in how LoraDB plans a query —
better or worse — shows up in review. When `estimatedRows` arrives (the engine's cost
model), `check` adds per-shape row budgets.

### S3 — Compiled once, specialised by variables

- **Variants**: an operation compiles to one statement per _variable shape_ — which
  optional filters, cursors and sorts are actually present — so every variant keeps its
  predicates index-friendly (rule 7). Variants per operation are capped (default 32);
  past the cap the rarest shapes share a generic statement, and `check` reports it.
- **Ahead of time**: `lora-graphql compile` takes the application's persisted
  operations (e.g. the manifest graphql-codegen's client preset produces) and emits a
  manifest per operation hash: statements per variant, parameter mapping, result shape,
  read/write-sets, and the S2 plan snapshot. At runtime a persisted operation does **no
  translation at all** — look up, bind, execute.
- **Ad hoc**: other operations go through an LRU plan cache keyed by
  (document hash, variable shape), so translation cost is paid once per shape.

### S4 — `@cypher` is checked before it runs

Hand-written statements get the checks generated ones do:

- **Offline** (no database), with the `lora-query` WASM parser: syntax;
  `$parameters` used ⊆ field arguments ∪ `{this, jwt}` (an unknown `$param` is an
  error, an unused argument a warning); labels, relationship types and properties
  exist in the model; forbidden constructs (rule 1–5) flagged.
- **Online** (`check`, or at `getSchema()`), with `explain()`: the statement compiles;
  `columnName` is among `resultColumns`; the plan shape matches the field (`readOnly`
  on `Query` and object fields, `mutating` allowed only on `Mutation`); S2's scan rules
  apply.

A broken `@cypher` fails the build with the type, field, offending span and LoraDB's
message — never the first request.

### S5 — Read-sets, write-sets, and what changed

Every compiled statement carries its **read-set** (labels, relationship types, and —
when the operation is anchored on keys — the keys) and every mutation its
**write-set** (labels and keys created, updated, deleted; relationships connected or
disconnected). The library uses them to:

- **invalidate precisely**: `lora.onWrite(change => …)` reports entities as
  `{ type, key }` (and label-level for bulk writes), ready for a response cache's
  invalidation API — e.g. GraphQL Yoga's response cache;
- **emit change events** for subscriptions on writes made through the library (writes
  made elsewhere need engine CDC — out of scope);
- **reason about conflicts**: two mutations with disjoint write-sets can run without
  ordering constraints, groundwork for the engine's concurrent-writes plan
  (`docs/design/concurrency-implementation-plan.md`).

### S6 — Statistics-aware planning and cost

`lora-graphql analyze` (and `lora.analyze()` at startup, bounded) samples label counts
and relationship degree distributions (median, p99, max per relationship type and
direction). The compiler uses them to:

- **choose the anchor**: for `festivals(where: { followers: { some: { key: { eq: $u } } } })`
  start from the user (a key seek) and expand, instead of scanning festivals. LoraDB
  doesn't reorder this itself (no join ordering — `known-risks.md`). _Verified_ on 1,000
  festivals / 100 users: the filter form plans `NodeByLabelScan` and touches 1,000 rows
  (0.46 ms); the anchored form plans `NodeByPropertyScan ← Expand` and touches 10
  (0.01 ms) — same result, ~46× faster, and the gap grows with the label;
- **estimate work** per operation (rows × fan-out through each relationship, using p99
  degree or `@cardinality`) and reject operations over the budget _before_ running
  them — a cost limit grounded in the data, instead of a static depth limit;
- **size variants**: the S3 variant cap favours the shapes the estimates say are
  expensive.

Until `analyze` has run, `@cardinality` declarations (or a conservative default)
stand in.

### S7 — One SDL, two diffs

Because the annotated SDL is the source of both the database schema and the public API,
`lora-graphql diff old.graphql new.graphql` reports both consequences of one change:

- **database**: the constraints, indexes and catalog entries to add or drop (drops
  flagged destructive), as statements for the application's migration tool;
- **API**: breaking and dangerous changes to the printed public SDL (via
  `@graphql-inspector/core`, which supports graphql 16 and 17 — graphql-js 17 removed
  `findBreakingChanges`).

A renamed field with `@alias` keeps the stored property — the diff shows an API break
with no data migration; a changed `@filterable` shows the index it adds or drops.

### S8 — Typed end to end

`compile` also emits TypeScript types for every persisted operation's result and
variables, and for each `@cypher` field's parameters — so the consumer's resolvers,
clients and hand-written statements are checked by `tsc`, not at runtime.

## Engine Prerequisites

Behaviours of LoraDB 0.15.0 that constrain the library (reproductions in Festimap's
`docs/LORADB_REQUESTS.md`). The plan works around each; every fix removes a workaround.

| #   | Behaviour today                                                                                                                               | Needed for                                                                              | Workaround until fixed                                                 |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| E1  | Labels/properties exist only while stored data carries them; unknown names are compile-time errors; the check is skipped on an empty database | Stable reads on empty labels; offline S4 name checks matching the engine                | Catalog node from `assertSchema()`; empty-label errors → empty results |
| E2  | `OPTIONAL MATCH` ~17–130× slower than a pattern comprehension                                                                                 | — (rule 1 avoids it)                                                                    | Pattern comprehensions only                                            |
| E3  | Schema commands rejected inside `transaction()`                                                                                               | Atomic `assertSchema`                                                                   | DDL one idempotent statement at a time                                 |
| E4  | No `CALL … YIELD`; procedure results lose labels/properties                                                                                   | `@fulltext`, procedure-backed `@cypher`                                                 | `@fulltext` disabled                                                   |
| E5  | No index-ordered `ORDER BY … LIMIT` (`Sort` over a full scan, _verified_)                                                                     | Connection latency independent of label size; S2 would otherwise flag every sorted page | S2 exempts `Sort{top_k}` until fixed, and reports it                   |
| E6  | Integers above 2^53 silently rounded                                                                                                          | A `BigInt` scalar                                                                       | `Int` only, validated safe                                             |
| E7  | No interactive transactions in the binding                                                                                                    | Caller-owned `executionContext`                                                         | Mutations as one batch                                                 |
| E8  | Per-query timeout / cancellation                                                                                                              | Rule 4                                                                                  | **In progress** in `lora-node` — use it                                |
| E9  | List comparison (`[a, b] > $list`) silently returns no rows                                                                                   | Compact keyset predicates                                                               | Expanded predicates (rule 5)                                           |
| E10 | `first()` unknown although listed in the support matrix                                                                                       | —                                                                                       | `head()`                                                               |
| E11 | `explain()` has no `estimatedRows` yet                                                                                                        | Engine-side cost for S2/S6                                                              | The library's own estimates (S6)                                       |
| E12 | `lora-query`'s parser ships as a bundler-target WASM only                                                                                     | Offline S4 in Node / CLI                                                                | Build a `--target nodejs` (or `web` + `init`) variant                  |

## Phases

Every phase: `yarn typecheck`, `yarn test`, `yarn lint` green; new behaviour covered by
TCK cases (GraphQL in → Cypher + params out, snapshotted) and integration tests against
an in-memory database.

### Phase 0: Skeleton, Driver, Parser Build

- `packages/lora-graphql` (Vite library, Vitest, ESLint, `tsc`) in the workspace scripts.
- `LoraDriver`: `execute(statement, params, { timeoutMs, signal, readOnly })`,
  `transaction(…)`, `explain(…)`; adapters for `lora-node` and `lora-wasm`; LoraDB value
  decoding (`{kind: "point" | "date" | "datetime", …}`), safe-int validation (E6).
- A Node-loadable build of the `lora-query` parser (E12), consumed by `analyze/`.

Exit: `RETURN 1` and an `explain()` through both drivers; the parser loads in Node and
reports a syntax error with its span.

### Phase 1: Model, Index Inference, Schema Assertion

- Directive SDL; model validation (one `@key`, relationship targets are nodes,
  SCREAMING_SNAKE relationship types, one type per property name, reserved names, non-null
  list items) — every problem at once, with type and field.
- **S1**: the index set inferred from `@key` / `@unique` / `@filterable` / `@sortable` /
  `@index`; `assertSchema({ create })` verifies (`SHOW CONSTRAINTS` / `SHOW INDEXES`) or
  creates it, plus the catalog (E1).

Exit: validation tests for every rule; inference tests for every row of the S1 table;
`assertSchema` idempotent on fresh and populated databases.

### Phase 2: Read API, Variants, Plan Checks

- Public read schema (types, `where` / `sort`, connections with keyset cursors,
  relationship fields, `@limit`, `@private`, `@alias`, `@relayId` + `node`),
  `printPublicSchema()`.
- The compiler: typed Cypher AST, the Translation Rules, **S3 variants** and the ad-hoc
  plan cache.
- **S2** plan checks over every TCK case; `lora-graphql check` / `print`.

Exit: TCK covering filters, sorts, nesting, connections, pagination and variants;
integration results match expectations; `check` fails on a planted full scan and passes
on the real schema; zero `OPTIONAL MATCH` / `COUNT {` / list comparison in any generated
statement.

### Phase 3: `@cypher` With Static Analysis

- On `Query` / `Mutation` fields: run with arguments as parameters; nodes returned are
  projected with the selection set. On object fields: inlined when `RETURN <expr>`,
  otherwise batched per parent (`UNWIND $parents …`, _verified_ shape).
- **S4**: offline lint (parser) and online checks (`explain()`), both in `check` and at
  `getSchema()`.

Exit: TCK + integration for each placement; each S4 rule has a failing fixture with an
exact message and span.

### Phase 4: Opt-In Mutations With Write-Sets

- Generated only for `@mutation` types; inputs validated against the model before any
  write; `@default` / `@timestamp`; one atomic batch per mutation; constraint violations →
  a typed `CONSTRAINT_VIOLATION` error naming the field.
- **S5 write-sets** computed for every mutation; `onWrite` events.

Exit: TCK + integration for every operation incl. nested connect/create and a failing
constraint that writes nothing; write-set assertions for each.

### Phase 5: Authorization

- `@authentication`, `@authorization(filter, validate)` over `$jwt.<claim>`, compiled into
  predicates and post-write validations; applies to `@cypher` too. Rules are part of the
  variant key (S3), so authorised statements stay index-friendly.

Exit: filtered rows are never returned; failed validations write nothing; plan checks
still pass with rules applied.

### Phase 6: Ahead-Of-Time Compilation And Types

- **S3 AOT**: `lora-graphql compile` over a persisted-operations manifest → compiled
  manifest (statements per variant, parameter map, result shape, read/write-sets, plan
  snapshot); runtime executes it without translating.
- **S8**: TypeScript types for operation results/variables and `@cypher` parameters.

Exit: a persisted operation runs with no call into the translator (asserted); compiled
and ad-hoc paths return identical results for every TCK case.

### Phase 7: Change Tracking

- **S5** complete: read-sets on every statement; `onWrite` with entity-level changes;
  an adapter for GraphQL Yoga's response cache invalidation; in-process change events
  for subscriptions on library-made writes.

Exit: for every mutation TCK case, every cached query whose read-set intersects the
write-set is invalidated — and no other.

### Phase 8: Statistics-Aware Planning

- **S6**: `analyze` (label counts, degree distributions), anchor selection, cost
  estimates with an operation budget, `@cardinality` fallback.

Exit: on a seeded skewed graph, anchor selection turns a planted label scan into a key
seek (S2 shows it); an operation over budget is rejected before execution with its
estimate in the error.

### Phase 9: Schema Evolution

- **S7**: `lora-graphql diff` — database statements (destructive flagged) and API
  breaking/dangerous changes from two SDL versions.

Exit: fixtures for add/remove/rename-with-`@alias`/change-`@filterable`/change-type, each
with the expected DDL and API report.

### Phase 10: Hardening And Release

- Performance guard: every TCK shape benchmarked on a seeded dataset (e.g. 20k nodes /
  100k relationships); >15% regression fails, like `scripts/check-bench-delta.mjs`.
- Docs on loradb.com: getting started, directive reference, generated API, the smart
  layer, translation rules, a migration guide from `@neo4j/graphql`.
- Release `@loradb/lora-graphql` with the binding version it was tested against.

## Lessons From `@neo4j/graphql`

- **Keep:** schema-first SDL, `@node` / `@relationship` / `@cypher`, one statement per
  operation, authorization compiled into the query, nested filter inputs.
- **Avoid:** generating every mutation and operator for every type; subquery-heavy
  translation; generic `IS NULL OR` predicates that defeat indexes; leaving index
  choice, plan quality and cache invalidation to the application.

## Consumer: Festimap

Festimap is the first user and the acceptance test:

- Its `@festimap/schema` package (API SDL + graph model, generating DDL, record types and
  resolver types) collapses into one annotated SDL; its repositories, resolvers,
  DataLoaders and query-budget tests are replaced by the generated schema, S2 and S3.
- Its persisted-operations manifest (Phase 4 of its API plan) feeds `compile` (S3); its
  response cache takes `onWrite` (S5).
- Business rules beyond `$jwt` predicates stay in its service layer, calling generated
  operations or `@cypher` mutations.

## Later

- A Rust `lora-graphql` crate compiling operations straight to LoraDB plans (no Cypher
  text), and a GraphQL endpoint in `lora-server`.
- Engine-side cost (`estimatedRows`) and change-data capture replacing S6 sampling and
  S5's library-only events.
- Editor support: `@cypher` statements highlighted and validated inside `.graphql` files
  with the `lora-query` CodeMirror editor.
- `lora-graphql introspect`: a starting annotated SDL from an existing database.

## Open Decisions

1. **Filter syntax**: nested operator objects (proposed) or suffixed fields
   (`name_CONTAINS`).
2. **Offset pagination**: omit, or allow per type with a hard cap.
3. **Where authorization lives**: in the library (Phase 5), or in the consumer with only
   `$jwt` passed to `@cypher`.
4. **Variant cap and fallback**: the default cap (32) and whether the fallback statement is
   generic or the operation is rejected.
5. **Package vs crate first**: TypeScript first for the GraphQL ecosystem, the Rust crate
   once translation is stable.
