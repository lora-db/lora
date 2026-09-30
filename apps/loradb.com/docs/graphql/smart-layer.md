---
title: The GraphQL Smart Layer
sidebar_label: Smart layer
description: How @loradb/lora-graphql reasons about the Cypher it generates, from inferred indexes and plan checks to compile caching, @cypher checks, write-sets, cost limits, schema diffs and typed tooling.
---

# The smart layer

A GraphQL layer that only translates queries leaves the hard parts to you:
which indexes exist, whether the plan uses them, how expensive a request
is, and what a mutation changed. `@loradb/lora-graphql` knows the API and
the statements it compiles, so it answers those questions itself. The
parts are numbered S1 to S8.

| Part | What it does | Where you see it |
| --- | --- | --- |
| [S1](#s1-inferred-indexes) | Derives constraints and indexes from the API | `requirements()`, `assertSchema()` |
| [S2](#s2-plan-checks) | Checks that plans use them | `explain()`, `check()`, `lora-graphql check` |
| [S3](#s3-compile-cache) | Compiles once, caches the rest | `persist()`, `execute()` |
| [S4](#s4-cypher-checks) | Checks `@cypher` statements | startup, `check()` |
| [S5](#s5-write-sets-and-change-tracking) | Reports exactly what each mutation wrote | `onWrite()`, `changes()`, `affects()` |
| [S6](#s6-statistics-and-cost) | Estimates cost and refuses expensive operations | `maxCost`, `analyze()` |
| [S7](#s7-schema-diff) | Diffs two SDLs into migrations and API breaks | `diffSchemas()`, `lora-graphql diff` |
| [S8](#s8-typed-tooling) | Validates operations at build time and types them | `lora-graphql compile`, `loadManifest()` |

## S1: inferred indexes {#s1-inferred-indexes}

`lora.requirements()` derives every constraint and index the API needs,
with the reason for each:

| API declares | Needs |
| --- | --- |
| `@key` | node key constraint |
| `@unique` | uniqueness constraint |
| non-null `@sortable` field | existence constraint |
| `EQ`, `IN` | nothing: LoraDB indexes equality lazily |
| `LT`, `LTE`, `GT`, `GTE`, `@sortable` | RANGE index |
| `CONTAINS`, `STARTS_WITH`, `ENDS_WITH` | TEXT index |
| `WITHIN_BBOX`, `DISTANCE` | POINT index |
| `@fulltext`, `@vector` | FULLTEXT and VECTOR indexes, by name |

Interface fields are inferred for every implementation's label. Indexes
and constraints go on a type's primary (first) label. A `@key` or `@unique`
field needs no separate RANGE index: its constraint's index serves. Duration
fields get no RANGE index (durations have no total order), and
relationship properties get no index at all.

```ts
const { missing } = await lora.assertSchema(); // report only
await lora.assertSchema({ create: true }); // create what is missing
```

`assertSchema({ create: true })` is idempotent. To review the DDL instead,
print it:

```bash
lora-graphql requirements schema.graphql --ddl
```

Some filters cannot use an index at all: `CASE_INSENSITIVE` compares
lowercased values, and `IS_NULL` looks for absent properties. `check`
lists them as lint notes. For search over large labels, prefer
`@fulltext`.

## S2: plan checks {#s2-plan-checks}

Every compiled statement records the access path it was written for.
`lora.explain(query, variables)` plans each statement of an operation and
reports:

- a label scan where a seek was expected;
- a mutating plan behind a read;
- result columns that do not match what the projection expects.

`lora.check({ operations })` runs this over a set of operations, and the
CLI runs it in CI:

```bash
lora-graphql check schema.graphql --operations src/operations
```

`check` builds an in-memory LoraDB (it needs `@loradb/lora-node`), asserts
the schema, plans every `@cypher` statement and every query in the
operation files, and exits non-zero on any finding. Options:

- `--variables vars.json`: variables per operation name, instead of sample
  values for the required ones.
- `--baseline plans.json`: records the operators of every statement. A
  plan that differs fails, so plan changes show up in review.
  `--update-baseline` accepts them; a missing file is written.
- `--row-budget n`: fails statements the engine estimates to scan more
  rows. Off by default, because estimates ignore a `LIMIT` that stops an
  index-ordered scan early.
- `--database dir [--name app]`: checks an existing database as it is, and
  reports indexes it has that the API does not use.

`check` also prints lint notes that do not fail the run: types with
mutations but no rules, filters no index applies to, and list
relationships without `@cardinality` or statistics.

`check` plans queries. Mutation statements need the data they write
against, so they are not plan-checked.

## S3: compile cache {#s3-compile-cache}

Translating a nested page takes about 0.13 ms. The library avoids even
that where it can:

- `lora.persist({ id: source })` parses and validates persisted operations
  at startup; `lora.execute({ id, variables, context })` runs them with no
  parsing or validation.
- Ad hoc documents passed to `execute({ source })` are cached by source
  text.
- Each read root field caches its compiled statements per field node,
  exact variables, claims, and the `$context` values the compile read. A
  repeated `festivals(limit: 20)` drops from 0.14 ms to 0.06 ms end to end.
- Statement text depends only on the shape of the input, not on values, so
  LoraDB's own plan cache is hit for every repeat.

What this does not do: statement text is not precompiled ahead of time.
It depends on which variables are set (absent filters are left out) and on
the caller's claims (claim checks are folded in), so it is compiled per
request and then cached. Servers that parse every request themselves
produce new field nodes each time and miss the per-field cache; use
`execute()`, persisted operations or a server with a document cache.

## S4: @cypher checks {#s4-cypher-checks}

`@cypher` statements are checked, not trusted blindly:

- **At startup:** every `$parameter` must be a field argument or `$jwt`,
  and statements on `Query` and object fields may not contain write
  clauses. Unused arguments, `OPTIONAL MATCH` and statements that never use
  `this` are warnings in `lora.model.warnings`.
- **In `check()`:** every statement is planned with `explain()`, so a
  syntax error, an unknown function or a missing column fails CI with the
  engine's message instead of failing the first request.

A scalar `@cypher` field may be `@filterable` and `@sortable` in root
fields. The statement then runs per node before the filter, and no index
applies; the model warns so `check` reports it.

These checks catch mistakes, not malice. `@cypher` statements run with the
database's full access and are trusted code: review them like any other
server code.

## S5: write-sets and change tracking {#s5-write-sets-and-change-tracking}

Mutations address nodes by `@key`, and bulk writes resolve their keys
first, so the library knows exactly what each mutation wrote. After every
commit it reports a `WriteChange`:

| Field | Holds |
| --- | --- |
| `operation` | `CREATE`, `UPDATE`, `DELETE`, `UPSERT`, `CYPHER`, or `EXTERNAL` for changes from the engine feed |
| `field` | The `Mutation` field that made the change |
| `created`, `updated`, `deleted` | Nodes, as `{ type, key }` |
| `connected`, `disconnected` | Relationships, by declaring field and both keys |
| `entities` | Every node whose observable state changed, relationship ends included |
| `types`, `relationshipTypes` | The node and relationship types touched |
| `broad` | `true` for `@cypher` mutations, whose write-set is unknown |
| `timestamp` | When the write was committed |
| `before` | Stored values before the write, for types with `@subscription(previousState: true)` |

```ts
lora.onWrite((change) => {
  // Invalidate a response cache (GraphQL Yoga's response cache shown):
  // every cached result that holds a node of a touched type.
  const types = change.broad ? [...lora.model.nodes.keys()] : change.types;
  cache.invalidate(types.map((typename) => ({ typename })));
});

for await (const change of lora.changes({ signal })) publish(change);
```

Every compiled read also carries a read-set (labels and relationship
types), and `lora.affects(reads, change)` tells whether a cached read may
be stale. It is label level: a read of `:Festival` nodes is affected by any
Festival write.

Limits worth knowing:

- By default only writes made through this `LoraGraphQL` instance are
  seen, and `@cypher` mutations are reported with `broad: true` and no
  entities: treat everything as changed.
- With `changeFeed: true` (lora-node), `changes()` and subscriptions are
  fed by the engine's committed change feed: every committed write to that
  database, whichever path in the owning process made it (raw Cypher,
  `@cypher` mutations, other instances), with keys and relationship fields
  resolved. A database directory is open in one process at a time, so this
  is not a cross-process feed. If the library's reader falls behind the
  engine, it resumes from the last position it read. The position is kept
  in memory only: a new instance starts at the current commit, and writes
  made while no instance was running are not replayed. `onWrite` still
  reports this instance's mutations. Call `lora.close()` to stop the feed.
- A `changes()` consumer that falls `maxQueuedChanges` (default 1000)
  behind is ended with an error rather than buffered without bound.
- `onWrite` and `changes()` receive the full write-set without applying
  read rules. They are server-side hooks: do not forward them to clients
  unfiltered. Subscriptions apply the rules for you.

The [GraphQL Yoga example](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples/yoga)
wires this up and explains the choices: per type rather than per entity
(`change.entities` alone misses a cached, filtered list that a changed node
now matches), no caching for counts and aggregates (they hold no node to
tag), a cache per token, and a TTL for writes the library cannot see.

## S6: statistics and cost {#s6-statistics-and-cost}

Every operation gets a cost estimate before it runs: the rows it touches,
multiplying page sizes through nested lists, capped by `@cardinality`, and
summed across root fields. An operation over `maxCost` (default 50 000)
fails with `COST_EXCEEDED` and runs nothing. A subscription is charged per
event.

```ts
const lora = new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  maxCost: 20_000,
  budget: (context) => (context.plan === "free" ? 5_000 : undefined),
  onCost: ({ field, cost, total, limit }) => metrics.record(field, cost),
});
```

- `budget(context)` sets the limit per request, for example per plan or
  user; `undefined` falls back to `maxCost`. `onCost` sees every root
  field's estimate; see [observability](/docs/graphql/observability).
- `execute()` returns the estimate in `extensions.cost`, so clients can
  tune their queries.
- `lora.analyze()` (or `lora-graphql analyze schema.graphql --database dir`) samples node
  counts and relationship degrees. Pass the result to
  `lora.useStatistics()` and estimates use the measured p99 degree instead
  of the page size.

Statistics also shape plans: a relationship filter that names a related
node by key starts from that node and expands instead of scanning the
label.

The estimate is an upper bound on rows touched, not a measurement of time.
Use it as an admission check next to `timeoutMs` (default 10 000 ms per
statement), not instead of it.

## S7: schema diff {#s7-schema-diff}

```ts
import { diffSchemas } from "@loradb/lora-graphql";

const diff = diffSchemas(beforeSdl, afterSdl);
```

```bash
lora-graphql diff old.graphql new.graphql # exit 1 on breaking changes
```

One SDL describes both the graph and the API, so one diff reports both:

- the database statements the change needs: index and constraint changes,
  relabels and property renames, with destructive ones flagged;
- the API's breaking and dangerous changes.

A field renamed with `@alias` over the same property is reported as an API
break with no data migration.

## S8: typed tooling {#s8-typed-tooling}

`lora-graphql compile` validates persisted operations at build time:

```bash
lora-graphql compile schema.graphql --operations src/operations --out generated
```

The input is `.graphql` files or directories of them, or a JSON map of id
to source. Each operation in a `.graphql` file becomes one entry, with the
id `<file path relative to the working directory>#<OperationName>` (an
anonymous operation gets its position, starting at 1). A JSON map keeps its
own ids. It writes:

- `manifest.json`, which `lora.loadManifest(manifest)` registers as
  persisted operations without parsing or validating them again;
- `operations.d.ts`, with `<Operation>Variables` and `<Operation>Result`
  types for each operation.

```ts
import manifest from "./generated/manifest.json" with { type: "json" };

lora.loadManifest(manifest);
const result = await lora.execute({
  id: "src/operations/festivals.graphql#TopFestivals",
  variables,
  context,
});
```

Without `--out`, the files go to `lora-graphql/`. The manifest records a
hash of the public schema and is refused for any other schema, so a stale
build fails at startup. Statement text is not in
the manifest: it depends on variable values and claims, and is compiled per
request, then cached.
