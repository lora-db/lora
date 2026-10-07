---
title: Relationships in GraphQL
sidebar_label: Relationships
description: Everything @relationship generates in @loradb/lora-graphql, covering cardinality, direction, relationship properties, the read fields and connections, relationship filters, nested write inputs, delete behaviour and the rules the library enforces.
keywords: [graphql relationship, one-to-many, one-to-one, relationship properties, nested mutations, connect, disconnect]
---

# Relationships

A `@relationship` field connects two node types. From that one
declaration the library generates the read fields, a connection, filters,
nested write inputs and the checks that keep the graph consistent. This
page is the reference for all of it.

If you are coming from a relational database, start with
[many-to-many relationships](/docs/graphql/many-to-many), which walks
through the same material from a join table.

## Declaring one

```graphql
type Book @node @mutation {
  isbn: String! @key
  title: String!
  author: Author! @relationship(type: "WROTE", direction: IN)
  publisher: Publisher
    @relationship(type: "PUBLISHED_BY", direction: OUT, properties: "Deal")
  tags: [Tag!]! @relationship(type: "TAGGED", direction: OUT)
}
```

| Argument | Default | Meaning |
| --- | --- | --- |
| `type` | required | The relationship type. `SCREAMING_SNAKE_CASE`: it must match `[A-Z][A-Z0-9_]*` |
| `direction` | required | `OUT` or `IN`, seen from the declaring type |
| `properties` | none | The name of a [`@relationshipProperties`](#relationship-properties) type |
| `queryDirection` | `DIRECTED` | `UNDIRECTED` reads and filters follow the relationship both ways. Writes always use `direction` |
| `onDelete` | `DETACH` | What deleting this node does to the nodes the field reaches. See [deleting](#deleting) |
| `nestedOperations` | `[CREATE, CONNECT, DISCONNECT, UPDATE, DELETE]` | Which nested writes the mutation inputs offer. See [nested writes](#nested-writes) |
| `aggregate` | `true` | `false` drops the connection's `aggregate` and the `aggregate` filter |

The target is the field's type: a `@node` type, or an interface or union
over node types.

### Cardinality comes from the field type

| Field type | Cardinality | SQL equivalent |
| --- | --- | --- |
| `author: Author!` | Exactly one | A `NOT NULL` foreign key |
| `publisher: Publisher` | Zero or one | A nullable foreign key |
| `tags: [Tag!]!` | Any number | A join table, or the "many" side of a foreign key |

A list must be written `[T!]!`. Nested lists are not supported.

Cardinality is enforced on every write, from both sides, not only
declared:

- A single relationship holds at most one node. Connecting a second
  author to a book from the author's side fails with
  `CONSTRAINT_VIOLATION`: `Book.author holds one Author, but Book "1" would have 2`.
- A required relationship must be set on create
  (`BAD_USER_INPUT`: `Book.author is required: connect or create one`) and
  must stay set. Disconnecting it without a replacement, or deleting its
  target, is refused.

A type with `@mutation(operations: [CREATE])` and a required relationship
must let a create set it: the field has to be settable on create and keep
`CONNECT` or `CREATE` in `nestedOperations`. Otherwise the model is
rejected at startup.

### Both ends

A relationship can be declared from one end or from both. To make it
navigable from both, declare a field on each type with the same `type` and
opposite `direction`:

```graphql
type Author @node @mutation {
  id: ID! @key
  books: [Book!]! @relationship(type: "WROTE", direction: OUT)
}
```

`Author.books` and `Book.author` now read and write the same
relationships. The two fields can have different cardinalities, as here: a
book has one author and an author has many books, which is a one-to-many.
Two lists make a many-to-many, two single fields a one-to-one.

A type can relate to itself, and one relationship type can lead to several
target types; reads tell the targets apart by label.

### Other directives on a relationship field

| Directive | Effect |
| --- | --- |
| `@filterable` | Enables [relationship filters](#filtering-by-related-nodes). Takes no `byValue` |
| `@limit(default:, max:)` | Page size bounds for this list. List relationships only |
| `@cardinality(max:)` | The declared fan-out, used by cost estimates. List relationships only |
| `@settable(onCreate:, onUpdate:)` | Whether the create and update inputs offer the field |
| `@readonly` | Removes the field from both inputs |
| `@authentication`, `@authorization` | Who may read and write through the field. See [rules on relationship fields](/docs/graphql/authorization#relationship-rules) |

`@key`, `@unique`, `@sortable`, `@alias`, `@index`, `@default`,
`@timestamp`, `@populatedBy`, `@selectable`, `@private`, `@groupBy`,
`@vector`, `@relayId` and `@storedAs` are refused on a relationship field.

## Relationship properties

Data that belongs to the link, not to either node, goes in a
`@relationshipProperties` type named by `properties:`.

```graphql
type Deal @relationshipProperties {
  year: Int @filterable(byValue: [GTE, LT]) @sortable
  signedAt: DateTime @timestamp(operations: [CREATE])
  advance: Int @private
  status: String @default(value: "draft")
}
```

- Fields must be scalars. The type is not a node: it has no `@key`, no
  root field, and cannot be used as a field's type.
- A property takes `@filterable`, `@sortable`, `@alias`, `@private`,
  `@readonly`, `@settable`, `@selectable`, `@default`, `@timestamp`,
  `@authentication` and `@authorization`. It cannot take `@key`,
  `@unique`, `@index`, `@vector`, `@groupBy`, `@relayId` or
  `@populatedBy`.
- `@default` and `@timestamp(operations: [CREATE])` apply when the
  relationship is created, not when an existing pair is connected again.
  `@timestamp(operations: [UPDATE])` is stamped on edge updates and on
  re-connects that set properties.
- Relationship properties have no index. A filter or sort on one works per
  parent node, bounded by the page.
- When both ends declare the relationship, give both the same
  `properties` type.

## Reading

For `Festival.followers: [User!]!` the owner type gets:

```graphql
type Festival {
  followers(where: UserWhere, sort: [UserSort!], limit: Int): [User!]!
  followersConnection(
    where: FestivalFollowersConnectionWhere
    sort: [FestivalFollowersConnectionSort!]
    first: Int
    after: String
    last: Int
    before: String
  ): FestivalFollowersConnection!
}
```

A single relationship gets one field, `genre(where: GenreWhere): Genre`,
which is `null` when the related node does not match `where` or the caller
cannot read it.

### The list

The plain list returns related nodes, filtered and sorted by the target's
own `@filterable` and `@sortable` fields, at most `limit` of them (25 by
default, 100 at most, or the field's `@limit`). Without a `sort` they come
in `@key` order.

### The connection

Every list relationship to a `@node` type also gets `<field>Connection`,
a keyset-paginated connection. Use it for paging, totals, and for anything
that involves the relationship's properties.

```graphql
{
  festival(key: "f1") {
    followersConnection(
      first: 10
      where: {
        edge: { since: { gte: 2020 } }
        node: { name: { contains: "a" } }
      }
      sort: [{ edge: { since: DESC } }, { name: ASC }]
    ) {
      totalCount
      edges {
        cursor
        properties {
          since
        }
        node {
          key
          name
        }
      }
      pageInfo {
        hasNextPage
        endCursor
      }
      aggregate {
        count {
          nodes
          edges
        }
        edge {
          since {
            min
            max
          }
        }
      }
    }
  }
}
```

What the connection offers depends on the declaration:

| Part | Present when |
| --- | --- |
| `edges { cursor node }`, `pageInfo`, `totalCount` | Always |
| `edges { properties }` | The relationship has `properties:` and at least one property is readable |
| `where: { node, edge }` | The relationship has `properties:`. Without it, `where` is the target's own filter |
| `edge` inside `where` | At least one property is `@filterable` |
| `sort: [{ edge: {...} }]` | At least one property is `@sortable` |
| `aggregate` | `aggregate: true` (the default) and the target type has `@query(aggregate: true)` |
| `aggregate { edge }` | The relationship has aggregatable properties |

`aggregate.count` has `nodes` and `edges`. They differ only when several
relationships of the type join the same two nodes, which the generated
mutations never create.

:::note Limits of the generated reads
- A **single** relationship has no connection, so its properties can be
  written but not read, filtered or sorted through the generated API. If
  clients need them, read them with a [`@cypher` field](/docs/graphql/cypher-fields),
  or move them onto one of the nodes.
- A relationship to an **interface or union** gets the list but no
  connection.
:::

## Filtering by related nodes

`@filterable` on the relationship field adds it to the owner's `where`.

**Single relationships** take the target's filter directly, plus
`<field>Exists`:

```graphql
{
  books(where: { publisher: { name: { eq: "Tor" } } }) { isbn }
  unpublished: books(where: { publisherExists: false }) { isbn }
}
```

A related node the caller cannot read counts as absent. Because `x`
reserves `xExists`, a type cannot have a single relationship `publisher`
and a field named `publisherExists`.

**List relationships** take quantifiers over the target's filter:

| Filter | Matches owners where |
| --- | --- |
| `some: {...}` | at least one related node matches |
| `none: {...}` | no related node matches |
| `all: {...}` | every related node matches. True when there are none; a missing property fails it |
| `single: {...}` | exactly one related node matches |
| `count: { eq, lt, lte, gt, gte }` | the number of related nodes compares. Non-negative integers |
| `aggregate: { count, node, edge }` | an aggregate over the related nodes or the relationship properties compares |

`count` and `single` count each related node once, however many
relationships lead to it. A quantifier with an empty filter (`some: {}`)
is left out like any other empty filter: to ask "has any", use
`count: { gt: 0 }`.

**Conditions on the link** use `<field>Connection` in the `where`, which
exists for list relationships that have `properties:`. It quantifies over
pairs, so `node` and `edge` conditions apply to the same relationship:

```graphql
{
  festivals(
    where: {
      followersConnection: {
        some: { node: { key: { eq: "u1" } }, edge: { since: { gte: 2020 } } }
      }
    }
  ) {
    key
  }
}
```

It offers `some`, `all`, `none` and `single`. Here `single` counts pairs,
so parallel relationships to one node count separately.

**Aggregate filters** compare an aggregate of the related set:

```graphql
{
  festivals(
    where: {
      followers: {
        aggregate: {
          count: { gte: 100 }
          edge: { since: { min: { gte: 2015 } } }
          node: { name: { longestLength: { lte: 40 } } }
        }
      }
    }
  ) {
    key
  }
}
```

Every aggregatable field offers `min` and `max`; numbers and durations
add `avg` and `sum`; strings and ids add `shortestLength`,
`longestLength` and `averageLength`. Each takes `eq`, `lt`, `lte`, `gt` or
`gte`.

:::note Aggregate filters reach fields without @filterable
The `aggregate` filter covers every readable, non-list field of the target
and of the relationship properties (except booleans, enums and points),
whether or not the field is `@filterable`. If that exposes more than you
want, set `@relationship(aggregate: false)` on the field or
`@selectable(onAggregate: false)` on the property. Fields with row-level
read rules or a mask are refused with `FORBIDDEN`.
:::

Relationship filters nest: a `where` may go through at most
`maxFilterDepth` relationship levels (2 by default) and deeper is
`BAD_USER_INPUT`, because each level multiplies the work by the
relationship's fan-out.

When a `some` or `single` filter (or a single relationship's filter) names
the related node's key with `eq` or `in`, the compiler starts from that
node and walks the relationship back to the owners, instead of scanning
the owners and testing each. `lora.explain()` shows which shape an
operation gets.

## Nested writes

A relationship field appears in the owner's create, upsert and update
inputs. For a list relationship to `User`, keyed by `key`, with properties
`Follows`:

```graphql
input FestivalFollowersCreateRelationInput {
  # each: { key: String!, edge: FollowsCreateInput }
  connect: [FestivalFollowersConnectInput!]
  # each: { node: UserCreateInput!, edge: FollowsCreateInput }
  create: [FestivalFollowersCreateNodeInput!]
}

input FestivalFollowersUpdateRelationInput {
  connect: [FestivalFollowersConnectInput!]
  create: [FestivalFollowersCreateNodeInput!]
  # keys of the nodes to disconnect
  disconnect: [String!]
  # each: { key: String!, edge: FollowsUpdateInput, node: UserUpdateInput }
  update: [FestivalFollowersUpdateConnectedInput!]
  # { where: UserWhere, limit: Int }
  delete: FestivalFollowersNestedDeleteInput
}
```

The key field in `connect` and `update` has the name and type of the
target's `@key`. A single relationship has the same operations with
single values:

| Operation | List relationship | Single relationship |
| --- | --- | --- |
| `connect` | `[{ key, edge }]` | `{ key, edge }` |
| `create` | `[{ node, edge }]` | `{ node, edge }` |
| `disconnect` | `[key]` | `true` |
| `update` | `[{ key, edge, node }]` | `{ edge, node }` |
| `delete` | `{ where, limit }` | `true` |

Create and upsert inputs offer `connect` and `create` only, so an upsert
cannot disconnect.

What each operation does:

- **`connect`** links an existing node by key. A key that does not exist,
  or that the caller cannot read, is `NOT_FOUND` and the mutation writes
  nothing. Connecting a pair that is already connected keeps the one
  relationship, sets the properties given and keeps the others. The same
  pair twice in one input makes one relationship, with the last mention's
  properties.
- **`create`** creates the related node and links it. It follows the
  target's own rules and counts toward `maxBatch`.
- **`disconnect`** removes the link and keeps both nodes. Keys that are
  not connected are ignored.
- **`update`** changes a connected node (`node`), the relationship's
  properties (`edge`), or both, in place. A node that is not connected is
  `NOT_FOUND`. `null` removes a property.
- **`delete`** deletes the related nodes themselves, following their
  `onDelete` rules. On a list it takes a `where` (all connected nodes when
  absent) and a `limit` (default `maxBatch`); more matches than `limit` is
  `LIMIT_EXCEEDED` and nothing is deleted.

On a single relationship, `connect` or `create` on update replaces the
current target, and `connect` together with `create` is `BAD_USER_INPUT`.

### Which operations an input offers

An operation is offered only when everything it needs is there:

| Operation | Needs |
| --- | --- |
| `connect` | `CONNECT` in `nestedOperations` |
| `create` | `CREATE`, and the target type has `@mutation` with `CREATE` |
| `disconnect` | `DISCONNECT` |
| `update` with `node` | `UPDATE`, the target has `@mutation` with `UPDATE`, and it has something updatable |
| `update` with `edge` | `UPDATE` or `UPDATE_EDGE`, and a property settable on update |
| `delete` | `DELETE`, and the target has `@mutation` with `DELETE` |

`UPDATE_EDGE` is a sixth `nestedOperations` value that is not in the
default list. `nestedOperations: [CONNECT, UPDATE_EDGE]` lets clients link
nodes and edit the link's properties without being able to edit the
connected node. It needs `properties:`.

A relationship input with no operation left is omitted from the parent
input, and `nestedOperations: []` makes a relationship read-only through
its owner.

Bulk `update<Plural>(where:)` changes properties only. Passing a
relationship in its `update` is `BAD_USER_INPUT`: `updateBook changes
relationships; updateBooks changes properties`.

### Interface and union targets

A relationship to an interface or union connects, creates and disconnects
per member type:

```graphql
mutation {
  updateFestival(
    key: "f1"
    update: {
      events: {
        connect: { Concert: [{ key: "c1" }], Talk: [{ key: "t1" }] }
        disconnect: { Concert: ["c0"] }
      }
    }
  ) {
    info {
      relationshipsCreated
    }
  }
}
```

There is no nested `update` or `delete` through an abstract target, and
no connection or `aggregate` filter. A single relationship to a union
holds one node across all members.

## Deleting

`onDelete` says what deleting the declaring node does to what the field
reaches:

| `onDelete` | Deleting the node | SQL equivalent |
| --- | --- | --- |
| `DETACH` (default) | Removes the relationships; related nodes stay | `ON DELETE CASCADE` on a join table |
| `CASCADE` | Deletes the related nodes too, checking the caller may delete each | `ON DELETE CASCADE` on a child table |
| `RESTRICT` | Refuses while related nodes exist | A foreign key with no `ON DELETE` |

`RESTRICT` fails with `CONSTRAINT_VIOLATION`: `Author "a1" still has books
(onDelete: RESTRICT); remove them first`. A `CASCADE` that would reach
more than `maxBatch` nodes is `LIMIT_EXCEEDED` and deletes nothing.

Deleting a node that another type requires (the only author of a book
whose `author` is `Author!`) is refused with `CONSTRAINT_VIOLATION` unless
that relationship cascades.

## Uniqueness across relationships

`@uniqueTogether` on a node type makes a combination of fields unique. A
field may be a scalar, a single relationship (compared by the target's
key) or one list relationship (compared as a set):

```graphql
type Enrollment
  @node
  @mutation
  @uniqueTogether(fields: ["student", "course", "term"]) {
  id: ID! @key(generate: true)
  student: Student! @relationship(type: "HAS_ENROLLMENT", direction: IN)
  course: Course! @relationship(type: "FOR_COURSE", direction: OUT)
  term: Term! @relationship(type: "IN_TERM", direction: OUT)
}
```

A second node with the same combination fails with
`CONSTRAINT_VIOLATION`. The directive is repeatable and takes an optional
`where` to make the rule apply only to matching nodes. It is enforced by
every generated mutation, for every caller; it is not a database
constraint, so writes made with raw Cypher or a `@cypher` mutation are not
checked.

## Relationships on interfaces

To select a relationship on an interface, declare it there with
`@declareRelationship` and give every implementation a `@relationship`
field of the same name:

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
```

Implementations must agree on the target, the list shape and the
properties type. The relationship type and direction may differ.

## Change events

A node that gains or loses a relationship sends an `UPDATE` event to
subscribers of its type. With `@subscription(relationships: true)` the
type also sends `CONNECT` and `DISCONNECT` events, one per relationship,
carrying `relationship { field type relatedType relatedKey }`. On the
server, `onWrite` and `changes()` list every link written as `connected`
and `disconnected` entries with the declaring `Owner.field` and both keys.
See [subscriptions](/docs/graphql/generated-api#subscriptions).

## Limits

| Limit | Default | Applies to |
| --- | --- | --- |
| `@limit` / `defaultLimit`, `maxLimit` | 25, 100 | Nodes per relationship list or connection page |
| `maxFilterDepth` | 2 | Relationship levels in one `where` |
| `maxBatch` | 1000 | Nodes one mutation creates, updates or deletes, nested ones included |
| 10 × `maxBatch` | 10,000 | Relationships one mutation writes: links, edge updates and disconnects |
| `maxCost` | 50,000 | Estimated rows one operation touches. Nested lists multiply; `@cardinality(max:)` or `analyze()` tightens the estimate |

## See also

- [Many-to-many relationships](/docs/graphql/many-to-many)
- [Rules on relationship fields](/docs/graphql/authorization#relationship-rules)
- [Directive reference](/docs/graphql/directives)
- [The generated API](/docs/graphql/generated-api)
