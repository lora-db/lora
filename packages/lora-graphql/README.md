# @loradb/lora-graphql

Schema-first GraphQL for LoraDB. One annotated SDL describes the graph and
the public API. The library turns it into an executable `graphql-js` schema
whose operations compile to single, parameterised Cypher statements, and it
reasons about those statements: it derives the indexes they need, checks
with `explain()` that they use them, bounds their cost before they run,
compiles authorization into them, and reports exactly what every mutation
wrote.

```ts
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";
import { createYoga } from "graphql-yoga";

const typeDefs = /* GraphQL */ `
  type Festival @node @mutation @query(aggregate: true) {
    key: ID! @key(generate: true) @relayId
    name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
    capacity: Int @filterable(byValue: [GTE, LT]) @sortable
    location: Point @filterable(byValue: [WITHIN_BBOX, DISTANCE])
    createdAt: DateTime @timestamp(operations: [CREATE])
    genre: Genre @relationship(type: "IN_GENRE", direction: OUT) @filterable
    followers: [User!]!
      @relationship(type: "FOLLOWS", direction: IN, properties: "Follows")
      @filterable
    followerCount: Int!
      @cypher(statement: "RETURN size([(this)<-[:FOLLOWS]-(:User) | 1]) AS n")
  }
  type Genre @node {
    key: String! @key
    name: String! @filterable
  }
  type User @node @mutation {
    key: String! @key
    name: String @sortable
  }
  type Follows @relationshipProperties {
    since: Int @default(value: 2026)
  }
`;

const db = await createDatabase();
const lora = new LoraGraphQL({ typeDefs, driver: loraDriver(db) });
await lora.assertSchema({ create: true }); // the constraints and indexes the API needs
const yoga = createYoga({
  schema: lora.getSchema(),
  context: ({ request }) => ({
    jwt: verifiedClaims(request),
    signal: request.signal,
  }),
});
```

## Contents

- [What it generates](#what-it-generates)
- [Directives](#directives)
- [Queries](#queries)
- [Mutations](#mutations)
- [@cypher fields](#cypher-fields)
- [Authorization](#authorization)
- [The smart layer](#the-smart-layer)
- [Change tracking](#change-tracking)
- [CLI](#cli)
- [Drivers, limits and errors](#drivers-limits-and-errors)
- [Translation rules](#translation-rules)
- [Coming from @neo4j/graphql](#coming-from-neo4jgraphql)
- [LoraDB behaviours this works around](#loradb-behaviours-this-works-around)

## What it generates

For each `@node` type (reads are on by default; `@query(read: false)` turns
them off):

| Field                                                                    | Does                                                                                                      |
| ------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `festivals(where, sort, limit)`                                          | A bounded list                                                                                            |
| `festivalsConnection(where, sort, first, after, last, before)`           | Relay connection with keyset cursors in both directions, never `SKIP`                                     |
| `festival(key:)`                                                         | Lookup by `@key`                                                                                          |
| `festivalsAggregate(where)`                                              | `count`, and `min` / `max` (`avg` / `sum` for numbers) of sortable fields, with `@query(aggregate: true)` |
| `node(id:)`                                                              | Any `@relayId` type by global id                                                                          |
| `searchFestivals(query, where, limit)`                                   | Full-text search, with `@fulltext`                                                                        |
| `similarFestivals(vector or to, where, limit)`                           | Vector similarity, with a `@vector` field                                                                 |
| `createFestivals`, `upsertFestivals`, `updateFestival`, `deleteFestival` | Only with `@mutation`                                                                                     |
| `festivalChanged(key, operations)`                                       | A subscription, only with `@subscription`                                                                 |

Relationship fields take `where`, `sort` and `limit`; list relationships
also get `…Connection`, whose edges carry the relationship properties and
filter on them (`where: { node, edge }`).

The surface is restrictive: a field is filterable only with `@filterable`,
and only by the operators listed; sortable only with `@sortable`;
`printPublicSchema()` prints exactly what clients see, with no directives.

## Directives

Model:

| Directive                                                     | On                | Meaning                                                                                                                           |
| ------------------------------------------------------------- | ----------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `@node(labels:, plural:)`                                     | type              | A node label set; default: the type name                                                                                          |
| `@key(generate:)`                                             | field             | Required, unique and immutable. The sort tie-breaker, cursor anchor and mutation address. `generate: true` fills a UUID on create |
| `@unique`                                                     | field             | Uniqueness constraint                                                                                                             |
| `@index(kind: RANGE \| TEXT \| POINT)`                        | field             | An explicit index, usually inferred                                                                                               |
| `@relationship(type:, direction:, properties:)`               | field             | An edge to another `@node` type                                                                                                   |
| `@relationshipProperties`                                     | type              | Properties on a relationship type                                                                                                 |
| `@alias(property:)`                                           | field             | API name differs from the stored property                                                                                         |
| `@private`                                                    | field             | Stored, never exposed                                                                                                             |
| `@readonly`                                                   | field             | Exposed, never client-settable                                                                                                    |
| `@default(value:)`                                            | field             | Stored on create when the input omits it                                                                                          |
| `@timestamp(operations: [CREATE, UPDATE])`                    | field             | Set to the current time; never client-settable                                                                                    |
| `@cardinality(max:)`                                          | list relationship | Declared fan-out, for cost estimates                                                                                              |
| `@cypher(statement:, columnName:)`                            | field             | A field backed by a Cypher statement                                                                                              |
| `@fulltext(indexes: [{ name, fields, analyzer, queryName }])` | type              | FULLTEXT indexes, each with a search root field                                                                                   |
| `@vector(dimensions:, similarity:, queryName:)`               | `[Float!]` field  | A VECTOR index and a similarity root field                                                                                        |

API:

| Directive                                             | On                      | Meaning                                                                                |
| ----------------------------------------------------- | ----------------------- | -------------------------------------------------------------------------------------- |
| `@query(read:, aggregate:)`                           | type                    | Generated reads                                                                        |
| `@mutation(operations: [CREATE, UPDATE, DELETE])`     | type                    | Generated mutations; none without it. `upsert` needs CREATE and UPDATE                 |
| `@subscription(operations: [CREATE, UPDATE, DELETE])` | type                    | Generated subscriptions; none without it                                               |
| `@filterable(byValue: [...])`                         | field                   | Filter operators. Bare: `EQ` and `IN`. On a relationship: enables relationship filters |
| `@sortable`                                           | field                   | Sort and paginate by this field                                                        |
| `@limit(default:, max:)`                              | type, list relationship | Page size bounds                                                                       |
| `@relayId`                                            | `@key` field            | Adds a global `id` and the `Node` interface                                            |
| `@authentication(operations:)`                        | type, field             | Needs an authenticated request                                                         |
| `@authorization(filter:, validate:)`                  | type                    | Row-level rules, compiled into statements                                              |

`directiveTypeDefs` (or `lora-graphql directives`) prints these as SDL for
editors and codegen.

Filter operators: `EQ`, `IN`, `LT`, `LTE`, `GT`, `GTE`, `CONTAINS`,
`STARTS_WITH`, `ENDS_WITH`, and on points `WITHIN_BBOX` and `DISTANCE`.
Each is checked against the field's type.

Types: `String`, `ID`, `Int`, `Float`, `Boolean`, enums, `BigInt` (a decimal
string), `Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`,
`Duration` (ISO-8601 strings), `Point` and `CartesianPoint` (objects; set with
`PointInput` / `CartesianPointInput`), and non-null lists of these.

## Queries

```graphql
{
  festivalsConnection(
    first: 10
    after: $cursor
    where: {
      name: { contains: "land" }
      followers: { some: { key: { eq: "u1" } } }
      OR: [{ capacity: { gte: 5000 } }, { genre: { name: { eq: "Techno" } } }]
    }
    sort: [{ capacity: DESC }]
  ) {
    totalCount
    edges {
      cursor
      node {
        key
        name
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

- Filters nest, with `AND` / `OR` / `NOT`. List relationships take `some`,
  `all`, `none`, `single` and `count`; single relationships take the
  target's filter directly.
- **Absent and `null` filters are left out of the statement**, so a filter
  bound to an unset variable costs nothing. `eq: null` does not mean
  `IS NULL`, and a relationship quantifier whose filter is empty is left
  out too: ask `count: { gt: 0 }` for "has any".
- Connections page forward with `first` / `after` and backward with
  `last` / `before`; one cursor works in both directions.
- `sort` takes one field per item. Connections always end on a unique field
  (the `@key`, or an earlier `@unique` field) so cursors are stable; a
  cursor is signed with its sort, and replaying it under another sort is an
  `INVALID_CURSOR` error.
- Nulls sort last ascending and first descending, as in Cypher, and keyset
  pages handle them.
- Every list is bounded: `limit` / `first` default to `@limit(default:)`
  (global 25) and asking for more than `max` (global 100) is a
  `LIMIT_EXCEEDED` error, not a silent clamp.

## Mutations

Mutations exist only for types with `@mutation`, and address nodes by
`@key`, so every write-set is exact.

```graphql
mutation {
  createFestivals(
    input: [
      {
        name: "Sunland"
        genre: { create: { node: { key: "techno", name: "Techno" } } }
        followers: { connect: [{ key: "u1", edge: { since: 2020 } }] }
      }
    ]
  ) {
    festivals {
      key
      name
      genre {
        name
      }
    }
    info {
      nodesCreated
      relationshipsCreated
    }
  }
  updateFestival(
    key: "f1"
    update: {
      capacity: null # removes the property
      genre: { connect: { key: "house" } } # replaces the single relationship
      followers: { disconnect: ["u2"] }
    }
  ) {
    festival {
      key
    }
  }
  deleteFestival(key: "f2") {
    nodesDeleted
    relationshipsDeleted
  }
}
```

`upsertFestivals` creates the inputs whose key is new and updates the rest,
in one transaction; fields required on create are only required for new
keys. `info` reports `nodesCreated`, `nodesUpdated`, `nodesDeleted`,
`relationshipsCreated` and `relationshipsDeleted`.

Each mutation runs in one interactive transaction and checks, before it
commits:

- every `connect` target exists and is visible to the caller (`NOT_FOUND`);
- a single relationship stays single, even when written from the other
  side (`CONSTRAINT_VIOLATION`);
- `@authorization` validate rules (`FORBIDDEN`).

Any failure rolls the whole mutation back. Engine constraint errors come
back as `CONSTRAINT_VIOLATION` naming the type and field. A mutation creates
at most `maxBatch` nodes (default 1000). `@key` is not updatable.

## Search

```graphql
type Event @node @fulltext(indexes: [{ fields: ["title", "summary"] }]) {
  key: String! @key
  title: String!
  summary: String
  embedding: [Float!] @vector(dimensions: 384, similarity: COSINE)
}
```

```graphql
{
  searchEvents(query: "techno sun*", where: { city: { eq: "Ams" } }) {
    score
    node {
      key
      title
    }
  }
  similarEvents(to: "e1", limit: 5) {
    score
    node {
      key
    }
  }
}
```

Full-text queries AND their terms, fold case and accents, and treat a
trailing `*` as a prefix. Vector search takes a query `vector` or the key of
a node whose embedding to start from (`to`, which is left out of the
results). The index returns its top candidates before `where` applies, so
the library asks it for four times the page when a filter is present.
`@vector` fields are stored as VECTOR values, which the index requires, and
read back as `[Float!]`. Both indexes are part of S1 and created by
`assertSchema({ create: true })`; read rules apply to every result.

## @cypher fields

```graphql
type Festival @node {
  key: ID! @key
  similar(limit: Int = 3): [Festival!]!
    @cypher(
      statement: """
      MATCH (this)-[:IN_GENRE]->(:Genre)<-[:IN_GENRE]-(other:Festival)
      WHERE other.key <> this.key
      RETURN other ORDER BY other.name LIMIT $limit
      """
      columnName: "other"
    )
}

type Query {
  festivalCount: Int!
    @cypher(statement: "MATCH (f:Festival) RETURN count(f) AS n")
}

type Mutation {
  renameGenre(key: String!, name: String!): Genre
    @cypher(
      statement: "MATCH (g:Genre) WHERE g.key = $key SET g.name = $name RETURN g"
    )
}
```

`this` is the parent node; arguments are `$parameters`; `$jwt` holds the
request's claims. `columnName` is inferred from `RETURN x` or `RETURN … AS x`.
Fields returning `@node` types are projected with the selection like any
other node, and read filters apply to them.

Statements are checked, not trusted:

- at startup: every `$parameter` must be an argument or `$jwt`, Query and
  object fields may not contain write clauses, and unused arguments,
  `OPTIONAL MATCH` and statements that never use `this` are warnings
  (`lora.model.warnings`);
- in `check()`: every statement is planned with `explain()`, so a syntax
  error, an unknown function or a missing column fails CI with the engine's
  message, not the first request.

## Authorization

The library does not verify tokens. Verify them in your server and put the
claims in the context as `jwt` (or pass `jwt: (context) => claims`).

```graphql
type Post
  @node
  @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE])
  @authorization(
    filter: [
      {
        where: { node: { published: { eq: true } } }
        requireAuthentication: false
      }
      { where: { node: { author: { key: { eq: "$jwt.sub" } } } } }
      { where: { jwt: { roles: { includes: "admin" } } } }
    ]
    validate: [
      {
        operations: [CREATE, UPDATE]
        when: [AFTER]
        where: {
          OR: [
            { node: { author: { key: { eq: "$jwt.sub" } } } }
            { jwt: { roles: { includes: "admin" } } }
          ]
        }
      }
    ]
  ) {
  key: ID! @key(generate: true)
  title: String!
  published: Boolean! @default(value: false)
  notes: String @authentication(operations: [READ])
  author: User! @relationship(type: "WROTE", direction: IN)
}
```

- A rule is `{ node, jwt, AND, OR, NOT }`. `node` is a filter over the type,
  where `"$jwt.path"` strings become the caller's claims. `jwt` tests claims
  (`eq`, `in`, `includes`, `contains`, `startsWith`, `endsWith`, `lt`, `lte`,
  `gt`, `gte`, `exists`).
- **Claim tests run in JavaScript at compile time**, so an admin's
  statement carries no filter at all, and a rule that needs a claim the
  request lacks simply denies. Statements stay specialised and
  index-friendly.
- `filter` rules (any passing rule grants) make other nodes invisible:
  in lists, lookups, counts, nested relationships, relationship filters,
  aggregates, and as targets of updates, deletes and connects.
- `validate` rules fail the request with `FORBIDDEN`: `BEFORE` an update or
  delete, `AFTER` a create or update (rolling it back), and for `READ`: on
  any returned node, and on cursors, counts and aggregates that cover one.
  They do not hide nodes from filters; use `filter` rules for that.
- Field-level `@authentication(operations: [READ])` also guards filtering,
  sorting and aggregating on the field.
- Rules are checked against the model at startup: an unknown field or
  operator, or a test that is empty or null, is an error, not an open door.
  A rule that needs a claim the request lacks denies as a whole, even
  under `NOT`.

## The smart layer

**S1: indexes from the API.** `requirements()` derives every constraint and
index the API needs, with the reason for each:

| API declares                           | Needs                                   |
| -------------------------------------- | --------------------------------------- |
| `@key`                                 | node key constraint                     |
| `@unique`                              | uniqueness constraint                   |
| non-null `@sortable` field             | existence constraint                    |
| `EQ`, `IN`                             | nothing: LoraDB indexes equality lazily |
| `LT`, `LTE`, `GT`, `GTE`, `@sortable`  | RANGE index                             |
| `CONTAINS`, `STARTS_WITH`, `ENDS_WITH` | TEXT index                              |
| `WITHIN_BBOX`, `DISTANCE`              | POINT index                             |

`assertSchema()` reports what the database lacks; `assertSchema({ create: true })`
creates it, idempotently.

**S2: plans are checked.** Every compiled statement records the access path
it was written for. `lora.explain(query, variables)` plans each statement and
reports a label scan where a seek was expected, a mutating plan behind a
read, or result columns that do not match. `lora.check({ operations })`
runs this over your operations; the CLI does it in CI.

**S3: compile once.** `lora.persist({ id: source })` parses and validates
persisted operations at startup, and `lora.execute({ id, variables, context })`
runs them with no parsing or validation. Ad hoc documents passed to
`execute({ source })` are cached too. Translation itself takes about 0.13 ms
for a nested page, and statement text depends only on the shape of the
input, so LoraDB's own plan cache is hit for every repeat.

**S5: read-sets and write-sets.** See [change tracking](#change-tracking).

**S6: statistics and cost.** A relationship filter that names a related node
by key starts from that node and expands, instead of scanning the label.
Every root field has a cost estimate (rows touched, multiplying page sizes
through nested lists, capped by `@cardinality`), and one over `maxCost`
(default 50 000) fails with `COST_EXCEEDED` before it runs. `lora.analyze()`
samples node counts and relationship degrees so estimates use the measured
p99 degree instead of the page size.

**S7: one SDL, two diffs.** `diffSchemas(before, after)` reports the
database statements a change needs (index and constraint changes, relabels,
property renames, destructive ones flagged) and the API's breaking and
dangerous changes. A field renamed with `@alias` over the same property is
reported as an API break with no data migration.

## Change tracking

```ts
lora.onWrite((change) => {
  cache.invalidate(
    change.entities.map((e) => ({ typename: e.type, id: e.key })),
  );
});

for await (const change of lora.changes({ signal })) publish(change);
```

A `WriteChange` lists the nodes `created`, `updated` and `deleted`, the
relationships `connected` and `disconnected` (by field and both keys),
`entities` (every node whose observable state changed, relationship ends
included), and the touched `types` and `relationshipTypes`. Every compiled
read carries a read-set (labels and relationship types); `lora.affects(reads, change)`
tells whether a cached read may be stale. `@cypher` mutations have no
known write-set and are reported with `broad: true`. Only writes made
through the library are seen.

## Subscriptions

```graphql
subscription {
  festivalChanged(key: "f1", operations: [UPDATE, DELETE]) {
    operation
    key
    node {
      name
      capacity
    }
  }
}
```

A type with `@subscription` gets `<type>Changed`, fed by the write-sets of
mutations made through the library (`@cypher` mutations have none, and
writes made elsewhere are not seen). A node that gained or lost a
relationship is an `UPDATE`. `node` is read when the event is delivered,
through the normal read path, so read rules apply: events for nodes the
subscriber cannot read are dropped, and deletions of rule-protected types
are sent only to subscribers following that key. Pass a `signal` in the
context to end the stream with the request.

## CLI

```sh
lora-graphql print schema.graphql              # the public SDL
lora-graphql requirements schema.graphql --ddl # constraints and indexes, as DDL
lora-graphql check schema.graphql --operations src/operations
lora-graphql diff old.graphql new.graphql      # exit 1 on breaking changes
lora-graphql directives                        # directive SDL for editors
```

`check` builds an in-memory LoraDB (needs `@loradb/lora-node`), asserts the
schema, plans every `@cypher` statement and every query in the operation
files (required variables get sample values), and exits non-zero on any
finding. It is the CI gate.

## Drivers, limits and errors

`loraDriver(db)` adapts a `Database` from `@loradb/lora-node` or
`@loradb/lora-wasm`. Reads stream; multi-statement reads run in one
read-only transaction; mutations need interactive transactions, which only
the Node binding has, so the WASM binding serves reads. `explain()`, and so
plan checks, need the Node binding too.

| Option                      | Default       |                                                  |
| --------------------------- | ------------- | ------------------------------------------------ |
| `timeoutMs`                 | 10 000        | Per statement; a `signal` in the context cancels |
| `maxCost`                   | 50 000        | Estimated rows per root field                    |
| `maxBatch`                  | 1000          | Nodes created per mutation                       |
| `defaultLimit` / `maxLimit` | 25 / 100      | Global page sizes; `@limit` may only lower `max` |
| `jwt`                       | `context.jwt` | Where the claims are                             |
| `onStatement`               |               | Observe every statement                          |

Errors carry `extensions.code`: `BAD_USER_INPUT`, `INVALID_CURSOR`,
`LIMIT_EXCEEDED`, `COST_EXCEEDED`, `UNAUTHENTICATED`, `FORBIDDEN`,
`NOT_FOUND`, `CONSTRAINT_VIOLATION` (with `type` and `field`) and
`DATABASE_ERROR`. An invalid SDL throws one `ModelError` listing every
problem, each located by type and field.

## Translation rules

Measured on LoraDB 0.15 over 20 000 festivals and 100 000 relationships
(`yarn bench`):

| Rule                                                                                         | Why                                                                                  |
| -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| Relationships as pattern comprehensions; counts and relationship filters as `size([… \| 1])` | No `OPTIONAL MATCH`; `EXISTS { }` does not parse                                     |
| `CALL { }` only for sorted nested lists, nested connections and `@cypher`                    | The only way to sort or aggregate per parent                                         |
| Ordered by an always-present string: `WHERE s >= ""`                                         | The planner walks the index in order and stops at the limit: 0.03 ms instead of 7 ms |
| `key IN $list` becomes `UNWIND` plus equality                                                | `IN` scans the label: 0.04 ms instead of 4.6 ms                                      |
| A relationship filter naming a key starts from that node                                     | 0.07 ms instead of 9.3 ms                                                            |
| Keyset predicates written out, led by `sortKey >= $v` on non-null keys                       | `[a, b] > $list` silently matches nothing; the lead bound gets a range scan          |
| Lists sort by the requested fields only; connections add a unique tie-breaker                | Two sort keys cannot stream from an index                                            |
| Every value a parameter; every identifier from the model, escaped                            | No injection, stable statement text                                                  |
| Absent filters left out, never `($p IS NULL OR …)`                                           | Keeps the predicate visible to the planner                                           |

## Coming from @neo4j/graphql

| @neo4j/graphql                              | lora-graphql                                           |
| ------------------------------------------- | ------------------------------------------------------ |
| Every type gets every operation             | Reads by default; mutations only with `@mutation`      |
| Every field filterable by every operator    | `@filterable(byValue:)`, checked against the type      |
| `name_CONTAINS` or `{ name: { contains } }` | `{ name: { contains } }`                               |
| Offset cursors (`arrayconnection:N`)        | Keyset cursors signed with their sort                  |
| Bulk `update`/`delete` by `where`           | By `@key`, so write-sets are exact                     |
| `@id`                                       | `@key(generate: true)`                                 |
| `@populatedBy`, `@timestamp`                | `@default`, `@timestamp`                               |
| `@authorization` rules evaluated in Cypher  | Claim checks folded in JavaScript; node rules compiled |
| You pick indexes                            | Inferred from the API, and verified with `explain()`   |
| Unbounded lists                             | Every list bounded; cost limit per field               |
| `@fulltext` over the Neo4j procedure        | `searchX` root fields, with the index created for you  |
| Subscriptions over CDC                      | `@subscription`, from library-made writes              |
| Interfaces, unions                          | Not yet                                                |

## LoraDB behaviours this works around

Found while building this package; each workaround goes away with its fix.

| Behaviour                                                                                                                  | Workaround                                                                              |
| -------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Reads under a deadline or in a transaction do not stop early at `LIMIT` (`execute.rs`: the pull path requires no deadline) | Single-statement reads use `stream()`, which checks the deadline between rows           |
| `MERGE (a)-[r:T]->(b)` and `[(a)-[:T]->(b) \| …]` ignore an already-bound `b`                                              | Connect deletes the pair's edge, then `CREATE`s                                         |
| Writes inside `CALL { }` fail as read-only                                                                                 | `@cypher` mutations run unwrapped                                                       |
| An aggregate nested in a call (`head(collect(x))`, `collect(x)[0..2]`) is not aggregated                                   | `WITH collect(x) AS c RETURN head(c)`                                                   |
| RANGE indexes skip temporal values but range predicates still use them, returning nothing                                  | No RANGE index is inferred for temporal fields; `@index(kind: RANGE)` on one is refused |
| Existence constraints are checked at `CREATE`, before `SET`                                                                | All properties go into the `CREATE` map                                                 |
| A `null` in a property map is stored                                                                                       | Absent properties are left out of the map                                               |
| `x IN $list` does not use an index; `[a, b] > $list` matches nothing; `first()` is unknown                                 | See translation rules                                                                   |

## Development

```sh
yarn test       # vitest: model, TCK snapshots, integration, auth, CLI
yarn bench      # latency on a seeded graph
yarn typecheck && yarn lint && yarn build
```
