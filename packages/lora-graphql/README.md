# @loradb/lora-graphql

Schema-first GraphQL for LoraDB. One annotated SDL describes the graph and
the public API. The library turns it into an executable `graphql-js` schema
whose operations compile to parameterised Cypher statements, and it reasons
about those statements: it derives the indexes they need, checks with
`explain()` that they use them, bounds their cost before they run, compiles
authorization into them, and reports exactly what every mutation wrote.

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
- [Interfaces and unions](#interfaces-and-unions)
- [Mutations](#mutations)
- [Search](#search)
- [@cypher fields](#cypher-fields)
- [Authorization](#authorization)
- [The smart layer](#the-smart-layer)
- [Change tracking](#change-tracking)
- [Subscriptions](#subscriptions)
- [Transactions](#transactions)
- [CLI](#cli)
- [Drivers, limits and errors](#drivers-limits-and-errors)
- [Translation rules](#translation-rules)
- [Coming from @neo4j/graphql](#coming-from-neo4jgraphql)
- [LoraDB behaviours this works around](#loradb-behaviours-this-works-around)

## What it generates

For each `@node` type (reads are on by default; `@query(read: false)` turns
them off):

| Field                                                          | Does                                                                                                      |
| -------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `festivals(where, sort, limit)`                                | A bounded list                                                                                            |
| `festivalsConnection(where, sort, first, after, last, before)` | Relay connection with keyset cursors in both directions, `totalCount` and `aggregate`; never `SKIP`       |
| `festival(key:)`                                               | Lookup by `@key`                                                                                          |
| `festivalsAggregate(where)`                                    | `count`, and `min` / `max` (`avg` / `sum` for numbers) of sortable fields, with `@query(aggregate: true)` |
| `searchFestivals(query, where, limit)`                         | Full-text search, with `@fulltext`                                                                        |
| `similarFestivals(vector or to, where, limit)`                 | Vector similarity, with a `@vector` field                                                                 |
| `node(id:)`                                                    | Any `@relayId` type by global id                                                                          |
| `events(where, sort, limit)`                                   | An interface's or union's members together                                                                |
| `createFestivals`, `upsertFestivals`                           | With `@mutation(CREATE)` (upsert also needs `UPDATE`)                                                     |
| `updateFestival`, `updateFestivals(where, limit)`              | With `@mutation(UPDATE)`: by key, or bulk by `where`                                                      |
| `deleteFestival`, `deleteFestivals(where, limit)`              | With `@mutation(DELETE)`: by key, or bulk by `where`                                                      |
| `festivalChanged(key, operations, where)`                      | A subscription, with `@subscription`                                                                      |

Relationship fields take `where`, `sort` and `limit`; list relationships
also get `…Connection`, whose edges carry the relationship properties and
filter on them (`where: { node, edge }`).

The surface is restrictive: a field is filterable only with `@filterable`,
and only by the operators listed; sortable only with `@sortable`;
`printPublicSchema()` prints exactly what clients see, with no directives.

## Directives

A directive applies where the tables below say, and nowhere else: one in
a position the model would not apply (an `@authorization` on an interface
field, a `@selectable` on a field of an object type without `@node`, a
`@limit` on a scalar field) is a model error naming the directive and the
position, never silently ignored. `DIRECTIVE_POSITIONS` in
`src/model/positions.ts` is the full table.

Model:

| Directive                                                                                                  | On                | Meaning                                                                                                                                                                                                                                    |
| ---------------------------------------------------------------------------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `@node(labels:, plural:)`                                                                                  | type              | A node label set; default: the type name                                                                                                                                                                                                   |
| `@key(generate:)`                                                                                          | field             | Required, unique and immutable. The sort tie-breaker, cursor anchor and mutation address. `generate: true` fills a UUID on create                                                                                                          |
| `@unique`                                                                                                  | field             | Uniqueness constraint                                                                                                                                                                                                                      |
| `@uniqueTogether(fields:, where:)`                                                                         | type              | No two nodes (matching `where`) share these fields: scalars, single relationships, at most one list relationship as a set. Repeatable; see [Mutations](#mutations)                                                                         |
| `@index(kind: RANGE \| TEXT \| POINT)`                                                                     | field             | An explicit index, usually inferred                                                                                                                                                                                                        |
| `@storedAs(type:)`                                                                                         | custom scalar     | How a custom scalar is stored (`STRING`, `INT`, `FLOAT`, `BOOLEAN`, `DATETIME`, `DATE`); its SDL description is what clients see, whatever implementation `scalars` passes                                                                 |
| `@relationship(type:, direction:, properties:, queryDirection:, onDelete:, nestedOperations:, aggregate:)` | field             | An edge to a `@node` type, interface or union. `queryDirection: UNDIRECTED` reads both ways; `onDelete: DETACH \| CASCADE \| RESTRICT`; `nestedOperations` lists the nested writes inputs offer; `aggregate: false` drops its aggregates   |
| `@declareRelationship`                                                                                     | interface field   | Every implementation declares this relationship (type and direction may differ); select it on the interface                                                                                                                                |
| `@relationshipProperties`                                                                                  | type              | Properties on a relationship type                                                                                                                                                                                                          |
| `@alias(property:)`                                                                                        | field             | API name differs from the stored property                                                                                                                                                                                                  |
| `@private`                                                                                                 | field             | Stored, never exposed                                                                                                                                                                                                                      |
| `@readonly`                                                                                                | field             | Exposed, never client-settable; on a relationship, absent from create and update inputs                                                                                                                                                    |
| `@settable(onCreate:, onUpdate:)`                                                                          | field             | Which mutations may set it, e.g. set once on create; on a relationship, whether the inputs offer it (an upsert of an existing node keeps it); on a relationship property, `onUpdate: false` also refuses a re-connect that would change it |
| `@selectable(onRead:, onAggregate:)`                                                                       | field             | `onRead: false` makes a field write-only; on a relationship property, it leaves the edge type too (`onAggregate: false`, the edge aggregates); with every property hidden, the edge has no `properties`                                    |
| `@default(value:)`                                                                                         | field             | Stored on create when the input omits it; on a relationship property, when the relationship is created                                                                                                                                     |
| `@timestamp(operations: [CREATE, UPDATE])`                                                                 | field             | Set to the current time; never client-settable. On a relationship property: CREATE when the relationship is created, UPDATE on edge updates and re-connects that set properties                                                            |
| `@populatedBy(callback:, operations:)`                                                                     | field             | Computed by a named callback on write                                                                                                                                                                                                      |
| `@cardinality(max:)`                                                                                       | list relationship | Declared fan-out, for cost estimates                                                                                                                                                                                                       |
| `@cypher(statement:, columnName:)`                                                                         | field             | A field backed by a Cypher statement. Returns scalars, `@node` types, interfaces or unions over them, or object types without `@node` (read from a map)                                                                                    |
| `@fulltext(indexes: [{ name, fields, analyzer, queryName }])`                                              | type              | FULLTEXT indexes, each with a search root field                                                                                                                                                                                            |
| `@vector(dimensions:, similarity:, queryName:)`                                                            | `[Float!]` field  | A VECTOR index and a similarity root field                                                                                                                                                                                                 |
| `@plural(value:)`                                                                                          | interface, union  | The root field's name                                                                                                                                                                                                                      |

API:

| Directive                                             | On                                           | Meaning                                                                                                       |
| ----------------------------------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `@query(read:, aggregate:)`                           | type, interface, union                       | Generated reads                                                                                               |
| `@mutation(operations: [CREATE, UPDATE, DELETE])`     | type                                         | Generated mutations; none without it                                                                          |
| `@subscription(operations: [CREATE, UPDATE, DELETE])` | type                                         | Generated subscriptions; none without it                                                                      |
| `@filterable(byValue: [...])`                         | field                                        | Filter operators. Bare: `EQ` and `IN` (lists: `INCLUDES`). On a relationship: enables relationship filters    |
| `@sortable`                                           | field                                        | Sort and paginate by this field (on a relationship property: `sort: [{ edge: { ... } }]`)                     |
| `@groupBy`                                            | field                                        | A grouping key of `<plural>Grouped(by:)` (needs `@query(aggregate: true)`)                                    |
| `@limit(default:, max:)`                              | type, interface, union, list relationship    | Page size bounds                                                                                              |
| `@size(max:)`                                         | list argument of a `@cypher` field           | The most items it takes (default `maxListArgument`)                                                           |
| `@relayId`                                            | `@key` field                                 | Adds a global `id` and the `Node` interface                                                                   |
| `@authentication(operations:, jwt:)`                  | type, field                                  | Needs an authenticated request, whose claims satisfy `jwt`                                                    |
| `@authorization(filter:, validate:)`                  | type (filter and validate), field (validate) | Row-level rules, compiled into statements                                                                     |
| `@jwt`, `@jwtClaim(path:)`                            | type, field                                  | The claims shape; rules may only use declared claims                                                          |
| `@viewer(type:, field:)`                              | `@jwt` claim                                 | The claim naming the caller's node (by a `@key` or `@unique` field): enables `isViewer` and `viewer` in rules |

`directiveTypeDefs` (or `lora-graphql directives`) prints these as SDL for
editors and codegen.

Filter operators: `EQ`, `IN`, `LT`, `LTE`, `GT`, `GTE`, `CONTAINS`,
`STARTS_WITH`, `ENDS_WITH`, `CASE_INSENSITIVE` (strings), `IS_NULL`
(nullable fields), `INCLUDES` (lists), and on points `WITHIN_BBOX` and
`DISTANCE`. Each is checked against the field's type.

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
      name: { caseInsensitive: { contains: "land" } }
      followers: { some: { key: { eq: "u1" } } }
      followersConnection: { some: { edge: { since: { gte: 2020 } } } }
      OR: [{ capacity: { gte: 5000 } }, { genre: { name: { eq: "Techno" } } }]
    }
    sort: [{ capacity: DESC }]
  ) {
    totalCount
    aggregate {
      count
      node {
        capacity {
          max
          avg
        }
      }
    }
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
  `all`, `none`, `single`, `count` and `aggregate` (`node` / `edge` field
  `min`, `max`, `avg`, `sum`); single relationships take the target's filter
  directly; `<field>Connection` quantifies over node and relationship
  properties together.
- **Absent and `null` filters are left out of the statement**, so a filter
  bound to an unset variable costs nothing. `eq: null` does not mean
  `IS NULL`: use `isNull: true`. An `OR` branch left empty that way is left
  out of the `OR`, and an `OR` or `NOT` with nothing left is left out
  entirely, so an unset variable never widens a filter to every row; a
  literal `OR: []` matches nothing. A relationship quantifier whose filter
  is empty is left out too: ask `count: { gt: 0 }` for "has any". A single
  relationship takes `<field>Exists: Boolean` for "is set" (`venueExists:
false`: festivals without a venue); as in every relationship filter, a
  related node the reader may not see counts as none.
- `count` and `single` count related nodes once each, however many
  relationships lead to them. `all` holds on an empty set, and a missing
  property fails it.
- `CASE_INSENSITIVE` compares lowercased values and cannot use an index:
  prefer `@fulltext` for search over large labels.
- Connections page forward with `first` / `after` and backward with
  `last` / `before`; one cursor works in both directions. `aggregate`
  covers every match, not only the page; selecting only `totalCount` or
  `aggregate` reads no page.
- `sort` takes one field per item. Lists sort by the requested fields only;
  connections always end on a unique, non-null field (the `@key`, or an
  earlier required `@unique` field) so cursors are stable. A cursor is tagged
  with its sort, and replaying it under another sort is an `INVALID_CURSOR`
  error. With `cursorSecret` it is also signed (HMAC-SHA-256), and any
  cursor the server did not issue is rejected. Nested lists without a sort come in `@key` order.
- Nulls sort last ascending and first descending, as in Cypher, and keyset
  pages handle them.
- Every list is bounded: `limit` / `first` default to `@limit(default:)`
  (global 25) and asking for more than `max` (global 100) is a
  `LIMIT_EXCEEDED` error, not a silent clamp.

### More query surface

- **Edge sort.** A relationship connection sorts by `@sortable`
  relationship properties: `followsConnection(sort: [{ edge: { since: DESC } }, { name: ASC }])`.
  Cursors carry the edge value; relationship properties have no index, so
  this sorts per parent, bounded by the page.
- **Grouped aggregates.** `sessionsGrouped(by: [kind, room], where:, limit:)`
  returns `[{ by { kind room } aggregate { count minutes { sum } } }]`,
  ordered by the group values, at most `limit` groups.
- **Richer aggregates.** Strings add `shortest` / `longest`, and aggregate
  filters take `shortestLength`, `longestLength` and `averageLength`.
  Durations aggregate `min`, `max`, `sum` and `avg`. Relationship
  connections count `count { nodes edges }`: they differ when several
  relationships lead to the same node.

## Interfaces and unions

```graphql
interface Event @limit(default: 10) {
  key: String!
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  starts: Int @sortable
}
type Concert implements Event @node @mutation {
  key: String! @key
  title: String!
  starts: Int
  band: String
}
type Exhibition implements Event @node @mutation {
  key: String! @key
  title: String!
  starts: Int
  artist: String
}
union Headline = Concert | Exhibition
type Venue @node @mutation {
  key: String! @key
  events: [Event!]! @relationship(type: "HOSTS", direction: OUT) @filterable
  headline: Headline @relationship(type: "HEADLINES", direction: OUT)
}
```

```graphql
{
  events(
    where: { title: { contains: "Rock" }, typename: [Concert] }
    sort: [{ starts: ASC }]
  ) {
    __typename
    key
    title
    ... on Concert {
      band
    }
  }
  headlines(where: { Exhibition: { title: { eq: "Modern art" } } }) {
    __typename
  }
  venue(key: "v1") {
    events(limit: 5) {
      key
    }
  }
}
```

Interfaces and unions range over `@node` types. An interface's
`@filterable` and `@sortable` fields apply to every implementation, and so
does index inference: each implementation's label gets the index. A root
list or relationship field over an interface runs one sorted, limited
subquery per implementation, each able to use its own index, and merges
them by the requested sort, then type name, then key. An interface `where`
takes the interface's fields plus `typename`; a union `where` takes one
filter per member, and once any member is named, members not named are left
out. Mutations connect, create and disconnect per member:
`events: { connect: { Concert: [{ key: "c1" }] } }`. A single relationship
to a union holds one node across all members.

### Interface relationships

An interface field under `@declareRelationship` is a relationship every
implementation declares (with the same target and shape; the type and
direction may differ), so `events { venue { name } }` works at interface
level.

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
      followers: {
        disconnect: ["u2"]
        update: [{ key: "u1", edge: { since: 2021 } }] # in place
      }
    }
    adjust: { visits: { add: 1 }, tags: { push: ["summer"] } }
  ) {
    festival {
      key
    }
  }
  updateFestivals(
    where: { capacity: { lt: 100 } }
    adjust: { capacity: { multiply: 2 } }
    limit: 50
  ) {
    info {
      nodesUpdated
    }
  }
  deleteFestival(key: "f2") {
    nodesDeleted
    relationshipsDeleted
  }
}
```

- **Updates** set fields (`null` removes one) and relationships (`connect`,
  `create`, `disconnect`, and `update` of connected nodes and relationship
  properties in place). Connecting an already-connected pair keeps one
  relationship and sets the properties the input gives; the others,
  `@default`s included, keep their values. Defaults apply when a
  relationship is created. Re-connecting a single relationship to its
  current target keeps that relationship.
- **`adjust`** applies math (`add`, `subtract`, `multiply`, `divide`) and
  list (`push`, `pop`, `remove`) operators to the stored value atomically;
  a missing number counts as 0 and a missing list as empty.
- **Bulk `updateFestivals` / `deleteFestivals`** resolve the keys `where`
  matches under the authorization filter, then run the keyed path, so their
  write-sets are exact too. More matches than `limit` (default `maxBatch`)
  is an error that writes nothing, and an empty `where` is refused.
- **`upsertFestivals`** creates the inputs whose key is new and updates the
  rest; fields required on create are required only for new keys. A key the
  caller may not see is never updated, and never reported as taken: it gets
  the answer a free key would (see below).
- **Creating under a hidden key** (`createFestivals`, `upsertFestivals`, a
  nested `create`) answers as if the key were free: whatever error the free
  key would get, and where it would be created, `FORBIDDEN` ("not allowed to
  create"), never a `CONSTRAINT_VIOLATION` that would confirm the key
  exists. A key held by a node the caller can read is still reported as
  taken. See `docs/design/graphql-threat-model.md`.
- **Deletes** follow `onDelete`: `DETACH` (default) removes the
  relationships, `CASCADE` deletes what the field reaches (checking the
  caller may delete each node), `RESTRICT` refuses while related nodes
  remain.
- **`@populatedBy(callback: "slug")`** computes a field with
  `callbacks: { slug: ({ input, key, context, operation }) => … }`.

Each mutation runs in one interactive transaction and checks, before it
commits:

- every `connect` target exists and is visible to the caller (`NOT_FOUND`);
- a single relationship stays single and a required one stays set, even
  when written from the other side or when its target is deleted
  (`CONSTRAINT_VIOLATION`);
- every `@uniqueTogether` holds (`CONSTRAINT_VIOLATION`, below);
- `@authorization` validate rules, type and field level (`FORBIDDEN`).

`@uniqueTogether` states a uniqueness the engine's per-property constraints
cannot: over relationship ends.

```graphql
type ConnectionRequest @node @uniqueTogether(fields: ["from", "to"]) {
  key: ID! @key(generate: true)
  from: Person! @relationship(type: "SENT", direction: IN)
  to: Person! @relationship(type: "TO", direction: OUT)
}
type Conversation
  @node
  @uniqueTogether(fields: ["participants"], where: { kind: { eq: DIRECT } }) {
  key: ID! @key
  kind: ConversationKind!
  participants: [Person!]! @relationship(type: "IN", direction: IN)
}
```

`fields` names scalar fields (compared by value), single relationships (by
the target's `@key`) and at most one list relationship (by the set of
target keys, in any order); `where` limits the nodes compared. A
combination with a null scalar, no target or an empty set is exempt, as a
null is in a unique index. Every generated mutation checks it after its
writes — creates, updates, upserts, nested creates, and connects,
disconnects and deletes from either side — for every caller, the bypass
included: it is a data invariant, not a rule. Each check seeks the written
nodes and compares only with nodes sharing their first relationship end
(or, with scalars only, their first scalar's value). Writes outside
generated mutations (`@cypher` mutations, Cypher of your own) are not
checked.

Any failure rolls the whole mutation back. Atomicity is per root field:
in an operation with several root fields, each runs in its own
transaction, so a later failure leaves the earlier ones committed. Pass
`mutationTransaction: "operation"` to run every root field of a mutation
in one transaction through `execute()` (persisted operations included):
it commits only when the operation reports no error, and otherwise rolls
back and returns `data: null`. With another server, put a `lora.begin()`
transaction in the context (see [Transactions](#transactions)). Engine
constraint errors come
back as `CONSTRAINT_VIOLATION` naming the type and field. A mutation creates
or deletes at most `maxBatch` nodes (default 1000). `@key` is not updatable.
`info` reports `nodesCreated`, `nodesUpdated`, `nodesDeleted`,
`relationshipsCreated` and `relationshipsDeleted`.

### Nested delete and trimmed inputs

`update: { stages: { delete: { where: { size: { gt: 2 } }, limit: 10 } } }`
deletes connected nodes (a single relationship takes `delete: true`),
bounded like bulk deletes and following `onDelete`. `limit` defaults to
`maxBatch`; more matches is `LIMIT_EXCEEDED`.
`@relationship(nestedOperations: [CONNECT])` keeps only the listed nested
writes in the inputs, and `aggregate: false` removes the relationship's
aggregates and aggregate filter. A nested `update: [{ key, edge, node }]`
(`UPDATE`) changes the connected node and the relationship's properties;
`UPDATE_EDGE` offers `update: [{ key, edge }]` alone, so an input can keep
"set my RSVP" without advertising "edit the festival" (it needs
relationship properties).

An input left with no field is left out, with what would take it: a
relationship without settable properties has no `edge` input, and a type
with nothing settable on update (no settable field, no relationship with
a nested write) has no update mutations; the model warns when
`@mutation(operations: [UPDATE])` asked for them. A generated schema
graphql-js would reject is a `ModelError` from `getSchema()`, never an
error on every request.

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
trailing `*` as a prefix. A `[String!]` field in `@fulltext` indexes each
of its strings. Search connections (`searchEventsConnection`) page with
cursors and take `totalCount`: every match after `where` and the read
rules (a vector search counts within its candidate window); a selection of
only `totalCount` reads no page. Vector search takes a query `vector` or the key of
a node whose embedding to start from (`to`, which is left out of the
results). The index returns its top candidates before `where` applies, so
the library asks it for four times the page when a filter is present.
`@vector` fields are stored as VECTOR values, which the index requires, and
read back as `[Float!]`. Both indexes are part of S1 and created by
`assertSchema({ create: true })`; read rules apply to every result.

Every search also has a connection: `searchDocsConnection(query:, where:,
first:, after:)` pages by keyset on (score, key). A vector index returns
its top candidates before any filter, so vector connections page within
`4 × @limit(max:)` candidates.

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
request's claims; `$viewer` is the caller (below). `columnName` is
inferred when the statement's last top-level `RETURN` has one item,
`RETURN x` or `RETURN … AS x` (commas inside calls, lists, maps and
`CALL { }` do not count); with several items, set it.
Fields returning `@node` types are projected with the selection like any
other node, and read filters apply to them.

Statements are checked, not trusted:

- at startup: every `$parameter` must be an argument, `$jwt` or
  `$viewer`, Query and object fields may not contain write clauses, and
  unused arguments, `OPTIONAL MATCH` and statements that never use
  `this` are warnings (`lora.model.warnings`);
- in `check()`: every statement is planned with `explain()`, so a syntax
  error, an unknown function or a missing column fails CI with the engine's
  message, not the first request.

### The caller: `$viewer`

With a `@viewer` claim, `$viewer` is the caller's node's `@key`, in field
and mutation statements alike, so a statement names the viewer, not the
claim:

```graphql
type Mutation {
  createPost(key: String!, caption: String!): Post
    @authentication
    @cypher(
      statement: """
      MATCH (a:Person) WHERE a.key = $viewer
      CREATE (a)-[:POSTED]->(p:Post {key: $key, caption: $caption})
      RETURN p
      """
    )
}
```

When `@viewer` maps to the key, `$viewer` is the claim itself. When it
maps to another field (an opaque `subject`), it is read in the statement
with one seek by the claim (`head([(v:Person {subject: $claim}) |
v.key])`), so the statement above keeps working, and keeps seeking, when
`@viewer` moves from `Person.key` to `Person.subject`. `$viewer` is null
signed out, for a claim that is not a string or number, and for a token
naming no node. A statement using it without a `@viewer` claim is a
model error, and `viewer` is reserved as an argument name, like `jwt`.

### Guarding root fields

`@authentication` and `@authorization(validate:)` guard a Query or
Mutation `@cypher` field before its statement runs. With no node, a rule
tests claims (`jwt`) and the caller's own node (`viewer`); `node` and
filter rules are model errors, and `operations` / `when` do not matter:
each rule guards the call.

```graphql
type Mutation {
  verify: Person
    @authentication
    @authorization(
      validate: [{ where: { viewer: { verified: { eq: true } } } }]
    )
    @cypher(
      statement: "MATCH (p:Person) WHERE p.key = $viewer SET p.verified = true RETURN p"
    )
}
```

As anywhere, any passing rule grants, and the schema's bypass skips them.
Claim tests are decided in JavaScript: a refusal (`FORBIDDEN`, or
`UNAUTHENTICATED` without a token) runs no statement. A `viewer` test is
one seek, in the mutation's transaction, before the statement. Every root
`@cypher` field is in the access matrix, guarded or not (Mutation fields
under the operation `EXECUTE`).

### List arguments

The statement sees its arguments as sent, so list arguments are capped:
at most `@size(max:)` items, or `maxListArgument` (default 1000) without
it. More is `BAD_USER_INPUT` before any statement runs; each level of a
nested list counts.

```graphql
createPost(key: String!, hashtags: [String!] = [] @size(max: 30)): Post
```

### Filters, sorts and richer results

A scalar `@cypher` field of a `@node` type may take `@filterable` and
`@sortable`: the statement then runs per node in a `CALL` before the
filter, in root fields only (through a relationship it is refused). No
index applies, and the model warns so `check` reports it.

A `@cypher` field may return an interface or union over `@node` types
(each node is projected as the member its label says) or an object type
without `@node`, whose fields are read from the returned map:
`RETURN { events: count(e), titles: collect(e.title) } AS s`.

## Authorization

The library does not verify tokens. Verify them in your server and put the
claims in the context as `jwt` (or pass `jwt: (context) => claims`).

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!] @jwtClaim(path: "app_metadata.roles")
}

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
      { where: { node: { tenant: { eq: "$context.tenant" } } } }
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
  tenant: String!
  published: Boolean! @default(value: false)
  notes: String @authentication(operations: [READ])
  royalties: Int
    @authorization(
      validate: [
        {
          operations: [READ]
          where: { node: { author: { key: { eq: "$jwt.sub" } } } }
        }
      ]
    )
  author: User! @relationship(type: "WROTE", direction: IN)
}
```

- A rule is `{ node, jwt, AND, OR, NOT }`. `node` is a filter over the type
  (relationship properties included, through `<field>Connection: { some:
{ node, edge } }`), where `"$jwt.path"` strings become the caller's claims and
  `"$context.path"` strings values from the GraphQL context. `jwt` tests
  claims (`eq`, `in`, `includes`, `contains`, `startsWith`, `endsWith`,
  `lt`, `lte`, `gt`, `gte`, `exists`).
- **The caller's own node.** Mark the claim that identifies the caller
  with `@viewer(type: "Person", field: "subject")` (the field is `@key` or
  `@unique`), so the key can stay a slug while the subject is an identity
  provider's opaque id. Rules then say `{ node: { isViewer: true } }` on
  the viewer type, or `{ node: { author: { isViewer: true } } }` through a
  relationship; this expands to `{ subject: { eq: "$jwt.sub" } }` at
  startup and compiles to exactly the same statement. `viewer: { verified:
{ eq: true } }` tests the caller's own node: one seek by the claim. Both
  are unknown without the claim, so `NOT { isViewer: true }` never grants a
  signed-out caller. `isViewer` takes `true` only; use `NOT` for the
  opposite. It works through relationships and union members
  (`author: { Person: { isViewer: true } }`); in a filter over an
  interface it is a model error, since there is no single type to expand
  against. In rule strings, `"${viewer.key}"` (any scalar field of the
  viewer type) is the caller's own value: the claim itself for the field
  `@viewer` maps to, otherwise read with one seek by the claim. Use it
  for keys built from the caller's key while the claim is an opaque
  subject: `key: { endsWith: ":${viewer.key}" }`, `key: { eq:
  "${viewer.key}" }`. It stands for one value, not inside a list.
- **The rule's own node.** `"${node.path}"` in a node part reads the node
  the rule is about, so a rule can relate two of its paths: "the request's
  recipient is in the conversation it gates" is
  `{ node: { conversation: { participants: { some: { key: { eq:
"${node.to.key}" } } } } } }`. The path ends on a scalar field and steps
  only through single relationships; the value is read in the statement
  (the stored one, as rules see it) and stands for one value, not inside a
  list. In a relationship field's rules, `${source.path}`, `${target.path}`
  and `${edge.property}` read its ends and properties: "the payer is on the
  expense's trip" is `{ source: { trip: { members: { some: { key: { eq:
"${target.key}" } } } } } }` on `Expense.paidBy`'s CONNECT. A named rule
  that reads `${node.…}` can't stand inside another node's filter, where it
  would read the outer node.
- A whole string starting with `$` must be a placeholder (`$jwt.<claim>`,
  `$context.<path>`): a misspelt one (`"$jtw.sub"`) is a model error, not
  a literal. Write a literal `$…` as `"\\$…"`.
- Inside a longer string, write `${jwt.path}` or `${context.path}`:
  `key: { startsWith: "${jwt.sub}:" }` confines a user to keys that begin
  with their `sub` and `:`. The claim must be a string, number or boolean;
  otherwise the rule denies. Pick a separator no `sub` contains: with `-`,
  user `a` could take `a-b-…`, the key space of user `a-b`. See
  [docs/design/graphql-threat-model.md](../../docs/design/graphql-threat-model.md).
- **Claim tests run in JavaScript at compile time**, so an admin's
  statement carries no filter at all. Statements stay specialised and
  index-friendly. A write whose rules the claims alone refuse is refused
  before any statement runs.
- A create under CREATE rules answers the same whether a `@unique` value
  (or the key) is taken by a node the caller may not create: what a free
  value gets. A create that would succeed answers `CONSTRAINT_VIOLATION`.
- **A test that needs a claim or context value the request lacks is
  unknown**: false where it stands, and a `NOT` over it is false too, so
  negation can never turn a missing claim into a grant. Node conditions
  follow Cypher: `NOT` over a null property is not true.
- `filter` rules (any passing rule grants) make other nodes invisible:
  in lists, lookups, counts, aggregates, search, nested relationships,
  relationship filters, subscriptions, and as targets of updates, deletes
  and connects. A node the same mutation creates is not hidden from its
  own connects, so a filter that depends on the new relationship (a
  request visible to its sender) does not block a nested create. Filter rules for `CREATE_RELATIONSHIP` and
  `DELETE_RELATIONSHIP` guard both ends of connects and disconnects.
- `validate` rules fail the request with `FORBIDDEN`: `BEFORE` an update or
  delete, `AFTER` a create or update (rolling it back), and for `READ`: on
  any returned node, and on cursors, counts and aggregates that cover one.
  They do not hide nodes from filters; use `filter` rules for that.
- Field-level `@authorization(validate:)` guards one field: reading it on a
  row that fails is `FORBIDDEN`, filtering by it only matches rows that
  pass, sorting or aggregating by it is refused, and writing it checks the
  rule. A write is what the input sets: a create that leaves the field out
  is not checked against it, even when `@default` or `@populatedBy` fills
  it, so a CREATE rule can keep a `verified: Boolean! @default(value: false)`
  settable by admins only while anyone creates the node. Field-level
  `@authentication` also guards filtering, sorting and aggregating on the
  field.
- Reading a guarded field is checked per row, token or not: without the
  token a rule needs, each row reads the field as `UNAUTHENTICATED`. The
  statement has the same shape either way, so `compile()`, `explain()`,
  `check()` and `expectSeeks` plan-check such operations without a token;
  pass `context` (per operation in `check({ operations })`) to compile
  them as a signed-in caller.
- `@authentication(operations:, jwt:)` covers `READ`, `CREATE`, `UPDATE`,
  `DELETE`, `CREATE_RELATIONSHIP`, `DELETE_RELATIONSHIP` and `SUBSCRIBE`,
  and may require claims. On a relationship field, `CREATE` / `UPDATE`
  cover setting it in a create or update input, `CREATE_RELATIONSHIP` a
  `connect` or nested `create` through it, and `DELETE_RELATIONSHIP` a
  `disconnect` or nested `delete`.
- Rules are checked against the model at startup: an unknown field,
  operator or (with `@jwt`) claim, or a test that is empty or null, is an
  error, not an open door.
- **Owner-scoped keys.** `key: String! @key(scope: VIEWER, separator: ":")`
  keeps created keys in the caller's key space: a create (nested creates
  and upsert-creates included) must use a key that starts with the
  `@viewer` claim and the separator, and is longer than that prefix. It
  is checked before any statement runs, so `FORBIDDEN` for `bob:x` reads
  the same whether `bob:x` exists or not. A claim containing the
  separator is refused, so user `a` cannot write into the key space of
  user `a:b`. It needs `@viewer`; the schema's bypass skips it. When
  `@viewer` maps to a non-key field (an opaque subject), the key space is
  the caller's node's `@key`, looked up once by the claim before anything
  is written; a token naming no node creates nothing. Keys shared by two
  owners (`f1:lou`) stay hand-written rules, with `${viewer.key}`.
- **Masks.** A field-level READ rule fails the row; a mask substitutes a
  value instead:

  ```graphql
  status: ConnectionRequestStatus!
    @authorization(
      mask: [
        {
          unless: {
            OR: [
              { node: { status: { in: [PENDING, ACCEPTED] } } }
              { node: { to: { isViewer: true } } }
            ]
          }
          value: PENDING
        }
      ]
    )
  lastSeenAt: DateTime
    @authorization(mask: [{ unless: { node: { isViewer: true } } }])
  ```

  A row failing `unless` reads the field as `value` (`null` when left
  out, which a non-null field refuses; `value` is type-checked). Filters
  compare the value the reader sees, so a mask never leaks through a
  filter, and such a filter cannot use the field's index. Rules see the
  stored value. Sorting, grouping and aggregating by a masked field are
  refused unless the claims settle the mask (the brief proposed sorting by
  the masked value; refusing keeps order from hinting at hidden values).
  Masks sit on scalar fields of `@node` types other than the `@key`.

- **Named rules.** Define a rule once and use it as `{ rule: "name" }`
  wherever a rule part may stand:

  ```graphql
  extend schema
    @authorizationRules(
      rules: [{ name: "admin", where: { jwt: { roles: { includes: "admin" } } } }]
    )

  type Trip
    @node
    @authorizationRule(
      name: "member"
      where: {
        OR: [
          { node: { members: { some: { isViewer: true } } } }
          { node: { owner: { isViewer: true } } }
        ]
      }
    )
    @authorization(
      filter: [{ where: { OR: [{ rule: "member" }, { rule: "admin" }] } }]
    ) { ... }

  type PackingItem
    @node
    @authorization(
      filter: [
        {
          where: {
            OR: [{ node: { trip: { rule: "member" } } }, { rule: "admin" }]
          }
        }
      ]
    ) { ... }
  ```

  Schema rules (`@authorizationRules`) test claims only. A type's rules
  (`@authorizationRule`, repeatable) are found first, then the schema's;
  inside a node filter (`trip: { rule: "member" }`) the name is a rule of
  that node's type, and that rule must test `node` only. Rules are inlined
  at startup, so they compile to exactly the hand-written statement. An
  unknown name, a type rule shadowing a schema rule, and a cycle (named
  with its chain) are model errors. `bypass` and `mutations` in
  `@authorizationDefaults` may name rules too.

- **Schema-wide defaults.**

  ```graphql
  extend schema
    @authorizationDefaults(
      bypass: { jwt: { roles: { includes: "admin" } } }
      mutations: { jwt: { roles: { includes: "editor" } } }
    )
  ```

  A request passing `bypass` skips every filter and validate rule, field
  and relationship rules included; `@authentication` still applies.
  `bypass` tests claims only, so it is decided before the statement is
  built: an admin's statement carries no rule predicate. A type keeps its
  rules for everyone with `@authorization(bypass: false)`;
  `@authorization(bypass: true)` is the default made explicit, and quiets
  `check()`'s note that the bypass skips the type's field rules. `mutations` is
  the write rule (`CREATE`, `UPDATE`, `DELETE`) of every `@mutation` type
  that declares no rule for those operations; a type's own rules replace
  it, never merge with it. `check()` fails on a `@mutation` type whose
  writes nothing guards, unless it says `@authorization(public: [...])`.

Relationship and `@cypher` fields take field-level `@authorization`
with READ validate rules: a row failing the rule reads the field as
`FORBIDDEN`, and filtering through the field applies the rule too.

Relationship properties take field-level `@authentication` and
`@authorization(validate:)` for `READ`, `CREATE` and `UPDATE`. One
`@relationshipProperties` type can serve fields on both ends, so these
rules test claims (`jwt`) only; a `node` part is a model error. Setting
the property on connect or nested create checks `CREATE` for a new
relationship and `UPDATE` for one that already exists; `update: { edge }`
checks `UPDATE`. A request the READ rules refuse reads the property as
`FORBIDDEN` and cannot filter, sort or aggregate by it.

### Rules on relationships

A relationship field also takes validate rules for the relationship's
own operations, where both ends are known:

```graphql
type Trip @node @mutation {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]!
    @relationship(type: "MEMBER", direction: IN, properties: "TripInvite")
    @authorization(
      validate: [
        # the owner invites
        {
          operations: [CONNECT]
          where: { source: { owner: { isViewer: true } } }
        }
        # the owner removes anyone; a member removes only themselves
        {
          operations: [DISCONNECT]
          where: {
            OR: [
              { source: { owner: { isViewer: true } } }
              { target: { isViewer: true } }
            ]
          }
        }
        # only the member answers their own invitation, and reads its marker
        {
          operations: [UPDATE_EDGE, READ_EDGE]
          where: { target: { isViewer: true } }
        }
      ]
    )
}
```

- `source` is the node declaring the field, `target` the related node and
  `edge` the relationship's properties; `jwt`, `viewer`, `AND`, `OR` and
  `NOT` work as in any rule. A `node` part is a model error here, and so
  is a relationship operation on a type or scalar field, or in the same
  rule as `READ`.
- `CONNECT` covers a new relationship (connect, nested create);
  `DISCONNECT` a disconnect, including replacing a single relationship;
  `UPDATE_EDGE` `update: [{ edge }]` and a re-connect that sets
  properties; `READ_EDGE` reading the properties. `CONNECT` and
  `UPDATE_EDGE` are checked on the relationship after the write and
  `DISCONNECT` before it, in the mutation's transaction: a failure is
  `FORBIDDEN` and rolls the mutation back.
- The rules hold whichever side the write comes from: a connect through
  `Person.trips` (the same relationship type, the other direction)
  answers to `Trip.members`' rules, with source and target as declared on
  `Trip.members`. If both fields carry rules, both apply.
- A relationship failing `READ_EDGE` reads its properties as `FORBIDDEN`.
  Unless the claims alone settle the rule, nothing may filter, sort or
  aggregate by that relationship's properties.
- Deleting a node removes its relationships without `DISCONNECT` rules:
  who may delete the node is the type's `DELETE` rule.

```graphql
type Membership @relationshipProperties {
  role: String
    @authorization(
      validate: [
        {
          operations: [CREATE, UPDATE]
          where: { jwt: { roles: { includes: "admin" } } }
        }
      ]
    )
}
```

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
| `@fulltext`, `@vector`                 | FULLTEXT and VECTOR indexes, by name    |

`assertSchema()` reports what the database lacks (`missing`), and a
full-text or vector index present under its name but defined differently
(labels, fields, analyzer: `mismatched`), since search would keep using the
old definition. `assertSchema({ create: true })` creates what is missing
and drops and re-creates what is mismatched (`created`, `recreated`),
idempotently. `check()` fails on either.

**S2: plans are checked.** Every compiled statement records the access path
it was written for. `lora.explain(query, variables)` plans each statement and
reports a label scan where a seek was expected, a mutating plan behind a
read, or result columns that do not match. The access path checked is the
root field's own, outside every `CALL { }`: a label scan inside one (a
`@cypher` statement's `MATCH`) is reported as a lint-level `notes` entry,
not blamed on the root. `lora.check({ operations })` runs this over your
operations; the CLI does it in CI.

**S3: compile once.** `lora.persist({ id: source })` parses and validates
persisted operations at startup, and `lora.execute({ id, variables, context })`
runs them with no parsing or validation. Ad hoc documents passed to
`execute({ source })` are cached too. A subscription runs the same way with
`lora.subscribe({ id | source, variables, context })`, which returns an
async iterable of results (end it with `context.signal`); `execute()` answers
a subscription with an error pointing there. Translation itself takes about 0.13 ms
for a nested page, and statement text depends only on the shape of the
input, so LoraDB's own plan cache is hit for every repeat.

**S5: read-sets and write-sets.** See [change tracking](#change-tracking).

**S6: statistics and cost.** A relationship filter that names a related node
by key starts from that node and expands, instead of scanning the label.
Every operation has a cost estimate (rows touched, multiplying page sizes
through nested lists, capped by `@cardinality`), summed across its root
fields, and an operation over `maxCost` (default 50 000) fails with
`COST_EXCEEDED` before it runs. `lora.analyze()` counts nodes and
measures relationship degrees: a nested list is then estimated at its
relationship's maximum degree, measured over every node and capped by the
page size, instead of the page size alone. Not a percentile: the caller
picks the parents (by key, or by following a hub), so any lower bound is
one it can exceed at will.

Filters are charged the rows they examine, not only the rows they return:

- a root filter no index answers (`contains`, `endsWith`,
  `caseInsensitive`, `NOT`, `OR`, a computed field) costs the label's
  node count, or the page size without statistics; an equality, range,
  prefix or point predicate seeks and costs nothing extra;
- each relationship a filter follows (`some`, `none`, `all`, `single`,
  `count`, `aggregate`, connection filters, single relationships) costs
  the related nodes it visits per candidate, multiplied per level: the
  mean degree over a scanned label, the maximum degree from parents the
  caller picked (by key, or the parents of a nested list), the default
  page size without statistics;
- `totalCount` and aggregates read every match: the label's node count
  when no key narrows them.

`maxFilterDepth` (default 2) refuses a `where` nesting more relationship
levels with `BAD_USER_INPUT`, counted through single relationships,
`<field>Exists` and connection filters too. Without `analyze()` a two-level
filter over a large label still passes, priced at the page size: run it
in production so estimates use real counts.

**S7: one SDL, two diffs.** `diffSchemas(before, after)` reports the
database statements a change needs (index and constraint changes, relabels,
property renames, destructive ones flagged) and the API's breaking and
dangerous changes. A field renamed with `@alias` over the same property is
reported as an API break with no data migration.

## Change tracking

```ts
lora.onWrite((change) => {
  // Per type: lists, counts and connections of these types may have
  // changed, not only the entities. A broad change (a @cypher mutation)
  // names nothing, so it invalidates every type.
  const types = change.broad ? allNodeTypes : change.types;
  cache.invalidate(types.map((typename) => ({ typename })));
});

for await (const change of lora.changes({ signal })) publish(change);
```

A `WriteChange` lists the nodes `created`, `updated` and `deleted`, the
relationships `connected` and `disconnected` (by field and both keys),
`entities` (every node whose observable state changed, relationship ends
included), and the touched `types` and `relationshipTypes`. Every compiled
read carries a read-set (labels and relationship types);
`lora.affects(reads, change)` tells whether a cached read may be stale.
`@cypher` mutations have no known write-set and are reported with
`broad: true`. Only writes made through the library are seen. A consumer
that falls `maxQueuedChanges` (default 1000) behind is ended with an error
rather than buffering without bound.

## Subscriptions

```graphql
subscription {
  festivalChanged(
    operations: [CREATE, UPDATE]
    where: { capacity: { gt: 1000 } }
  ) {
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
relationship is an `UPDATE`. `where` tests the node as it is after the
write; `node` is read when the event is delivered, through the normal read
path. Events for nodes the subscriber cannot read are dropped (checked in
one query per write, not per event), and deletions, which cannot be checked
after the fact, go only to subscribers following that `key` without a
`where`. `@authentication(operations: [SUBSCRIBE])` guards the subscription
itself. Pass a `signal` in the context to end the stream with the request.

Every event has a `timestamp` (when the write was committed). With
`@subscription(relationships: true)` a type also gets `CONNECT` and
`DISCONNECT` events, one per relationship, with `relationship { field type
relatedType relatedKey }`. With `@subscription(previousState: true)`,
`UPDATE` and `DELETE` events carry `previousState`: the stored values
before the write (readable scalar fields without field-level rules), at
the cost of one read per write. Subscribers whose checks compile to the
same statement share it: twenty subscribers with the same `where` and
claims cost one visibility query and one node read per write.

By default subscriptions see the writes this instance makes. With
`changeFeed: true` (lora-node), subscriptions and `changes()` are fed by
the engine's committed change feed instead: every committed write to the
database, whichever path in the owning process made it (hand-written
Cypher, `@cypher` mutations, imports, other instances), in commit order,
with relationship ends resolved to their `@key`s. A database directory is
open in one process at a time, so this is not a cross-process feed. If the
reader falls behind the engine it resumes from its last position, which is
kept in memory: a new instance starts at the current commit and does not
replay earlier writes. `onWrite` still reports
this instance's mutations, and `previousState` needs them: the feed carries
the state after the write. Call `lora.close()` to stop the feed.

## Transactions

```ts
const tx = await lora.begin();
try {
  await graphql({
    schema,
    source: createOrder,
    contextValue: { jwt, transaction: tx },
  });
  await tx.execute("MATCH (s:Stock {sku: $sku}) SET s.count = s.count - 1", {
    sku,
  });
  await tx.commit(); // change events fire now
} catch (err) {
  await tx.rollback();
  throw err;
}
```

With `transaction` in the context, every operation of the request runs in
it, next to the application's own `tx.execute(cypher)`: they commit or roll
back together, reads see the transaction's writes, and change events wait
for the commit. A failed mutation rolls the transaction back.

## CLI

```sh
lora-graphql print schema.graphql              # the public SDL
lora-graphql requirements schema.graphql --ddl # constraints and indexes, as DDL
lora-graphql check schema.graphql --operations src/operations
lora-graphql compile schema.graphql --operations src/operations --out generated
lora-graphql analyze schema.graphql --database ./data   # statistics JSON
lora-graphql diff old.graphql new.graphql      # exit 1 on breaking changes
lora-graphql directives                        # directive SDL for editors
```

`check` builds an in-memory LoraDB (needs `@loradb/lora-node`), asserts the
schema, plans every `@cypher` statement and every query in the operation
files, and exits non-zero on any finding. It is the CI gate. Options:

- `--variables vars.json`: variables per operation name, instead of
  sample values for the required ones.
- `--context ctx.json`: the GraphQL context per operation name (`*` for
  the rest), e.g. `{ "*": { "jwt": { "sub": "u1" } } }`, so rules compile
  as they do for a signed-in caller. `check({ operations })` takes the
  same as `context` on each operation.
- `--baseline plans.json`: the operators of every statement; a plan that
  differs fails, so plan changes show up in review (`--update-baseline`
  accepts them; a missing file is written).
- `--row-budget n`: fail statements the engine estimates to scan more rows.
- `--database dir [--name app]`: check an existing database as it is, and
  report indexes it has that the API does not use.

`access` prints who may do what: for every type, guarded field and root
`@cypher` field, each operation as each kind of caller (anonymous, authenticated, and each role
the rules test, such as `roles:admin`), with the verdict (`allowed`,
`filtered`, `validated`, `masked`, `denied`, `unauthenticated`) and the
rules that decide it. `lora.accessMatrix()` returns the same list; its
order is stable, so a snapshot in CI turns access changes into diffs.

`check` lints authorization too:

- a filter rule every signed-in caller passes;
- a rule whose default `requireAuthentication` refuses anonymous callers
  a branch that needs no claims;
- field rules the schema's bypass skips, on a type that says neither
  `@authorization(bypass: false)` nor `bypass: true`;
- an UPDATE rule testing a single relationship the update input can
  re-point (`connect`, `disconnect`, `create`): the rule sees the node
  before (and, validated `AFTER`, after) the write, so a caller passing it
  on two targets moves the node. Tests that name the caller's own node
  (`isViewer`) are skipped. Fix:
  `@settable(onCreate: true, onUpdate: false)`;
- a CREATE validate rule testing a field UPDATE can change while no UPDATE
  rule (or field-level UPDATE rule) tests it: created as the rule demands,
  then changed;
- a validate rule (not `READ` or `CREATE`) or relationship rule testing a
  masked field the claims do not settle: rules see the stored value, so
  success versus `FORBIDDEN` tells the caller what the mask hides;
- nested `create`, `update` or `delete` into a type whose rules for that
  write refuse every signed-in caller without a role (an admin-only type,
  or the `@authorizationDefaults(mutations:)` default), offered by an input
  that caller can use: surface only admins can use. Fix:
  `nestedOperations: [CONNECT, DISCONNECT]`.

A `@mutation` type with a generated write no rule guards (no
`@authentication` or `@authorization` rule for it, and no
`@authorizationDefaults(mutations:)` default) fails the run: declare it
with `@authorization(public: [CREATE, ...])` when every caller may make
it. It also prints lint notes that do not fail the run: `CASE_INSENSITIVE`
and `IS_NULL` filters (no index applies), and list relationships without
`@cardinality` or statistics.

`compile` validates persisted operations (`.graphql` files, one entry per
operation, or a JSON map of id to source) into `manifest.json`, which
`lora.loadManifest()` registers without parsing or validating again, and
`operations.d.ts` with `<Operation>Variables` and `<Operation>Result`
types. The manifest records a hash of the public schema and is refused
for any other. Statement text is not in it: it depends on variable values
and claims, and is compiled per request (and cached).

`analyze` samples a database's label counts and relationship degrees; pass
its output to `lora.useStatistics()`.

`migrate neo4j schema.graphql [--operations dir]` rewrites an
`@neo4j/graphql` SDL: `@id` becomes `@key(generate: true)`, `@node` is
added, `@fulltext`, `@subscription(events:)` and `@relationship` arguments
are translated, and what has no equivalent (`@coalesce`, federation,
`connectOrCreate`, type-level `@vector`) is removed and listed as
`# TODO(migrate)` lines. With the client's operations (both the neo4j 5
`title_CONTAINS` and neo4j 6 `{ title: { contains } }` filter forms),
`@mutation`, `@filterable` and `@sortable` follow what they use; without
them, every type keeps its mutations and a TODO says to narrow them.

### Testing

```ts
import {
  createTestLoraGraphQL,
  expectSeeks,
} from "@loradb/lora-graphql/testing";

const t = await createTestLoraGraphQL({
  typeDefs,
  seed: ["CREATE (:Festival {key: 'f1', name: 'Sunland'})"],
});
expect(await t.data(`{ festival(key: "f1") { name } }`)).toEqual({
  festival: { name: "Sunland" },
});
await expectSeeks(
  t.lora,
  `{ festivals(where: { name: { eq: "Sunland" } }) { key } }`,
);
t.close();
```

`createTestLoraGraphQL` builds an in-memory database with the schema
asserted, runs the seed, and records every statement in `t.statements`.
`expectSeeks` throws, listing each plan finding, unless every root field of
the query uses the index access it was compiled for (`rowBudget` too).
Both need `@loradb/lora-node` and work with any test runner.

`expectAccess` checks who may do what, against the database:

```ts
await expectAccess(t, {
  as: { sub: "lou", roles: [] },
  allowed: [
    "read Trip lou:tomorrowland",
    'update Person lou {"name": "Lou"}',
    "connect Trip.members lou:tomorrowland → f1",
  ],
  denied: ['update-edge Trip.members lou:tomorrowland → f1 {"rsvp": "GOING"}'],
});
```

Entries are `read`, `create`, `update` or `delete` a `Type key`, and
`connect`, `disconnect` or `update-edge` a `Type.field key → key`, each
with optional input as JSON. Every entry runs as the caller in a
transaction that is rolled back, so probes leave no trace. Denied means
`FORBIDDEN`, `UNAUTHENTICATED`, `NOT_FOUND` or (for `read`) not visible;
any other error is a mismatch either way. All mismatches are reported at
once.

## Drivers, limits and errors

`loraDriver(db)` adapts a `Database` from `@loradb/lora-node` or
`@loradb/lora-wasm`. A read of one statement runs with `execute()` (with
lora-node, on a libuv worker, not the JavaScript thread), except a lookup
by `@key` that selects only stored fields, which streams its single row
synchronously; a read of several statements runs in one read-only
transaction. Mutations need interactive transactions, which only the Node
binding has, so the WASM binding serves reads.
`explain()`, and so plan checks, need the Node binding too.

| Option                       | Default       | Meaning                                             |
| ---------------------------- | ------------- | --------------------------------------------------- |
| `timeoutMs`                  | 10 000        | Per statement; a `signal` in the context cancels    |
| `operationTimeoutMs`         | 2 × timeout   | All root fields of one query; then `TIMEOUT`        |
| `maxConcurrentStatements`    | 2             | Statements one operation runs at once               |
| `maxCost`                    | 50 000        | Estimated rows per operation                        |
| `maxBatch`                   | 1000          | Nodes created or deleted per mutation; bulk `limit` |
| `compileCacheBytes`          | 64 MiB        | Compile cache and document cache, each              |
| `maxQueuedChanges`           | 1000          | How far a change consumer may fall behind           |
| `defaultLimit` / `maxLimit`  | 25 / 100      | Global page sizes; `@limit` may only lower `max`    |
| `callbacks`                  |               | Named callbacks for `@populatedBy`                  |
| `jwt`                        | `context.jwt` | Where the claims are                                |
| `onStatement`                |               | Observe every statement                             |
| `cursorSecret`               |               | Sign cursors; reject unsigned or forged ones        |
| `maskErrors`                 | production    | Clients get `DATABASE_ERROR` and an `id` only       |
| `onError`                    |               | Receives each database error's detail and `id`      |
| `guards`                     | see below     | Document limits; `false` turns them off             |
| `persistedOnly`              | false         | `execute()`, `subscribe()` run persisted ops only   |
| `budget`                     |               | Cost limit per request, from the context            |
| `onCost`                     |               | Each root field's estimate, total and limit         |
| `onStatementEnd`             |               | Duration, rows and error of every statement call    |
| `timing`                     | false         | `extensions.timing` in `execute()` results          |
| `tracer` / `traceStatements` |               | OpenTelemetry-style spans; Cypher text on request   |
| `metrics`                    |               | Counters and histograms (see below)                 |

Errors carry `extensions.code`: `BAD_USER_INPUT`, `INVALID_CURSOR`,
`LIMIT_EXCEEDED`, `COST_EXCEEDED`, `TIMEOUT` (with `operationTimeoutMs`),
`UNAUTHENTICATED`, `FORBIDDEN`,
`NOT_FOUND`, `CONSTRAINT_VIOLATION` (with `type` and `field`),
`DATABASE_ERROR` (with an `id`, also given to `onError`),
`PERSISTED_QUERY_ONLY` (`execute()` or `subscribe()` got a document under
`persistedOnly`) and `WRONG_OPERATION_TYPE` (`execute()` got a
subscription, or `subscribe()` a query or mutation). An invalid SDL
throws one `ModelError` listing every problem, each located by type and
field.

### Security defaults

`maxCost` bounds the rows an operation touches; the document guards bound
the document before that. `execute()`, `subscribe()` and `persist()` apply them, and
`lora.validationRules()` / `lora.envelopPlugin()` bring them to any other
server (GraphQL Yoga takes the plugin as is):

| Guard                   | Default    | Limit                                                                  |
| ----------------------- | ---------- | ---------------------------------------------------------------------- |
| `maxDepth`              | 12         | Field nesting, through fragments                                       |
| `maxIntrospectionDepth` | 20         | Nesting under `__schema` / `__type`                                    |
| `maxAliases`            | 30         | Aliased fields per document                                            |
| `maxRootFields`         | 20         | Root fields per operation                                              |
| `maxTokens`             | 5000       | Lexer tokens per document, while parsing                               |
| `maxListArgument`       | 1000       | Items per list argument of a `@cypher` field (`@size(max:)` overrides) |
| `maxFilterDepth`        | 2          | Relationship levels one `where` nests                                  |
| `maxListFilter`         | 1000       | Items in an `in` filter operand                                        |
| `maxStringFilter`       | 10 000     | Characters in a string filter operand (`eq`, `contains`, `in` items…)  |
| `introspection`         | production | Off when `NODE_ENV` is `production`                                    |

One query also has a time budget and a share of the engine: its root
fields (aliases included) run at most `maxConcurrentStatements` (default 2) statements at once, so one request cannot take every libuv worker
from the others, and after `operationTimeoutMs` (default twice
`timeoutMs`, 20 s) its statements are aborted and its unfinished fields
fail with `TIMEOUT`. Each statement also gets no more than the time the
operation has left. Mutations keep `timeoutMs` per statement, and
subscriptions are not bounded by the operation budget.

With `NODE_ENV=production`, database errors are masked and introspection
is off unless configured otherwise. See
[the threat model](../../docs/design/graphql-threat-model.md) for what
the library trusts and where each check runs.

### Observability

`onStatement` fires before a statement runs; `onStatementEnd` after, with
`durationMs`, `rows`, `error`, `mode`, the cost estimate, the operation
name and the persisted id. Reads report their batch of statements in one
event; mutations report each statement.

With `timing`, `execute()` also returns the request's timings to the
client:

```json
"extensions": {
  "cost": 51,
  "timing": {
    "totalMs": 4.21,
    "databaseMs": 3.05,
    "fields": { "all": { "totalMs": 2.9, "databaseMs": 2.4 }, "one": { "totalMs": 0.8, "databaseMs": 0.65 } }
  }
}
```

`totalMs` covers the whole `execute()` call (parse, validation,
execution); `databaseMs` the statements LoraDB ran; `fields` both per root
field, by response key. Pass `true`, or a function of the context to
decide per request (`timing: (ctx) => ctx.jwt?.roles?.includes("admin")`).
It is off by default: timings sent to clients can act as a timing side
channel. Servers calling graphql-js on `getSchema()` directly use
`onStatementEnd` or `tracer` instead.

Pass an OpenTelemetry tracer (`trace.getTracer("lora-graphql")`) as
`tracer` and each root field gets a `lora.graphql.field` span holding a
`lora.cypher` span per statement call, with `db.system`,
`db.operation.name` and `db.response.returned_rows`. The Cypher text goes
in `db.statement` only with `traceStatements: true`. `metrics` takes any
object with `counter(name, value, attributes)` and
`histogram(name, value, attributes)`: it receives
`lora.graphql.statements`, `lora.graphql.errors`,
`lora.graphql.statement.duration` (ms) and `lora.graphql.cost`.

`budget(context)` sets the cost limit per request (a plan, a user), and
`onCost` sees every estimate. `execute()` also returns the operation's
estimate as `extensions.cost`, so clients can tune their queries.

### Compile cache

`execute()` caches parsed documents, and each read root field caches its
compiled statements per field node, exact variables, claims and the
`$context` values the compile read (claims are folded into the text, so
each distinct set gets its own compile). A repeated `festivals(limit: 20)`
drops from 0.14 ms to 0.06 ms end to end. Servers that parse every request
themselves get new field nodes each time and do not benefit; use
`execute()` or persisted operations.

Both caches are bounded by size as well as by count: `compileCacheBytes`
(default 64 MiB, approximate) caps the compile cache and, separately, the
parsed documents (about 100 bytes per source character), evicting the
oldest first. A request whose variables for the field exceed 16 KiB (a
long `in:` list, an embedding vector) is compiled but not cached, and an
entry does not keep its document alive once the document cache drops it.

`check({ rowBudget })` flags statements whose largest engine row estimate
exceeds the budget; every plan report carries `estimatedRows` either way.

### Versions

`@loradb/lora-graphql` is released in lockstep with `@loradb/lora-node`:
version X.Y.Z declares `"@loradb/lora-node": "^X.Y.Z"` as its peer and is
tested against that binding. Upgrade both together. `graphql` 16 and 17 are
both supported peers; the test suite runs on each.

## Translation rules

Measured on LoraDB 0.15 over 20 000 festivals and 100 000 relationships
(`yarn bench`):

| Rule                                                                                 | Why                                                                                  |
| ------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------ |
| Relationship filters as `size([… \| 1])`; aggregates of related values with `reduce` | No `OPTIONAL MATCH`; aggregates nest safely                                          |
| `CALL { }` for nested lists, nested connections, `@cypher` and interface members     | The only way to sort, limit and aggregate per parent                                 |
| Ordered by an always-present string: `WHERE s >= ""`                                 | The planner walks the index in order and stops at the limit: 0.03 ms instead of 7 ms |
| Mutation statements seek the key in their own `MATCH`, then expand                   | The plan no longer depends on the optimizer finding the seek: 0.06 ms per connect    |
| A relationship filter naming a key starts from that node                             | 0.07 ms instead of 9.3 ms                                                            |
| Keyset predicates written out, led by `sortKey >= $v` on non-null keys               | The lead bound gets a range scan; `[a, b] > $list` does not                          |
| Lists sort by the requested fields only; connections add a unique tie-breaker        | Two sort keys cannot stream from an index                                            |
| Every value a parameter; every identifier from the model, escaped                    | No injection, stable statement text                                                  |
| Absent filters left out, never `($p IS NULL OR …)`                                   | Keeps the predicate visible to the planner                                           |

## Coming from @neo4j/graphql

| @neo4j/graphql                                              | lora-graphql                                                                                 |
| ----------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Every type gets every operation                             | Reads by default; mutations and subscriptions opt in                                         |
| Every field filterable by every operator                    | `@filterable(byValue:)`, checked against the type                                            |
| Offset cursors (`arrayconnection:N`)                        | Keyset cursors tagged with their sort (signed with a secret)                                 |
| `update`/`delete` with an optional `where`                  | By `@key`, or bulk with a required, bounded `where`                                          |
| `connect: { where }`, silently a no-op when nothing matches | Connect by key; a missing target is `NOT_FOUND`                                              |
| Single relationships not enforced; required ones refused    | Enforced from both sides; required ones supported                                            |
| `{ viewers: { add: 1 } }`                                   | `adjust: { viewers: { add: 1 } }`                                                            |
| `@id`, `@populatedBy`, `@timestamp`                         | `@key(generate: true)`, `@populatedBy`, `@timestamp`                                         |
| `@authorization` rules evaluated in Cypher                  | Claim checks folded in JavaScript; node rules compiled                                       |
| You pick indexes                                            | Inferred from the API, and verified with `explain()`                                         |
| Unbounded lists; complexity left to you                     | Every list bounded; a cost limit per operation                                               |
| Subscriptions over CDC                                      | `@subscription`, from library-made writes or, with `changeFeed: true`, every committed write |
| Interfaces and unions with `UNION`                          | Per-member subqueries merged by sort, each using its own indexes                             |
| Federation                                                  | Not supported                                                                                |

## LoraDB behaviours this works around

Found while building this package; each workaround goes away with its fix.
Fixed in the engine and no longer worked around: labels after the first
node of a `MATCH`, early `LIMIT` under a deadline or in a transaction,
`MERGE` with a bound end node (connect is one `MERGE`), writes in
`CALL { }`, temporal RANGE indexes (inferred again for temporal fields),
`x IN $list` seeks, `null` values in property maps, existence checks
before a following `SET`, list comparison (`[a, b] > $list`) and
`first()`.

| Behaviour                                                                                | Workaround                                                                |
| ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| An aggregate nested in a call (`head(collect(x))`, `collect(x)[0..2]`) is not aggregated | `WITH collect(x) AS c RETURN head(c)`; `reduce` for per-parent aggregates |
| `max`, `sum` and `avg` over durations are wrong                                          | Duration aggregates are folded with `reduce`                              |
| Integer division returns a float; negative list slices (`l[..-1]`) return `[]`           | `toInteger(a / b)` for Int fields; `l[..size(l) - n]`                     |
| `COUNT { … RETURN DISTINCT x }` and `UNION` inside `CALL` do not parse                   | `reduce` for distinct counts; per-member subqueries                       |

## Development

```sh
yarn test       # vitest: model, TCK snapshots, integration, auth, mutations, CLI
yarn test:graphql17  # the same suite on graphql 17
yarn bench      # latency on a seeded graph
yarn typecheck && yarn lint && yarn build
```
