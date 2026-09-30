---
title: Migrating from @neo4j/graphql
sidebar_label: Migrating from @neo4j/graphql
description: How @loradb/lora-graphql differs from @neo4j/graphql, what the lora-graphql migrate neo4j command rewrites for you, and what needs a manual decision.
---

# Migrating from @neo4j/graphql

`@loradb/lora-graphql` reads a similar annotated SDL and keeps many
directive names (`@node`, `@relationship`, `@cypher`, `@authorization`),
so most schemas port with a handful of edits. The generated API is
deliberately smaller, though, and some behaviour differs in ways clients
will notice. Read the comparison before you move a production API.

## At a glance

| @neo4j/graphql | lora-graphql |
| --- | --- |
| Every type gets every operation | Reads by default; mutations and subscriptions opt in with `@mutation` and `@subscription` |
| Every field filterable by every operator | `@filterable(byValue:)`, checked against the type |
| Every field sortable | `@sortable` |
| Offset cursors (`arrayconnection:N`) | Keyset cursors tagged with their sort, optionally signed |
| `update` / `delete` with an optional `where` | By `@key`, or bulk with a required, bounded `where` |
| `connect: { where }`, silently a no-op when nothing matches | Connect by key; a missing target is `NOT_FOUND` |
| Single relationships not enforced; required ones refused | Enforced from both sides; required ones supported |
| `{ viewers: { add: 1 } }` in `update` | `adjust: { viewers: { add: 1 } }` |
| `@id` | `@key(generate: true)` |
| `@populatedBy`, `@timestamp` | Same names |
| `@authorization` rules evaluated in Cypher | Claim checks folded in JavaScript at compile time; node rules compiled into the statement |
| JWT decoded and verified by the library | Your server verifies the JWT; the library reads verified claims |
| You create indexes | Inferred from the API, created by `assertSchema()`, verified with `explain()` |
| Unbounded lists; query complexity left to you | Every list bounded; a cost limit per operation |
| Subscriptions over CDC | `@subscription`, from library writes, or every committed write with `changeFeed: true` |
| Interfaces and unions with `UNION` | Per-member subqueries merged by sort, each using its own indexes |
| `connectOrCreate` | `upsert<Plural>` |
| Federation | Not supported |

## Run the migration command

```bash
lora-graphql migrate neo4j schema.graphql --operations src/operations
```

`lora-graphql migrate neo4j <schema.graphql> [--operations <file|dir>]`
prints the rewritten SDL on standard output, preceded by one
`# TODO(migrate): ...` line per decision left to you:

- `@id` becomes `@key(generate: true)`, and `@node` is added where it is
  missing.
- `@fulltext`, `@subscription(events:)` and the `@relationship`
  arguments (`queryDirection`, `nestedOperations`) are translated.
- `@mutation`, `@filterable` and `@sortable` are opt-in here. With
  `--operations` (your client operations, in either the neo4j 5
  `title_CONTAINS` or the neo4j 6 `{ title: { contains } }` form), they
  follow what the operations use. Without it, every type keeps its
  mutations, as in `@neo4j/graphql`, and a TODO says to narrow them.
- What has no equivalent (`@coalesce`, `@exclude`,
  `@subscriptionsAuthorization`, the federation directives,
  `connectOrCreate`, type-level `@vector`) is removed and listed as a TODO. `@authorization`
  and `@authentication` rules are kept, with a TODO to check their filter
  operators.

The output is a draft, not a finished schema. Save it
(`lora-graphql migrate neo4j schema.graphql > schema.lora.graphql`), diff
it against the original, resolve every TODO, and then run the checks
below.

## Then check it

```bash
lora-graphql print schema.lora.graphql # the public API clients will see
lora-graphql check schema.lora.graphql --operations src/operations
```

`print` shows the generated API without directives, so you can compare it
with the old one. `check` compiles every operation against the new schema
and plans it, so an operation that uses a filter you did not opt in, or a
query that would scan a label, fails here instead of in production.

## What changes for clients

These are the differences clients see, roughly in order of how often they
bite:

1. **Fewer filters and sorts.** Only what the schema opts in. Operations
   that use anything else fail validation. Collect your operations and run
   `check` against them.
2. **Keyset pagination.** Cursors are opaque and tied to their sort.
   Replaying a cursor under another sort is `INVALID_CURSOR`, and there is
   no offset.
3. **Bounded lists.** `limit` and `first` default to 25 and fail above
   100 (both configurable per type with `@limit`, and globally). A client
   that relied on unbounded lists must page.
4. **Keys, not `where`, for single writes.** `updateFestival(key:)` and
   `deleteFestival(key:)` address one node. Bulk `updateFestivals` and
   `deleteFestivals` need a non-empty `where` and fail, writing nothing,
   when more than `limit` nodes match.
5. **Connect fails loudly.** Connecting to a key that does not exist, or
   that the caller cannot see, is `NOT_FOUND` and rolls the mutation back.
6. **Math and list operators move to `adjust`.**
7. **Errors carry codes.** `extensions.code` is one of `BAD_USER_INPUT`,
   `INVALID_CURSOR`, `LIMIT_EXCEEDED`, `COST_EXCEEDED`, `UNAUTHENTICATED`,
   `FORBIDDEN`, `NOT_FOUND`, `CONSTRAINT_VIOLATION` or `DATABASE_ERROR`,
   plus `PERSISTED_QUERY_ONLY` when the server accepts persisted
   operations only. See [errors](/docs/graphql/errors).

## What changes on the server

- **JWT verification moves to your server.** `@neo4j/graphql` can decode
  and verify tokens; lora-graphql never does. Verify the token and put the
  claims in the context as `jwt`. See
  [authorization](/docs/graphql/authorization).
- **Claims should be declared.** Add a `@jwt` type so rules that reference
  an unknown claim fail at startup.
- **Indexes come from the API.** Drop hand-written index DDL for what the
  API uses and run `assertSchema({ create: true })`, or apply
  `lora-graphql requirements --ddl` in your migrations.
- **Subscriptions follow library writes unless you opt into the feed.**
  By default events come from mutations made through the `LoraGraphQL`
  instance, and `@cypher` mutations produce a broad change. With
  `changeFeed: true` (lora-node) they come from the engine's committed
  change feed instead, which covers every committed write to the database
  from the process that owns it, raw Cypher included. It is not CDC: a
  database is open in one process at a time, and the feed starts at the
  current commit, so writes made while the server was down are not
  replayed. `migrate` adds a TODO to every `@subscription` it rewrites.
- **`@cypher` is the same idea with stricter checks.** Parameters must be
  field arguments or `$jwt`, and statements on `Query` fields may not
  write. `check` plans every statement.

## Not planned

These `@neo4j/graphql` features are left out on purpose:

- Offset pagination and `arrayconnection` cursors.
- `update` and `delete` with an optional, unbounded `where`.
- Every operation on every type, every filter on every field, and the
  deprecated flat filter aliases.
- `unsafeEscapeOptions`.
- JWT verification in the library.
- Vector providers that call external embedding APIs from the database
  layer.
- A single relationship that silently returns the first of several.
  Cardinality is enforced instead.
- `connectOrCreate`, which neo4j 7 removed; `upsert` covers the need.
- Federation, unless there is demand for it.
