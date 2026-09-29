---
slug: loradb-v0-16-graphql
title: "LoraDB v0.16: GraphQL, and an engine that finds the seek"
description: "LoraDB v0.16 ships @loradb/lora-graphql, a schema-first GraphQL layer that compiles every operation to Cypher, plus condition pushdown, early LIMIT in transactions, temporal range indexes, writes in CALL subqueries and a committed change feed in the engine."
authors: [loradb]
tags: [release-notes, announcement, graphql, cypher, performance, indexing, tooling]
---

LoraDB v0.16 adds a GraphQL API, and a planner that no longer needs
help finding an index.

The headline is a new package, `@loradb/lora-graphql`. One annotated
schema becomes an executable GraphQL API whose operations compile to
parameterised Cypher. Building it put the engine under a kind of load
the Cypher examples never did, and most of this release's engine work
came out of that: condition pushdown, `IN` seeks, early `LIMIT` inside
transactions, temporal range indexes, writes in `CALL { }`, and a
committed change feed.

<!-- truncate -->

## `@loradb/lora-graphql`

```graphql
type Festival @node @mutation @query(aggregate: true) {
  key: ID! @key(generate: true)
  name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  capacity: Int @filterable(byValue: [GT, LT])
  followers: [User!]! @relationship(type: "FOLLOWS", direction: IN)
}
```

```ts
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";

const db = await createDatabase();
const lora = new LoraGraphQL({ typeDefs, driver: loraDriver(db) });
await lora.assertSchema({ create: true });
const schema = lora.getSchema(); // any graphql-js server
```

What makes it more than a resolver generator:

- **Opt-in surface.** Types are readable by default; mutations,
  subscriptions, filters and sorts are opted into per type and per
  field.
- **Indexes from the API.** Every `@filterable` and `@sortable` field
  implies the index it needs, and `assertSchema` creates them.
- **Plans are checked.** `lora-graphql check` plans every operation
  against an in-memory database and fails CI on a label scan where a
  seek was expected, on a plan that changed against a baseline, or on a
  statement the engine estimates to scan more than a budget.
- **Authorization compiled into the statement.** Claim checks are
  folded at compile time; row rules become `WHERE` predicates, so a
  list, a count, an aggregate and a cursor all see the same rows.
- **Exact write-sets.** Mutations address nodes by key and report
  exactly what they wrote, for cache invalidation and subscriptions.

It covers interfaces and unions, keyset connections in both
directions, aggregates (grouped too), full-text and vector search,
nested writes, upserts, bulk writes bounded by a limit, custom
resolvers and scalars, and a `migrate neo4j` command for existing
`@neo4j/graphql` schemas. Production defaults are safe ones: database
errors are masked, introspection is off, and documents are bounded in
depth, aliases, root fields and tokens when `NODE_ENV` is
`production`.

The [GraphQL docs](/docs/graphql) start from installation.

## The engine found the seek

A mutation that connects a festival to a follower used to run a
statement like this:

```cypher
MATCH (a:Festival)<-[r:FOLLOWS]-(b:User)
WHERE a.key = $from AND b.key = $to
```

The optimizer only turned a key test into an index seek when the test
sat directly on a node scan, so this one expanded every `FOLLOWS`
relationship first: 175 ms on 20,000 festivals. The planner now splits
a `WHERE` into its conjuncts, places each on the operator that binds
its variables, and starts a pattern from whichever end can seek. The
same statement takes 0.007 ms.

Around it:

- `x.key IN $list` seeks once per distinct element (14 ms to 0.02 ms
  for 20 keys).
- `MATCH (n:L) RETURN count(n)` answers from label counts.
- `MERGE (a)-[r:T]->(b)` with both ends bound honours `b`, and pattern
  comprehensions do too.
- Later nodes in a pattern have their labels tested.

## Transactions and time

- Reads under a deadline or inside a transaction stop at `LIMIT`: a
  `LIMIT 10` read in a transaction went from 21 ms to 0.1 ms.
- RANGE indexes hold `Date`, `DateTime`, `LocalDateTime`, `Time` and
  `LocalTime`, ordered by instant. A newest-first feed over 20,000 posts
  takes 0.014 ms instead of 8.1 ms, and no longer returns nothing.
- `CALL { }` subqueries may write.
- `null` values in a property map are not stored, and `SET n += {a:
  null}` removes `a`.
- Existence constraints are checked when a statement finishes, so
  `CREATE` followed by `SET` of a required property works.

Writes also got cheaper as graphs grow: secondary indexes are
copy-on-write, so a write no longer clones whole indexes.

## A committed change feed

`db.changes()` in the Node binding yields every committed write in
commit order, with a resume position. With a WAL it resumes across
restarts. A consumer that falls behind is told where to resume;
writers never wait for it. The GraphQL layer uses it for
subscriptions that see writes from any process with
`changeFeed: true`.

## Node binding

- 64-bit integers arrive as `bigint` when they exceed JavaScript's
  safe range.
- `timeoutMs` and `AbortSignal` per call, and a database-wide default.
- Interactive transactions with `db.begin()`, and `tx.executeMany()`
  to run a batch in one native call.
- A typed `LORA_LOCKED` error for a directory another process holds.
- Prebuilt binaries for musl (Alpine) on x64 and arm64.

## Fixed

- A filter whose range could not match anything, such as
  `x > 5 AND x < 5` on an indexed property, panicked and took the host
  process down. It returns no rows.
- Comparisons on `LocalDateTime`, `Time` and `LocalTime` always
  returned false, and `ORDER BY` on them sorted by text.

## Behaviour changes

- A `MERGE` that matches only part of its pattern creates the whole
  pattern, as Cypher specifies. It used to reuse the half match.
- Rows without `ORDER BY` may come back in a different order when the
  planner walks a pattern from the other end.

## Known issues

These are recorded, not fixed yet: `max`, `sum` and `avg` over
durations are wrong (the GraphQL layer works around it),
`date('...')` and `datetime('...')` return null while the cast forms
work, `min` and `max` treat `LocalDateTime`, `Time` and `LocalTime`
values as equal, and an index lookup for `5` misses `5.0`.
