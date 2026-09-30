---
slug: loradb-v0-17-inside-lora-graphql
title: "LoraDB v0.17: inside lora-graphql"
description: "How @loradb/lora-graphql turns one annotated schema into a GraphQL API: a validated model, one Cypher statement per root field, authorization folded into the statement, plans checked with explain(), and mutations that know their exact write-set. Plus what else ships in v0.17."
authors: [loradb]
tags: [release-notes, announcement, graphql, architecture, cypher]
image: /img/blog/loradb-v0-17-inside-lora-graphql-header.png
---

![LoraDB v0.17. Inside lora-graphql.](/img/blog/loradb-v0-17-inside-lora-graphql-header.png)

v0.16 introduced `@loradb/lora-graphql`. This post is about how it
works: what happens between an annotated schema and the Cypher that
reaches the engine, and why it is built that way.

The short version: there are no resolvers that walk the graph. A
GraphQL operation is compiled, root field by root field, into one
parameterised Cypher statement whose plan can be checked before it
ever runs.

<!-- truncate -->

## From SDL to a model

Everything starts with type definitions like these:

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Festival
  @node
  @mutation
  @authorization(
    filter: [
      { requireAuthentication: false, where: { node: { published: { eq: true } } } }
      { where: { jwt: { roles: { includes: "editor" } } } }
    ]
  ) {
  key: String! @key
  name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  published: Boolean! @default(value: false)
  capacity: Int @filterable(byValue: [GT, LT])
  followers: [User!]! @relationship(type: "FOLLOWS", direction: IN, properties: "Follows")
}
```

The first thing the library does with them is not build a GraphQL
schema. It builds a **model**: a plain, validated description of
labels, properties, keys, relationships, rules and the API surface.
Every later stage reads the model and nothing else.

Validation happens here, all at once. A rule that names a field the
type does not have, a claim the `@jwt` type does not declare, a
required relationship that no mutation could ever set, or a directive
that means nothing where it is written: each becomes one entry of a
single `ModelError`, located by type and field. Nothing is accepted
and silently ignored. If the model builds, the API it describes is
one the library can actually enforce.

## From the model to a schema

The generated GraphQL schema is derived from the model: object types,
`where` and `sort` inputs with only the operators each field opts into,
connections, aggregates, and the mutation inputs. It is plain
graphql-js, served by Yoga, Apollo or anything else, and it works with
graphql 16 and 17.

Inputs are generated from what the model allows, not from the SDL's
shape. A field that is `@settable(onUpdate: false)` is not in the
update input, so a client that sends it gets a validation error, not
a silent overwrite. An input that would end up with no field is left
out, together with the argument or mutation that would take it, and
`getSchema()` validates the result, so a model that builds never
produces a schema that fails at request time.

The same model also says which **indexes** the API needs. A key needs a
constraint, `GT` and `@sortable` need a RANGE index, `CONTAINS` needs
a TEXT index. `requirements()` lists them with the reason for each, and
`assertSchema({ create: true })` creates them.

## One statement per root field

The resolvers the schema carries do one thing: compile the root field
they sit on and run the result. Take this query:

```graphql
{
  festivals(where: { capacity: { gt: 1000 } }, sort: [{ name: ASC }], limit: 10) {
    key
    name
    followers(limit: 3) { name }
    followersConnection { totalCount }
  }
}
```

For an anonymous request it compiles to one statement (the connection's
bookkeeping fields are abridged here):

```cypher
MATCH (this:Festival)
WHERE this.capacity > $p0 AND this.published = $p1
WITH this ORDER BY this.name ASC LIMIT $p7
CALL {
  WITH this
  MATCH (this)<-[:FOLLOWS]-(this_followers:User)
  WITH this_followers ORDER BY this_followers.key ASC LIMIT $p2
  RETURN collect(this_followers { .name }) AS this_followers_list
}
RETURN this {
  .key, .name,
  followers: this_followers_list,
  followersConnection: {
    __totalCount: size([(this)<-[:FOLLOWS]-(:User) | 1]), …
  }
} AS this
```

A few rules shape every statement the compiler writes:

- **Page first, then project.** The root page is sorted and limited in a
  `WITH` before anything nested runs, so nested work happens for ten
  festivals, not for every one that matched.
- **Nested lists in `CALL { }`.** A subquery per nested list is the one
  way to sort, limit and aggregate per parent.
- **Counts as pattern comprehensions.** `size([(this)<-[:FOLLOWS]-() | 1])`
  instead of `OPTIONAL MATCH`, which multiplies rows.
- **Every value a parameter.** Values never become statement text, and
  identifiers only ever come from the model, escaped. Statement text
  depends on the shape of the request, not on its values, so the
  engine's plan cache is hit for every repeat.
- **Absent filters are absent.** A filter the client did not send is
  left out of the statement, never written as `($p IS NULL OR …)`, so
  the planner sees every predicate that is there.

Before the statement runs, the compiler has also estimated its cost:
rows touched, multiplied through nested page sizes and capped by
`@cardinality` or sampled degrees. An operation over `maxCost` fails
with `COST_EXCEEDED` without touching the database.

## Authorization is compiled, not evaluated

The query above ran anonymously. Here is the same field for a caller
whose token carries the `editor` role:

```cypher
MATCH (this:Festival)
WHERE this.capacity > $p0
WITH this ORDER BY this.name ASC LIMIT $p7
…
```

The `published = true` test is gone. A rule is `{ node, jwt, AND, OR,
NOT }`, and its two halves are treated differently:

- The **`jwt` half** reads only the request's claims, so it is decided
  in JavaScript while compiling. The editor's rule folds to *true*,
  which grants, and the other rule's predicate disappears.
- The **`node` half** becomes a predicate in the statement, with claims
  bound as parameters: `{ node: { owner: { eq: "$jwt.sub" } } }`
  becomes `this.owner = $p0`. From v0.17 a claim can also sit inside a
  longer string, `key: { startsWith: "${jwt.sub}:" }`, which ties a
  client-chosen key to its owner.

Because rules live in the statement, a list, a count, an aggregate, a
nested relationship, a cursor and a subscription event all see the
same rows. There is no second code path that could forget a filter.

Missing claims get care of their own. A test that needs a claim the
token lacks is *unknown*: false where it stands, and a `NOT` around it
cannot turn it into a grant. Claim paths read own properties only, so
`$jwt.constructor` resolves to nothing.

Field-level rules compile per row. A field guarded by a READ rule is
projected as `CASE WHEN <rule> THEN <value> ELSE { __forbidden: true }
END`, and the field's resolver turns the marker into a `FORBIDDEN` (or,
without a token, `UNAUTHENTICATED`) error for that row only. The
statement has the same shape with or without a token, which matters
for the next part.

## Plans are checked, not hoped for

Each compiled statement carries the access path it was written for:
"an exact seek on `:Festival` by key", "a range seek for
`capacity > …`". `lora.explain()` asks the engine for its plan and
compares. A label scan where a seek was expected, a mutating plan
behind a read, or result columns that do not match are findings.

`lora-graphql check` runs this in CI over your operations, on an
in-memory database with the schema asserted:

```sh
lora-graphql check schema.graphql --operations src/graphql \
  --context ctx.json --baseline plans.json --row-budget 5000
```

`--baseline` fails the build when an operation's plan changes, so a
plan regression shows up in review like any other diff. `--context`
compiles operations as a signed-in caller would. Only the root field's
own access path is judged: a scan inside a `@cypher` statement's
subquery is reported as a lint note for its author, not blamed on the
root.

## Mutations know what they wrote

Mutations are keyed. `update`, `delete`, `connect` and `disconnect`
address nodes by `@key`, and bulk mutations resolve their `where` to
keys first, under the same rules, before doing anything. That gives
every mutation an exact **write-set**: which nodes were created,
updated and deleted, and which relationships were connected and
disconnected.

A mutation is planned from its input, then run in one interactive
transaction. The statements are batched by shape, one `UNWIND` for all
rows that look alike:

```cypher
UNWIND $p0 AS row
MATCH (a:Festival) WHERE a.key = row.from
MATCH (b:User) WHERE b.key = row.to
WITH a, b, row, size([(a)<-[:FOLLOWS]-(b) | 1]) > 0 AS existed
MERGE (a)<-[r:FOLLOWS]-(b)
ON CREATE SET r += row.defaults
SET r += row.props
RETURN row.from AS from, row.to AS key, existed
```

That one statement connects any number of pairs. Each seek sits in its
own `MATCH`, so it never depends on the optimizer finding it. `@default`
properties apply only when the relationship is created, and
`existed` tells a new relationship from a re-connected one, for the
counts and for rules that differ between creating and updating a
property.

Then come the checks Cypher cannot express as constraints: a single
relationship holds one node from whichever side it was written, a
required relationship stays set, and `AFTER` rules hold for what was
written. Each is a statement in the same transaction, and a failure
rolls everything back. An identical re-upsert of 200 rows runs six
statements and writes nothing.

The write-set feeds `onWrite` and subscriptions. With
`changeFeed: true`, subscriptions are fed from the engine's committed
change feed instead, so they also see writes made by hand-written
Cypher.

## Compile once, run many

Compiling a nested page takes a fraction of a millisecond, and the
result is cached anyway. The cache key is the field's node in the
parsed document, the variables, the claims, and the values of every
`$context` path the rules actually read. Persisted operations skip
parsing and validation altogether: `lora.persist()` validates them at
startup, and `lora-graphql compile` emits the manifest and TypeScript
types for them at build time.

## Also in v0.17

Most of this release came from running `lora-graphql` under a real
application's whole API. In the library:

- Rules on relationship properties, `@settable` and `@readonly` on
  relationship fields, and `@timestamp` on relationship properties are
  enforced; a re-connect keeps a relationship's properties.
- `${jwt.path}` and `${context.path}` inside rule strings, as above.
- `check()` takes a GraphQL context per operation, and field rules no
  longer stop anonymous plan checks.
- The test suite runs on graphql 16 and 17; `compile()`, `explain()` and
  `check()` read variables correctly on 17.
- Disconnects are batched, generated types merge fields selected twice,
  `@storedAs` scalars keep their SDL description, and `@cypher` column
  inference understands commas inside calls.

In the engine:

- `<`, `<=`, `>` and `>=` compare lists element by element, so
  `[a, b] > $cursor` works. Integers compare exactly above 2^53.
- `first()` is accepted as the standard name for `head()`.
- A `WITH ... WHERE` predicate that uses a projected variable only
  inside a pattern, such as `WITH a WHERE size([(a)<-[:T]-() | 1]) = 0`,
  now runs after the `WITH`. It used to run before it, with `a` unbound,
  and returned no rows.

### Behaviour changes

- An ordering comparison between values that cannot be ordered against
  each other (`1 < 'a'`, a `DATE` against a `DATETIME`) returns `null`,
  as Cypher specifies, instead of `false`. Under `NOT` that is the
  difference between keeping and dropping a row.
- In `lora-graphql`, an anonymous request that selects a field guarded
  by a READ rule gets an `UNAUTHENTICATED` error on that field, with the
  rest of its data, instead of an error for the whole root field.

The [GraphQL docs](/docs/graphql) cover every directive, and the
[translation rules](/docs/graphql/translation-rules) page lists each
Cypher shape the compiler uses and the measurement behind it.
