---
title: "@cypher Fields"
sidebar_label: "@cypher fields"
description: Back a GraphQL field, query or mutation with your own Cypher statement in @loradb/lora-graphql, with parameters, the caller as $viewer, argument bounds, authorization on root fields, and the checks the library runs on every statement.
keywords: [graphql cypher directive, custom resolver, custom query, custom mutation, viewer]
---

# @cypher fields

The generated API covers lookups, lists, filters, aggregates and keyed
writes. For anything else (a recommendation, a multi-hop traversal, a
domain-specific write) back a field with a Cypher statement of your own.
A `@cypher` field can sit on a node type, on `Query` or on `Mutation`.

```graphql
type Festival @node {
  key: ID! @key
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

type Mutation {
  renameGenre(key: String!, name: String!): Genre
    @cypher(
      statement: """
      MATCH (g:Genre) WHERE g.key = $key
      SET g.name = $name
      RETURN g
      """
    )
}
```

`Query` and `Mutation` are the only places you declare root fields
yourself, and every field on them needs `@cypher`.

## What a statement can use

| Name | Holds |
| --- | --- |
| `this` | The parent node, on a field of a node type |
| `$<argument>` | Each field argument, by name |
| `$jwt` | The request's claims as one map. `null` when signed out |
| `$viewer` | The `@key` of the caller's node. See [the caller](#the-caller-viewer) |

Any other `$name` is a model error at startup. There is no `$context`:
pass what a statement needs as an argument or a claim.

Arguments must be scalars, enums or lists of them. `jwt` and `viewer` are
reserved argument names. SDL defaults (`limit: Int = 3`) are published in
the schema and applied at execution, and an explicit `null` for an
argument that has a default gets the default too. That matters for
`LIMIT` and `SKIP`, which the engine refuses with `null`: give those
arguments a default.

## What a field can return

- **Scalars and enums**, or lists of them.
- **`@node` types.** The returned nodes are projected with the client's
  selection like any other node, including nested relationships, and the
  type's read rules apply to them.
- **Interfaces and unions** over `@node` types. Each node is projected as
  the member its label says.
- **An object type without `@node`**, whose fields are read from a
  returned map:

  ```graphql
  type FestivalStats {
    events: Int!
    titles: [String!]!
  }

  type Festival @node {
    key: ID! @key
    stats: FestivalStats!
      @cypher(
        statement: """
        MATCH (this)-[:HOSTS]->(e:Event)
        RETURN { events: count(e), titles: collect(e.title) } AS s
        """
      )
  }
  ```

`columnName` names the returned column. It is inferred when the
statement's last top-level `RETURN` has exactly one item, written
`RETURN x` or `RETURN ... AS x`. Commas inside calls, lists, maps and
`CALL { }` do not count as separate items. With several items, set
`columnName`.

Lists must be written `[T!]` or `[T!]!`: nested lists and nullable items
are refused.

## The caller: $viewer {#the-caller-viewer}

With a [`@viewer` claim](/docs/graphql/authorization#viewer), `$viewer` is
the `@key` of the caller's own node, in field statements and mutation
statements alike. A statement names the viewer, not the claim:

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
maps to another unique field (an opaque `subject`, say), it is looked up
in the statement with one index seek by the claim. So the statement above
keeps working, and keeps seeking, if you later move `@viewer` from
`Person.key` to `Person.subject`.

`$viewer` is `null` when the caller is signed out, when the claim is not a
string or a number, and when the token names no node. Using `$viewer`
without a `@viewer` claim in the schema is a model error.

## Guarding root fields

Rules on a node type do not reach a root `@cypher` field: it has no node
to test. Guard it directly with `@authentication` and
`@authorization(validate:)`, which are checked before the statement runs.

```graphql
type Mutation {
  verify: Person
    @authentication
    @authorization(
      validate: [{ where: { viewer: { verified: { eq: true } } } }]
    )
    @cypher(
      statement: """
      MATCH (p:Person) WHERE p.key = $viewer
      SET p.verified = true
      RETURN p
      """
    )
}
```

- A rule here tests the caller's claims (`jwt`) and the caller's own node
  (`viewer`). `node`, `filter` rules and `mask` are model errors, and
  `operations` and `when` make no difference: every rule guards the call.
- As everywhere, any passing rule grants, and the schema's
  `@authorizationDefaults(bypass:)` skips them. A root `@cypher` field
  cannot opt out of the bypass.
- Claim tests are decided in JavaScript, so a refusal (`FORBIDDEN`, or
  `UNAUTHENTICATED` without a token) runs no statement at all. A `viewer`
  test costs one seek before the statement, in the mutation's transaction.
- Every root `@cypher` field is in the
  [access matrix](/docs/graphql/authorization#checking-access), guarded or
  not. `Mutation` fields appear under the operation `EXECUTE`.

:::warning Statements are trusted server code
A statement runs with the database's full access. Rules on the types it
touches are not applied inside it: a `Query` field that returns a scalar
computed from private data returns it to every caller the field's own
guards let through. Nodes it returns as `@node` types do get that type's
read rules.
:::

## Bounding arguments

A statement sees its arguments as the client sent them, so the library
bounds them before anything runs.

**Lists** take at most `@size(max:)` items, or `maxListArgument` (default
1000) without it. Each level of a nested list counts.

```graphql
type Mutation {
  createPost(key: String!, hashtags: [String!] = [] @size(max: 30)): Post
    @cypher(statement: "...")
}
```

**Numbers** take `@range(min:, max:)`, with either bound or both,
inclusive. It checks an `Int` or `Float` argument, or each item of a list.
`null` is not checked; nullability is the type's business.

```graphql
type Query {
  nearby(
    lat: Float! @range(min: -90, max: 90)
    lon: Float! @range(min: -180, max: 180)
    km: Int = 10 @range(min: 1, max: 500)
  ): [Festival!]!
    @cypher(statement: "...")
}
```

A value outside the bounds is `BAD_USER_INPUT`, and a default outside its
own bounds is a model error.

## Reads and writes

On `Query` and on object types a statement may not write. The check looks
for `CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE` and `DETACH` outside
strings and comments, at startup.

On `Mutation` a statement may write, with these consequences:

- **The write-set is unknown.** The `onWrite` event is `broad` ("treat
  everything as changed"), and so are subscription events, unless
  `changeFeed: true` feeds them from the engine, which sees the real
  changes.
- **A field returning a `@node` type must return the nodes themselves**
  (`RETURN p`, not a map). They are read again by key in the same
  transaction, so the client's selection and the type's read rules apply.
- **`@uniqueTogether` is checked by scan.** After the statement, every
  `@uniqueTogether` type it might have written (one whose label,
  constrained relationship type or constrained property the text names) is
  checked over all its nodes, in the same transaction. The model warns
  about each such mutation. Prefer a generated mutation for a constrained
  type.
- **Nothing else is validated.** Cardinality, required relationships and
  the authorization rules of the types it writes are the statement's
  responsibility.

With a [`transaction` in the context](/docs/graphql/generated-api#transactions)
a `@cypher` mutation runs in it, and its broad event waits for the commit.

## Filtering and sorting by a computed value

A scalar, non-list `@cypher` field of a `@node` type may take
`@filterable` and `@sortable` when every argument has a default:

```graphql
type Festival @node {
  key: ID! @key
  followerCount: Int!
    @cypher(statement: "RETURN size([(this)<-[:FOLLOWS]-(u:User) | u]) AS n")
    @filterable(byValue: [GTE])
    @sortable
}
```

The statement then runs once per candidate node, before the filter. No
index can help, so this suits small labels or filters that are already
selective. It works in root fields only: used through a relationship
filter or a nested sort it is `BAD_USER_INPUT`. The model warns, so
`lora-graphql check` reports each such field.

## Cost

A `@cypher` field on a node type runs once per parent row: a page of 25
festivals selecting `similar` runs the statement 25 times, inside the one
statement the root field compiles to. That work is **not** counted by
`maxCost`, which only knows the page sizes. Keep such statements
anchored on `this` and bounded with `LIMIT`, and prefer selecting them on
single lookups and small pages.

## What the library checks

Statements are checked, not just trusted to be right:

**At startup**, as model errors:

- every `$parameter` is an argument, `$jwt` or `$viewer`;
- no write clause outside `Mutation`;
- the column can be inferred or `columnName` is set;
- argument and return types are supported;
- `@cypher` is not combined with `@relationship`, `@key`, `@unique`,
  `@alias`, `@index`, `@default`, `@timestamp` or `@groupBy`.

**At startup**, as warnings in `lora.model.warnings`: unused arguments,
`OPTIONAL MATCH`, and field statements that never use `this`.

**In `check()` and `lora-graphql check`**: every statement is planned with
the engine's `explain()`. A syntax error, an unknown function or a missing
column fails CI with the engine's message, not the first request in
production. A label scan inside the statement is reported as a lint note.

## See also

- [Directive reference](/docs/graphql/directives)
- [Authorization](/docs/graphql/authorization)
- [Cypher queries](/docs/queries): the query language itself.
- [The smart layer: @cypher checks](/docs/graphql/smart-layer#s4-cypher-checks)
