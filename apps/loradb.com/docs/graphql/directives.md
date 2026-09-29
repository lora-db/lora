---
title: GraphQL Directive Reference
sidebar_label: Directives
description: Every directive @loradb/lora-graphql understands, grouped into model directives (how the graph is stored) and API directives (what clients may do).
unlisted: true
---

# Directive reference

One SDL describes the graph and the API. Directives fall into two groups:

- **Model directives** say how types map to nodes, relationships and
  properties.
- **API directives** say what clients may read, write, filter, sort and
  subscribe to, and under which rules.

Unknown directives, unknown arguments and arguments of the wrong type are
errors at startup, because the directive definitions are prepended to your
SDL before it is validated. To get those definitions for an editor or a
codegen tool, print them:

```bash
lora-graphql directives > directives.graphql
```

or import `directiveTypeDefs` from `@loradb/lora-graphql`.

## Model directives

| Directive | On | Meaning |
| --- | --- | --- |
| `@node(labels:, plural:)` | type | A node label set. Default: the type name. `plural` names the root fields |
| `@key(generate:)` | field | Required, unique and immutable. The sort tie-breaker, cursor anchor and mutation address. `generate: true` fills a UUID on create |
| `@unique` | field | A uniqueness constraint |
| `@index(kind: RANGE \| TEXT \| POINT)` | field | An explicit index. Usually inferred from `@filterable` and `@sortable` |
| `@relationship(...)` | field | An edge to a `@node` type, interface or union. See [below](#relationship) |
| `@declareRelationship` | interface field | Every implementation declares this relationship (type and direction may differ), so clients can select it on the interface |
| `@relationshipProperties` | type | Properties carried by a relationship type |
| `@alias(property:)` | field | The API name differs from the stored property |
| `@private` | field | Stored, never exposed |
| `@readonly` | field | Exposed, never client-settable |
| `@settable(onCreate:, onUpdate:)` | field | Which mutations may set it, for example set once on create |
| `@selectable(onRead:, onAggregate:)` | field | `onRead: false` makes a field write-only |
| `@default(value:)` | field | Stored on create when the input leaves the field out |
| `@timestamp(operations: [CREATE, UPDATE])` | field | Set to the current time; never client-settable |
| `@populatedBy(callback:, operations:)` | field | Computed on write by a named callback from the `callbacks` option |
| `@cardinality(max:)` | list relationship | Declared fan-out, used by cost estimates |
| `@cypher(statement:, columnName:)` | field | A field backed by a Cypher statement. See [below](#cypher) |
| `@customResolver(requires:)` | field | A field computed in JavaScript by a resolver from the `resolvers` option |
| `@storedAs(type:)` | custom scalar | How a custom scalar is stored: `STRING`, `INT`, `FLOAT`, `BOOLEAN`, `DATETIME` or `DATE` |
| `@fulltext(indexes: [{ name, fields, analyzer, queryName }])` | type | FULLTEXT indexes, each with a search root field |
| `@vector(dimensions:, similarity:, queryName:)` | `[Float!]` field | A VECTOR index and a similarity root field |
| `@plural(value:)` | interface, union | The root field's name |

### @relationship

```graphql
followers: [User!]!
  @relationship(
    type: "FOLLOWS"
    direction: IN
    properties: "Follows"
    queryDirection: DIRECTED
    onDelete: DETACH
    nestedOperations: [CONNECT, DISCONNECT]
    aggregate: true
  )
```

| Argument | Default | Meaning |
| --- | --- | --- |
| `type` | required | The relationship type, for example `FOLLOWS` |
| `direction` | required | `OUT` or `IN`, seen from the declaring type |
| `properties` | none | A `@relationshipProperties` type for properties on the edge |
| `queryDirection` | `DIRECTED` | `UNDIRECTED` reads follow the relationship both ways; writes use `direction` |
| `onDelete` | `DETACH` | Deleting this node: `DETACH` removes the relationships, `CASCADE` deletes the related nodes too, `RESTRICT` refuses while related nodes remain |
| `nestedOperations` | all | Which nested writes mutation inputs offer: `CREATE`, `CONNECT`, `DISCONNECT`, `UPDATE`, `DELETE` |
| `aggregate` | `true` | `false` drops the field's aggregates and aggregate filter |

A single relationship (`genre: Genre`) holds at most one node, and a
required one (`author: User!`) must stay set. Both are enforced from both
sides on every write, not only declared.

### @cypher

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
```

- `this` is the parent node, field arguments are `$parameters`, and `$jwt`
  holds the request's claims.
- `columnName` is inferred from `RETURN x` or `RETURN ... AS x`.
- A `@cypher` field may return scalars, `@node` types, interfaces or unions
  over them, or an object type without `@node` whose fields are read from a
  returned map.
- On `Query` and object types the statement may not write. On `Mutation` it
  may, but the library cannot know what it wrote: change events for it are
  broad.
- Statements are trusted server code. They run with the database's full
  access, and rules on the owning type do not reach inside them.

The library checks statements at startup (every `$parameter` must be an
argument or `$jwt`, no writes outside `Mutation`) and plans each one with
`explain()` in `check()`. See [the smart layer](/docs/graphql/smart-layer#s4-cypher-checks).

### @customResolver and @storedAs

```graphql
scalar Email @storedAs(type: STRING)

type Festival @node {
  key: String! @key
  name: String!
  capacity: Int
  label: String! @customResolver(requires: "name capacity")
  contact: Email @filterable
}
```

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  resolvers: {
    Festival: { label: (src) => `${src.name} (${src.capacity})` },
  },
  scalars: { Email: EmailScalar }, // a GraphQLScalarType; optional
});
```

`requires` is a selection on the type, fetched in the same statement as the
rest of the node. A missing resolver or an invalid `requires` fails at
startup. A custom scalar's filters, sorts and indexes follow its storage
type; without an implementation in `scalars` its values pass through.

## API directives

| Directive | On | Meaning |
| --- | --- | --- |
| `@query(read:, aggregate:)` | type, interface, union | Generated reads. On by default; `aggregate: true` adds aggregate and grouped roots |
| `@mutation(operations: [CREATE, UPDATE, DELETE])` | type | Generated mutations. None without it |
| `@subscription(operations:, relationships:, previousState:)` | type | Generated subscriptions. None without it |
| `@filterable(byValue: [...])` | field | Filter operators. Bare: `EQ` and `IN` (lists: `INCLUDES`). On a relationship: enables relationship filters |
| `@sortable` | field | Sort and keyset-paginate by this field. On a relationship property: `sort: [{ edge: { ... } }]` |
| `@groupBy` | field | A grouping key of `<plural>Grouped(by:)`. Needs `@query(aggregate: true)` |
| `@limit(default:, max:)` | type, interface, union, list relationship | Page size bounds. May only lower the global `max` |
| `@relayId` | `@key` field | Adds a global `id` and the `Node` interface |
| `@authentication(operations:, jwt:)` | type, field | Needs an authenticated request whose claims satisfy `jwt` |
| `@authorization(filter:, validate:)` | type (filter and validate), field (validate) | Row-level rules, compiled into statements |
| `@jwt`, `@jwtClaim(path:)` | type, field | The claims shape; rules may only use declared claims |

The [authorization](/docs/graphql/authorization) page covers the last three
in depth.

### Filter operators

| Operator | Applies to | Index it needs |
| --- | --- | --- |
| `EQ`, `IN` | strings, ids, numbers, temporals, durations, enums (`Boolean`: `EQ` only) | none: LoraDB indexes equality lazily |
| `LT`, `LTE`, `GT`, `GTE` | strings, ids, numbers, temporals (not durations) | RANGE |
| `CONTAINS`, `STARTS_WITH`, `ENDS_WITH` | strings | TEXT |
| `CASE_INSENSITIVE` | strings | none can apply; prefer `@fulltext` on large labels |
| `IS_NULL` | nullable fields | none can apply |
| `INCLUDES` | lists | none |
| `WITHIN_BBOX`, `DISTANCE` | `Point`, `CartesianPoint` | POINT |

Each operator is checked against the field's type at startup, so
`CONTAINS` on an `Int` is an error, not a filter that never matches.

### @subscription options

| Argument | Default | Meaning |
| --- | --- | --- |
| `operations` | `[CREATE, UPDATE, DELETE]` | Which events the type sends |
| `relationships` | `false` | Also send `CONNECT` and `DISCONNECT` events, one per relationship |
| `previousState` | `false` | `UPDATE` and `DELETE` events carry the stored values before the write, at the cost of one read per write |

### @limit and the global limits

Every list is bounded. `limit` and `first` default to `@limit(default:)`
(globally 25, set with `defaultLimit`), and asking for more than `max`
(globally 100, set with `maxLimit`) is a `LIMIT_EXCEEDED` error, not a
silent clamp.

## Scalar types

`String`, `ID`, `Int`, `Float`, `Boolean`, enums, `BigInt` (a decimal
string), `Date`, `Time`, `LocalTime`, `DateTime`, `LocalDateTime`,
`Duration` (ISO-8601 strings), `Point` and `CartesianPoint` (objects, set
with `PointInput` and `CartesianPointInput`), custom scalars with
`@storedAs`, and non-null lists of these.
