---
title: GraphQL Directive Reference
sidebar_label: Directives
description: Every directive @loradb/lora-graphql understands, each with its arguments and a working example. Nodes and keys, fields, relationships, computed fields, reads, search, writes, subscriptions and access control.
---

# Directive reference

One SDL describes both the graph and the API, and directives are how you
say what you mean. This page has every directive, each with what it does,
its arguments and an example you can paste.

Every schema on this page is built by the package's test suite, and every
query is validated against the schema above it.

## How directives are checked

Unknown directives, unknown arguments and arguments of the wrong type are
errors at startup, because the directive definitions are prepended to
your SDL before it is validated. A directive in a position where it would
do nothing, such as `@limit` on a scalar field, is an error too, never
silently ignored.

To get the definitions for an editor or a codegen tool:

```bash
lora-graphql directives > directives.graphql
```

or import `directiveTypeDefs` from `@loradb/lora-graphql`.

A directive on `extend type`, `extend interface`, `extend union`,
`extend scalar` or `extend schema` counts as one on the definition.
Repeatable directives add up; a non-repeatable directive written on both
is an error.

## All directives

**Nodes and keys:**
[`@node`](#node) ·
[`@key`](#key) ·
[`@unique`](#unique) ·
[`@uniqueTogether`](#uniquetogether) ·
[`@index`](#index) ·
[`@relayId`](#relayid)

**Fields:**
[`@alias`](#alias) ·
[`@private`](#private) ·
[`@default`](#default) ·
[`@timestamp`](#timestamp) ·
[`@populatedBy`](#populatedby) ·
[`@readonly`](#readonly) ·
[`@settable`](#settable) ·
[`@selectable`](#selectable) ·
[`@storedAs`](#storedas)

**Relationships:**
[`@relationship`](#relationship) ·
[`@relationshipProperties`](#relationshipproperties) ·
[`@declareRelationship`](#declarerelationship) ·
[`@cardinality`](#cardinality)

**Computed fields:**
[`@cypher`](#cypher) ·
[`@size`](#size) ·
[`@range`](#range) ·
[`@customResolver`](#customresolver)

**Reading:**
[`@query`](#query) ·
[`@filterable`](#filterable) ·
[`@sortable`](#sortable) ·
[`@groupBy`](#groupby) ·
[`@limit`](#limit) ·
[`@plural`](#plural)

**Search:**
[`@fulltext`](#fulltext) ·
[`@vector`](#vector)

**Writing and events:**
[`@mutation`](#mutation) ·
[`@subscription`](#subscription)

**Access control:**
[`@jwt`](#jwt) ·
[`@jwtClaim`](#jwtclaim) ·
[`@viewer`](#viewer) ·
[`@authentication`](#authentication) ·
[`@authorization`](#authorization) ·
[`@authorizationRule`](#authorizationrule) ·
[`@authorizationRules`](#authorizationrules) ·
[`@authorizationDefaults`](#authorizationdefaults)

## Nodes and keys

### @node {#node}

Makes a type a kind of node in the graph. Every type you query or relate
to needs it.

```graphql
type Festival @node {
  key: String! @key
  name: String!
}

type Person @node(labels: ["Person", "Account"], plural: "people") {
  key: String! @key
}
```

Arguments:

- `labels`: the node's labels. Default: the type name. The first label is
  the primary one: reads match on it, indexes and constraints go on it,
  and no two types may share it. Creates set every label.
- `plural`: the name used in root fields. Default: the type name plus `s`
  (or `es`, or `ies`), so `Person` would be `persons`.

```graphql
{
  festivals {
    name
  }
  people {
    key
  }
  festival(key: "sunland") {
    name
  }
}
```

### @key {#key}

The node's identity: required, unique and never changed. Exactly one per
type, on a non-null `String`, `ID`, `Int` or `BigInt`.

```graphql
type Festival @node @mutation {
  key: ID! @key(generate: true)
  name: String!
}
```

The key is how a node is looked up, updated, deleted and connected. It is
also the last sort key of every list and the anchor of every cursor, so
paging stays stable. The key field is always filterable by `eq` and `in`
and always sortable.

Arguments:

- `generate`: `true` fills a UUID on create when the input leaves the key
  out. On `ID` or `String`. Default `false`.
- `scope`: `VIEWER` makes a created key start with the caller's
  [`@viewer`](#viewer) claim and the separator, as in `lou:tomorrowland`,
  so callers can only create under their own prefix.
- `separator`: the separator for `scope`. Default `:`.

```graphql
mutation {
  createFestivals(input: [{ name: "Sunland" }]) {
    festivals {
      key
    }
  }
}
```

See [owner-scoped keys](/docs/graphql/authorization-recipes#owner-scoped-keys)
for `scope: VIEWER` at work.

### @unique {#unique}

A uniqueness constraint on one field. A second node with the same value
fails with `CONSTRAINT_VIOLATION`.

```graphql
type Person @node @mutation {
  key: String! @key
  email: String! @unique
}
```

### @uniqueTogether {#uniquetogether}

No two nodes share a combination of fields. A field may be a scalar, a
single relationship (compared by the target's key) or one list
relationship (compared as a set). Repeatable.

```graphql
type Seat
  @node
  @mutation
  @uniqueTogether(fields: ["venue", "row", "number"]) {
  key: ID! @key(generate: true)
  row: String!
  number: Int!
  venue: Venue! @relationship(type: "IN_VENUE", direction: OUT)
}

type Venue @node {
  key: String! @key
}
```

Arguments:

- `fields`: the field names that must be unique together.
- `where`: a filter limiting which nodes are compared, for a rule that
  applies only to some of them.

It is enforced by every generated mutation, for every caller. It is not a
database constraint, so raw Cypher is not checked. See
[uniqueness across relationships](/docs/graphql/relationships#uniqueness-across-relationships).

### @index {#index}

An explicit index. You rarely need it: indexes are inferred from
`@filterable` and `@sortable`. Use it for a field that only your own
`@cypher` statements filter on.

```graphql
type Festival @node {
  key: String! @key
  externalId: String @index(kind: RANGE)
}
```

Argument:

- `kind`: `RANGE`, `TEXT` or `POINT`.

### @relayId {#relayid}

On the `@key` field: adds an opaque global `id` to the type, the `Node`
interface, and the root fields `node(id:)` and `nodes(ids:)`. Relay and
normalising client caches use these.

```graphql
type Festival @node {
  key: String! @key @relayId
  name: String!
}
```

```graphql
query ($id: ID!) {
  node(id: $id) {
    id
    ... on Festival {
      name
    }
  }
}
```

The type may not have a field of its own named `id`. `toGlobalId(type,
key)` and `fromGlobalId(id)` are exported for servers that need to build
or read one.

## Fields

### @alias {#alias}

The API name differs from the stored property. Useful for renaming a
field without migrating data.

```graphql
type Festival @node {
  key: String! @key
  name: String! @alias(property: "title")
}
```

Clients read and filter `name`; the node stores `title`.

### @private {#private}

Stored, never exposed. The field is in no output and no input, but rules
and `@cypher` statements can still use it.

```graphql
type Person @node {
  key: String! @key
  name: String!
  passwordHash: String @private
}
```

### @default {#default}

The value stored on create when the input leaves the field out.

```graphql
type Post @node @mutation {
  slug: String! @key
  title: String!
  published: Boolean! @default(value: false)
  status: String! @default(value: "DRAFT")
}
```

The default is applied once, on create. It also satisfies a non-null
field, so `published` is required in the output but optional in the
create input. On a relationship property it applies when the relationship
is created.

### @timestamp {#timestamp}

Set to the current time by the database. On `DateTime`, `LocalDateTime`
or `Date`.

```graphql
type Post @node @mutation {
  slug: String! @key
  title: String!
  createdAt: DateTime! @timestamp(operations: [CREATE])
  updatedAt: DateTime @timestamp
}
```

Argument:

- `operations`: which writes set it. Default `[CREATE, UPDATE]`.

A timestamp field is not in the create or update input: clients cannot
set it unless you [opt in](#supplying-a-computed-field).

### @populatedBy {#populatedby}

Computed on write by a function you pass in the `callbacks` option.

```graphql
type Festival @node @mutation {
  key: String! @key
  name: String!
  slug: String! @populatedBy(callback: "slug", operations: [CREATE])
}
```

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  callbacks: {
    slug: ({ operation, type, field, key, input, context }) =>
      String(input.name).toLowerCase().replace(/\W+/g, "-"),
  },
});
```

Arguments:

- `callback`: the name of the function in `callbacks`. A missing one
  fails at startup.
- `operations`: which writes call it. Default `[CREATE, UPDATE]`.

The callback receives the operation, the type and field names, the
node's key, the node's input in this mutation and the GraphQL context,
and may return a promise. It is the usual way to fill a field from the
caller's claims, such as a tenant id from `context.jwt`.

A non-null field is allowed when `operations` includes `CREATE`. A
callback that then returns `null` or `undefined` fails the mutation with
`CONSTRAINT_VIOLATION`. It cannot be combined with `@timestamp` or
`@default`, and it is not allowed on relationship properties.

### Supplying a computed field {#supplying-a-computed-field}

`@timestamp` and `@populatedBy` fields are computed, so they are not in
the inputs. To let some callers supply the value (a seed script
backfilling history, an import that keeps its dates), opt in with
`@settable` and a field-level `validate` rule for that operation saying
who may:

```graphql
type Claims @jwt {
  roles: [String!]
}

type Post @node @mutation {
  slug: String! @key
  title: String!
  createdAt: DateTime!
    @timestamp(operations: [CREATE])
    @settable(onCreate: true)
    @authorization(
      validate: [
        {
          operations: [CREATE]
          where: { jwt: { roles: { includes: "admin" } } }
        }
      ]
    )
}
```

A supplied value is stored as given; an omitted one is computed as
before. Without the rule the combination is a model error, so a computed
field never becomes client-settable by accident.

### @readonly {#readonly}

Readable, never settable by clients. The field leaves the create and
update inputs.

```graphql
type Festival @node @mutation {
  key: String! @key
  name: String!
  ticketsSold: Int @readonly
}
```

Use it for values your own code maintains, with Cypher or a `@cypher`
mutation. On a relationship it removes the field from both inputs.

### @settable {#settable}

Which mutations may set a field. `@readonly` is both set to `false`.

```graphql
type Order @node @mutation {
  key: String! @key
  currency: String! @settable(onUpdate: false)
  note: String @settable(onCreate: false)
}
```

Arguments:

- `onCreate`: the create input has the field. Default `true`.
- `onUpdate`: the update input has the field. Default `true`.

`currency` is chosen once and cannot change. `note` can only be added
later. On a relationship it decides whether the inputs offer the field;
on a relationship property, `onUpdate: false` also refuses a re-connect
that would change it.

### @selectable {#selectable}

Whether a field can be read or aggregated.

```graphql
type Person @node @mutation @query(aggregate: true) {
  key: String! @key
  name: String! @sortable
  pin: String @selectable(onRead: false)
  salary: Int @sortable @selectable(onAggregate: false)
}
```

Arguments:

- `onRead`: `false` makes the field write-only: it is in the inputs and
  not in the output type. Default `true`.
- `onAggregate`: `false` keeps the field out of aggregates. Default
  `true`.

### @storedAs {#storedas}

On a custom scalar: how its values are stored.

```graphql
scalar Email @storedAs(type: STRING)

type Person @node {
  key: String! @key
  email: Email @filterable
}
```

Argument:

- `type`: `STRING`, `INT`, `FLOAT`, `BOOLEAN`, `DATETIME` or `DATE`.

Filters, sorts and indexes follow the storage type. Values pass through
unchanged unless you supply an implementation, which can validate and
normalise them:

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  scalars: { Email: EmailScalar }, // a GraphQLScalarType
});
```

## Relationships

The [relationships](/docs/graphql/relationships) page covers everything
these generate, and
[many-to-many relationships](/docs/graphql/many-to-many) walks through
them from a relational join table.

### @relationship {#relationship}

Connects two node types. The target is the field's type: a `@node` type,
or an interface or union over node types.

```graphql
type Festival @node @mutation {
  key: String! @key
  genre: Genre @relationship(type: "IN_GENRE", direction: OUT) @filterable
  stages: [Stage!]!
    @relationship(type: "HAS_STAGE", direction: OUT, onDelete: CASCADE)
  followers: [User!]!
    @relationship(
      type: "FOLLOWS"
      direction: IN
      properties: "Follows"
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @filterable
}

type Genre @node {
  key: String! @key
}

type Stage @node @mutation {
  key: String! @key
  name: String!
}

type User @node {
  key: String! @key
}

type Follows @relationshipProperties {
  since: Int
}
```

Arguments:

- `type` (required): the relationship type, in `SCREAMING_SNAKE_CASE`.
- `direction` (required): `OUT` or `IN`, seen from the declaring type.
- `properties`: the name of a
  [`@relationshipProperties`](#relationshipproperties) type.
- `queryDirection`: `UNDIRECTED` reads follow the relationship both ways,
  for symmetric relationships such as friendship. Writes always use
  `direction`. Default `DIRECTED`.
- `onDelete`: what deleting this node does to the nodes the field
  reaches. `DETACH` removes the relationships, `CASCADE` deletes the
  related nodes too, `RESTRICT` refuses while any exist. Default
  `DETACH`.
- `nestedOperations`: which nested writes the mutation inputs offer.
  Default `[CREATE, CONNECT, DISCONNECT, UPDATE, DELETE]`. A sixth value,
  `UPDATE_EDGE`, offers updating the relationship's properties without
  the connected node.
- `aggregate`: `false` drops the connection's aggregates and the
  aggregate filter. Default `true`.

The field's type sets the cardinality: `Genre` is zero or one, `Genre!`
exactly one, and `[Stage!]!` any number. Lists are always written
`[T!]!`. Cardinality is enforced on every write, from both sides.

```graphql
{
  festivals(where: { followers: { some: { key: { eq: "ada" } } } }) {
    key
    genre {
      key
    }
    followersConnection(first: 10) {
      edges {
        properties {
          since
        }
        node {
          key
        }
      }
    }
  }
}
```

### @relationshipProperties {#relationshipproperties}

A type holding the properties of a relationship: data that belongs to the
link, not to either node. Name it in `@relationship(properties:)`.

```graphql
type Team @node @mutation {
  key: String! @key
  members: [Person!]!
    @relationship(type: "MEMBER_OF", direction: IN, properties: "Membership")
}

type Person @node {
  key: String! @key
}

type Membership @relationshipProperties {
  role: String! @default(value: "member") @filterable
  joinedAt: DateTime @timestamp(operations: [CREATE]) @sortable
}
```

Fields must be scalars. The type is not a node: it has no key and no root
field. A property takes `@filterable`, `@sortable`, `@alias`, `@private`,
`@readonly`, `@settable`, `@selectable`, `@default`, `@timestamp`,
`@authentication` and `@authorization`.

```graphql
{
  team(key: "core") {
    membersConnection(
      where: { edge: { role: { eq: "owner" } } }
      sort: [{ edge: { joinedAt: DESC } }]
    ) {
      edges {
        properties {
          role
          joinedAt
        }
        node {
          key
        }
      }
    }
  }
}
```

### @declareRelationship {#declarerelationship}

On an interface field: every implementation declares this relationship,
so clients can select it on the interface.

```graphql
interface Event {
  key: String!
  venue: Venue @declareRelationship
}

type Concert implements Event @node {
  key: String! @key
  venue: Venue @relationship(type: "HELD_AT", direction: OUT)
}

type Talk implements Event @node {
  key: String! @key
  venue: Venue @relationship(type: "HOSTED_AT", direction: OUT)
}

type Venue @node {
  key: String! @key
}
```

Implementations must agree on the target, the list shape and the
properties type. The relationship type and direction may differ.

```graphql
{
  events {
    key
    venue {
      key
    }
  }
}
```

### @cardinality {#cardinality}

The most related nodes a list relationship has. Cost estimates use it in
place of assuming a full page.

```graphql
type Festival @node {
  key: String! @key
  stages: [Stage!]!
    @relationship(type: "HAS_STAGE", direction: OUT)
    @cardinality(max: 12)
}

type Stage @node {
  key: String! @key
}
```

It is a statement about your data, not a limit the library enforces. With
it, deeper nested queries fit under `maxCost`. See
[statistics and cost](/docs/graphql/smart-layer#s6-statistics-and-cost).

## Computed fields

### @cypher {#cypher}

A field backed by your own Cypher statement, on a node type, on `Query`
or on `Mutation`.

```graphql
type Festival @node {
  key: String! @key
  name: String!
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
```

Arguments:

- `statement` (required): the Cypher. `this` is the parent node, each
  field argument is a `$parameter`, `$jwt` holds the request's claims and
  `$viewer` the key of the caller's node.
- `columnName`: the returned column. Inferred when the last `RETURN` has
  one item.

```graphql
{
  festivalCount
  festival(key: "sunland") {
    similar(limit: 5) {
      name
    }
  }
}
```

A statement may write only on `Mutation`. Statements are trusted server
code: they run with the database's full access, and rules on the types
they touch do not apply inside them. The
[@cypher fields](/docs/graphql/cypher-fields) page covers return types,
`$viewer`, guarding root fields and what the library checks.

### @size {#size}

On a list argument of a `@cypher` field: the most items it takes. More is
`BAD_USER_INPUT` before the statement runs.

```graphql
type Festival @node {
  key: String! @key
}

type Query {
  festivalsByKeys(keys: [String!]! @size(max: 50)): [Festival!]!
    @cypher(statement: "MATCH (f:Festival) WHERE f.key IN $keys RETURN f")
}
```

Without it, the `maxListArgument` option applies (1000 by default).

### @range {#range}

On an `Int` or `Float` argument of a `@cypher` field: inclusive bounds,
checked before the statement runs. For a list, each item.

```graphql
type Festival @node {
  key: String! @key
  capacity: Int
}

type Query {
  largest(
    count: Int = 10 @range(min: 1, max: 100)
    minCapacity: Int = 0 @range(min: 0)
  ): [Festival!]!
    @cypher(
      statement: """
      MATCH (f:Festival)
      WHERE f.capacity >= $minCapacity
      RETURN f ORDER BY f.capacity DESC LIMIT $count
      """
    )
}
```

Arguments:

- `min`, `max`: either or both.

### @customResolver {#customresolver}

A field computed in JavaScript by a resolver you pass in the `resolvers`
option.

```graphql
type Festival @node {
  key: String! @key
  name: String!
  capacity: Int
  label: String! @customResolver(requires: "name capacity")
}
```

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  resolvers: {
    Festival: { label: (source) => `${source.name} (${source.capacity})` },
  },
});
```

Argument:

- `requires`: a selection on the type, fetched in the same statement as
  the rest of the node, so the resolver finds it on its source even when
  the client did not ask for it.

A missing resolver or an invalid `requires` fails at startup. The field
cannot be filtered, sorted or grouped by, since it does not exist in the
database.

## Reading

### @query {#query}

Controls the generated reads of a type. Reads are on by default, so you
only need it to change them.

```graphql
type Festival @node @query(aggregate: true) {
  key: String! @key
  capacity: Int @sortable
}

type AuditEntry @node @query(read: false) {
  key: String! @key
}
```

Arguments:

- `read`: `false` removes the type's root fields, and with them its
  search and subscription fields. The type can still be reached through
  relationships. Default `true`.
- `aggregate`: `true` adds `<plural>Aggregate` and `<plural>Grouped`, and
  the `aggregate` field of the type's connections. Default `false`. It
  has no effect on an interface or union.

```graphql
{
  festivalsAggregate {
    count
    capacity {
      min
      max
      avg
    }
  }
}
```

### @filterable {#filterable}

Which filters clients may use on a field. Nothing is filterable without
it, except the key.

```graphql
type Festival @node {
  key: String! @key
  name: String! @filterable(byValue: [EQ, CONTAINS, CASE_INSENSITIVE])
  country: String! @filterable
  capacity: Int @filterable(byValue: [GTE, LT, IS_NULL])
  tags: [String!] @filterable
  location: Point @filterable(byValue: [WITHIN_BBOX, DISTANCE])
  genre: Genre @relationship(type: "IN_GENRE", direction: OUT) @filterable
}

type Genre @node {
  key: String! @key
}
```

Argument:

- `byValue`: the operators. Without it: `EQ` and `IN` (`EQ` alone on a
  `Boolean`, `INCLUDES` on a list). On a relationship field it takes no
  argument and enables
  [relationship filters](/docs/graphql/relationships#filtering-by-related-nodes).

The operators, and the index each one gets:

- `EQ`, `IN`: strings, ids, numbers, temporals, durations and enums. No
  index needed: LoraDB indexes equality lazily.
- `LT`, `LTE`, `GT`, `GTE`: strings, ids, numbers and temporals. A RANGE
  index.
- `CONTAINS`, `STARTS_WITH`, `ENDS_WITH`: strings and ids. A TEXT index.
- `CASE_INSENSITIVE`: strings and ids. No index can apply; prefer
  [`@fulltext`](#fulltext) on large data.
- `IS_NULL`: nullable fields. No index can apply.
- `INCLUDES`: lists.
- `WITHIN_BBOX`, `DISTANCE`: `Point` and `CartesianPoint`. A POINT index.

Each operator is checked against the field's type at startup, so
`CONTAINS` on an `Int` is an error, not a filter that never matches.

In a `where`, operators are camelCase:

```graphql
{
  festivals(
    where: {
      name: { caseInsensitive: { contains: "land" } }
      country: { in: ["NL", "BE"] }
      capacity: { gte: 1000, lt: 50000 }
      tags: { includes: "outdoor" }
      location: {
        distance: { from: { latitude: 52.37, longitude: 4.9 }, lte: 50000 }
      }
      genre: { key: { eq: "techno" } }
    }
  ) {
    name
  }
}
```

`caseInsensitive` nests the string operators the field also lists.
`distance` takes `{ from, lte }` in metres for geographic points, and
`withinBBox` takes `{ lowerLeft, upperRight }`; a `lowerLeft` longitude
greater than the `upperRight` one means the box crosses the antimeridian.

### @sortable {#sortable}

Clients may sort and paginate by the field. It gets a RANGE index, and
it becomes part of the type's aggregates.

```graphql
type Festival @node {
  key: String! @key
  name: String! @sortable
  startsOn: Date @sortable
}
```

```graphql
{
  festivalsConnection(first: 10, sort: [{ startsOn: DESC }, { name: ASC }]) {
    edges {
      node {
        name
      }
    }
  }
}
```

Not on lists, points or durations. Nulls sort last ascending and first
descending. On a relationship property it enables
`sort: [{ edge: { ... } }]` on the connection.

### @groupBy {#groupby}

Offers the field as a grouping key of `<plural>Grouped`. Needs
`@query(aggregate: true)`.

```graphql
type Festival @node @query(aggregate: true) {
  key: String! @key
  country: String! @groupBy
  capacity: Int @sortable
}
```

```graphql
{
  festivalsGrouped(by: [country]) {
    by {
      country
    }
    aggregate {
      count
      capacity {
        sum
      }
    }
  }
}
```

On a readable field that is not a list or a point.

### @limit {#limit}

Page size bounds, for a type's lists or for one relationship.

```graphql
type Festival @node @limit(default: 10, max: 50) {
  key: String! @key
  stages: [Stage!]!
    @relationship(type: "HAS_STAGE", direction: OUT)
    @limit(default: 5, max: 20)
}

type Stage @node {
  key: String! @key
}
```

Arguments:

- `default`: the page size when a list gets no `limit` or `first`.
  Globally 25, set with the `defaultLimit` option.
- `max`: the largest page. Globally 100, set with `maxLimit`. `@limit`
  may only lower it.

Asking for more than `max` is a `LIMIT_EXCEEDED` error, not a silently
shorter page.

### @plural {#plural}

The root field name of an interface or union. Types use
[`@node(plural:)`](#node).

```graphql
interface Event @plural(value: "happenings") {
  key: String!
}

type Concert implements Event @node {
  key: String! @key
}

union SearchResult @plural(value: "results") = Concert
```

```graphql
{
  happenings {
    key
  }
}
```

## Search

### @fulltext {#fulltext}

Full-text search over one or more fields: a FULLTEXT index and a search
root field per entry.

```graphql
type Festival
  @node
  @fulltext(
    indexes: [
      { fields: ["name", "description"] }
      { name: "festival_tags", fields: ["tags"], analyzer: SIMPLE }
    ]
  ) {
  key: String! @key
  name: String!
  description: String
  tags: [String!]
}
```

Each entry of `indexes` takes:

- `fields` (required): `String` or `ID` fields, or lists of them.
- `name`: the index name. The first defaults to `<label>_search`; later
  ones need a name.
- `analyzer`: `STANDARD` (default) or `SIMPLE`.
- `queryName`: the root field. Default `search<Plural>` for the first,
  `search<Plural>By<Name>` after.

```graphql
{
  searchFestivals(query: "open air techno") {
    score
    node {
      name
    }
  }
}
```

Terms are combined with AND, case and accents are folded, and a trailing
`*` matches a prefix. A field with a mask, a READ `validate` rule or READ
`@authentication` cannot be indexed, since searching it would reveal
which rows contain a hidden word.

### @vector {#vector}

On a `[Float!]` field: a VECTOR index and a similarity root field.

```graphql
type Festival @node {
  key: String! @key
  name: String!
  embedding: [Float!] @vector(dimensions: 384, similarity: COSINE)
}
```

Arguments:

- `dimensions` (required): 1 to 4096.
- `similarity`: `COSINE` (default) or `EUCLIDEAN`.
- `queryName`: the root field. Default `similar<Plural>`, or
  `similar<Plural>By<Field>` when the type has several vector fields.

Search from a query vector, or from an existing node with `to`:

```graphql
{
  similarFestivals(to: "sunland", limit: 5) {
    score
    node {
      name
    }
  }
}
```

## Writing and events

### @mutation {#mutation}

Generates mutations for a type. There are none without it.

```graphql
type Claims @jwt {
  roles: [String!]
}

type Festival
  @node
  @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE]) {
  key: String! @key
  name: String!
}

type Genre
  @node
  @mutation(operations: [CREATE])
  @authentication(operations: [CREATE]) {
  key: String! @key
}
```

Argument:

- `operations`: `CREATE`, `UPDATE` and `DELETE`. Default: all three.

`CREATE` generates `create<Plural>`, `UPDATE` generates `update<Type>`
and `update<Plural>`, `DELETE` generates `delete<Type>` and
`delete<Plural>`, and `CREATE` with `UPDATE` adds `upsert<Plural>`.

```graphql
mutation {
  createFestivals(input: [{ key: "sunland", name: "Sunland" }]) {
    info {
      nodesCreated
    }
  }
  updateFestival(key: "sunland", update: { name: "Sunland Open Air" }) {
    festival {
      name
    }
  }
}
```

`lora-graphql check` fails on a `@mutation` type whose writes nothing
guards, which is why the example has `@authentication`.

### @subscription {#subscription}

Generates a `<type>Changed` subscription. There is none without it.

```graphql
type Festival
  @node
  @mutation
  @subscription(relationships: true, previousState: true) {
  key: String! @key
  name: String!
  stages: [Stage!]! @relationship(type: "HAS_STAGE", direction: OUT)
}

type Stage @node {
  key: String! @key
}
```

Arguments:

- `operations`: which events the type sends. Default
  `[CREATE, UPDATE, DELETE]`.
- `relationships`: `true` also sends `CONNECT` and `DISCONNECT` events,
  one per relationship. Default `false`.
- `previousState`: `true` makes `UPDATE` and `DELETE` events carry the
  stored values before the write, at the cost of one read per write.
  Default `false`.

```graphql
subscription {
  festivalChanged(operations: [UPDATE]) {
    operation
    key
    node {
      name
    }
    previousState {
      name
    }
  }
}
```

## Access control

[Authentication](/docs/graphql/authentication) and the
[authorization recipes](/docs/graphql/authorization-recipes) show these
at work, with the response each caller gets. The
[authorization reference](/docs/graphql/authorization) defines every rule.

### @jwt {#jwt}

Declares the claims your rules use. With it, a rule naming any other
claim is an error at startup.

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Festival @node {
  key: String! @key
}
```

The type's name does not matter, and it is not part of the public
schema.

### @jwtClaim {#jwtclaim}

Where a declared claim lives in the token, when it is not at the top
level.

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!] @jwtClaim(path: "app_metadata.roles")
  tenant: String @jwtClaim(path: "org.id")
}

type Festival @node {
  key: String! @key
}
```

Argument:

- `path`: dots step into nested objects. Rules then use the short name,
  `roles`.

### @viewer {#viewer}

Marks the claim that identifies the caller's own node. It enables
`isViewer`, `viewer` and `${viewer.key}` in rules, and `$viewer` in
`@cypher` statements.

```graphql
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "subject")
}

type Person @node {
  key: String! @key
  subject: String! @unique
  name: String!
}
```

Arguments:

- `type`: the node type callers are.
- `field`: a `@key` or `@unique` field of that type that the claim
  equals.

Pointing it at a separate unique field, as here, lets keys stay readable
while the claim is an identity provider's opaque id.

### @authentication {#authentication}

Requires a signed-in caller, and optionally a claim. On a type or a
field. A caller who fails gets `UNAUTHENTICATED`.

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Article
  @node
  @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE]) {
  slug: String! @key
  title: String!
  internalNotes: String @authentication(operations: [READ])
}

type Report
  @node
  @authentication(jwt: { roles: { includes: "analyst" } }) {
  key: String! @key
}
```

Arguments:

- `operations`: `READ`, `CREATE`, `UPDATE`, `DELETE`,
  `CREATE_RELATIONSHIP`, `DELETE_RELATIONSHIP` and `SUBSCRIBE`. Default:
  all seven.
- `jwt`: a test the claims must also pass.

Anyone reads articles; only signed-in callers write them or read
`internalNotes`; only analysts touch reports.

### @authorization {#authorization}

Row-level rules, compiled into the statements. On a type or a field.

```graphql
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}

type Person @node {
  key: String! @key
  name: String!
  phone: String
    @authorization(mask: [{ unless: { node: { isViewer: true } } }])
}

type Note
  @node
  @mutation
  @authorization(
    filter: [{ where: { node: { owner: { isViewer: true } } } }]
    validate: [
      {
        operations: [CREATE]
        where: { node: { owner: { isViewer: true } } }
      }
    ]
  ) {
  key: String! @key
  text: String!
  owner: Person!
    @relationship(type: "OWNS", direction: IN)
    @settable(onUpdate: false)
}
```

Arguments:

- `filter` (on a type): rules that hide nodes the caller may not see.
  Each takes `operations` (default `[READ, UPDATE, DELETE]`) and `where`.
  Any passing rule grants.
- `validate` (on a type or a field): rules that fail the request with
  `FORBIDDEN`. Each takes `operations` (default
  `[READ, CREATE, UPDATE, DELETE]`), `when` (`BEFORE`, `AFTER`, default
  both) and `where`. On a relationship field they also take `CONNECT`,
  `DISCONNECT`, `UPDATE_EDGE` and `READ_EDGE`.
- `mask` (on a scalar field): `[{ unless, value }]`. A row failing
  `unless` reads the field as `value`, `null` when left out, with no
  error.
- `bypass` (on a type): `false` keeps the schema's bypass from skipping
  this type's rules.
- `public` (on a `@mutation` type): operations deliberately open to every
  caller, so `check` does not report them as unguarded.

A `where` takes `node` (a filter over the row), `jwt` (a test on the
claims), `viewer` (a filter over the caller's own node), `rule` (a named
rule), and `AND`, `OR`, `NOT`.

Here a note exists only for its owner, a note can only be created for
oneself, and a phone number reads as `null` to everyone but its owner.

### @authorizationRule {#authorizationrule}

Names a rule of a type, so several rules, and rules on related types, can
share it. Repeatable.

```graphql
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}

type Person @node {
  key: String! @key
}

type Project
  @node
  @authorizationRule(
    name: "member"
    where: { node: { members: { some: { isViewer: true } } } }
  )
  @authorization(filter: [{ where: { rule: "member" } }]) {
  key: String! @key
  members: [Person!]! @relationship(type: "MEMBER_OF", direction: IN)
}

type Task
  @node
  @authorization(
    filter: [{ where: { node: { project: { rule: "member" } } } }]
  ) {
  key: String! @key
  project: Project! @relationship(type: "PART_OF", direction: OUT)
}
```

Arguments:

- `name`: how rules refer to it, as `{ rule: "member" }`.
- `where`: the rule.

`Task` reaches the rule through its relationship, so "is a member" is
defined once.

### @authorizationRules {#authorizationrules}

Named rules over claims, for the whole schema. On `extend schema`.

```graphql
extend schema
  @authorizationRules(
    rules: [
      { name: "admin", where: { jwt: { roles: { includes: "admin" } } } }
      { name: "staff", where: { jwt: { roles: { includes: "staff" } } } }
    ]
  )

type Claims @jwt {
  roles: [String!]
}

type Invoice
  @node
  @authorization(
    filter: [{ where: { OR: [{ rule: "admin" }, { rule: "staff" }] } }]
  ) {
  key: String! @key
  total: Int!
}
```

Schema rules test claims only. A type's own
[`@authorizationRule`](#authorizationrule) of the same name is an error,
not an override.

### @authorizationDefaults {#authorizationdefaults}

Settings for every type at once. On `extend schema`.

```graphql
extend schema
  @authorizationDefaults(
    bypass: { jwt: { roles: { includes: "admin" } } }
    mutations: { jwt: { roles: { includes: "editor" } } }
    requireAuthentication: false
  )

type Claims @jwt {
  roles: [String!]
}

type Product @node @mutation {
  sku: String! @key
  name: String!
}
```

Arguments:

- `bypass`: a claims-only test. A request passing it skips every filter
  and validate rule. `@authentication` still applies.
- `mutations`: the write rule of every `@mutation` type, for each of
  `CREATE`, `UPDATE` and `DELETE` that the type's own rules do not cover.
- `requireAuthentication`: default `true`, meaning every rule denies
  without a token. `false` lets a rule that reads no claims decide for
  anonymous callers too.

`Product` has no rules of its own, so editors write it, everyone reads
it, and admins skip rules everywhere.

## Scalar types

Fields may be `String`, `ID`, `Int`, `Float`, `Boolean`, enums, `BigInt`
(a decimal string), `Date`, `Time`, `LocalTime`, `DateTime`,
`LocalDateTime`, `Duration` (ISO-8601 strings), `Point` and
`CartesianPoint` (objects, set with `PointInput` and
`CartesianPointInput`), custom scalars with [`@storedAs`](#storedas), and
non-null lists of these.

```graphql
enum Kind {
  INDOOR
  OUTDOOR
}

type Festival @node {
  key: ID! @key
  name: String!
  kind: Kind
  capacity: Int
  ticketPrice: Float
  soldOut: Boolean
  attendance: BigInt
  startsOn: Date
  doorsOpen: LocalTime
  createdAt: DateTime
  duration: Duration
  location: Point
  tags: [String!]
}
```
