---
title: GraphQL for LoraDB
sidebar_label: Getting started
description: Install @loradb/lora-graphql, describe the graph and the public API in one annotated SDL, create the indexes it needs, and serve it from any graphql-js server.
unlisted: true
---

# GraphQL for LoraDB

`@loradb/lora-graphql` is a schema-first GraphQL layer for LoraDB. You write
one annotated SDL that describes both the graph (labels, keys,
relationships) and the public API (which types are readable, writable,
filterable and sortable). The library turns it into an executable
`graphql-js` schema whose operations compile to parameterised Cypher
statements.

It also reasons about those statements. It derives the constraints and
indexes they need, checks with `explain()` that the plans use them, bounds
their cost before they run, compiles authorization rules into them, and
reports exactly what every mutation wrote. The
[smart layer](/docs/graphql/smart-layer) page covers each of these.

:::note Scope
The generated API is deliberately small. Reads are on by default; writes
and subscriptions are opt in per type; a field is filterable or sortable
only when you say so. See [what is not supported](#what-is-not-supported)
before you plan a migration.
:::

## Install

```bash
npm install @loradb/lora-graphql @loradb/lora-node graphql
```

- `graphql` (16 or 17) is a peer dependency, so the server and the library
  share one instance.
- `@loradb/lora-node` is released in lockstep: version X.Y.Z of
  `@loradb/lora-graphql` is tested against `@loradb/lora-node` X.Y.Z.
  Upgrade both together.
- Mutations need interactive transactions, which only the Node binding
  has. `@loradb/lora-wasm` can serve reads.

## Describe the graph and the API

```graphql title="schema.graphql"
type Festival @node @mutation @query(aggregate: true) {
  key: ID! @key(generate: true)
  name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  capacity: Int @filterable(byValue: [GTE, LT]) @sortable
  createdAt: DateTime @timestamp(operations: [CREATE])
  genre: Genre @relationship(type: "IN_GENRE", direction: OUT) @filterable
  followers: [User!]!
    @relationship(type: "FOLLOWS", direction: IN, properties: "Follows")
    @filterable
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
```

A few things to notice:

- `@node` makes a type a node label. Its `@key` field is required, unique
  and immutable: it is the address of every lookup, update and delete, the
  last sort key of every connection, and the anchor of every cursor.
- `@mutation` opts the type into generated mutations. `Genre` has none, so
  clients can read genres and connect a festival to an existing one, but
  not create, update or delete genres.
- `@filterable(byValue: [...])` and `@sortable` decide what clients may
  filter and sort by. Nothing else is filterable.
- `@query(aggregate: true)` adds aggregate root fields.

The [directive reference](/docs/graphql/directives) lists every directive,
and [the generated API](/docs/graphql/generated-api) shows what each type
gets.

## Create the schema object

```ts title="server.ts"
import { readFile } from "node:fs/promises";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";

const typeDefs = await readFile("schema.graphql", "utf8");
const db = await createDatabase();

const lora = new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
});
```

`new LoraGraphQL` validates the SDL. An invalid model throws one
`ModelError` that lists every problem, each located by type and field.
`loraDriver(db)` adapts a `Database` from `@loradb/lora-node` or
`@loradb/lora-wasm`.

## Create the indexes the API needs

```ts
const report = await lora.assertSchema({ create: true });
console.log(report.created); // what was missing and is now created
```

The library infers every constraint and index from the API: a node key
constraint for `@key`, a RANGE index for `@sortable` and range filters, a
TEXT index for `CONTAINS`, and so on. `assertSchema()` without options only
reports what is missing; with `create: true` it creates it, idempotently.
Run it at startup, or print the DDL in CI with
`lora-graphql requirements schema.graphql --ddl` and apply it yourself.

## Serve the schema

`lora.getSchema()` returns a standard `GraphQLSchema`, so any `graphql-js`
server can serve it. The request context carries the caller's verified
claims as `jwt`, and an optional `signal` that cancels the request's
statements:

```ts
import { createYoga } from "graphql-yoga";
import { createServer } from "node:http";

const yoga = createYoga({
  schema: lora.getSchema(),
  plugins: [lora.envelopPlugin()], // document guards: depth, aliases, tokens
  context: async ({ request }) => ({
    jwt: await verifiedClaims(request), // your JWT verification
    signal: request.signal,
  }),
});

createServer(yoga).listen(4000);
```

The library never verifies tokens. `verifiedClaims` stands for your own
verification (for example `jose`'s `jwtVerify`): put only verified claims in
the context. The [authorization](/docs/graphql/authorization) page covers
this and the other production settings.

`lora.envelopPlugin()` brings the document guards (depth, aliases, root
fields, lexer tokens, introspection) to GraphQL Yoga and other Envelop
servers. For servers without Envelop, use `lora.validationRules()` and the
exported `parseOptions()`. The package ships runnable
[examples](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples)
for GraphQL Yoga, Apollo Server and graphql-http.

## Run a query

```graphql
{
  festivalsConnection(
    first: 10
    where: { capacity: { gte: 1000 } }
    sort: [{ capacity: DESC }]
  ) {
    totalCount
    edges {
      cursor
      node {
        key
        name
        genre {
          name
        }
      }
    }
    pageInfo {
      hasNextPage
      endCursor
    }
  }
}
```

Every list is bounded (25 by default, at most 100), connections page by
keyset cursors, and the whole root field compiles to one Cypher statement.

## Execute and persisted operations

Besides `getSchema()`, the library can execute documents itself:

```ts
const result = await lora.execute({
  source: "{ festivals(limit: 5) { key name } }",
  variables: {},
  context: { jwt },
});
```

`execute()` applies the document guards, caches parsed and validated
documents by source text, and returns the operation's cost estimate in
`extensions.cost`.

For first-party clients, register the operations they use at startup:

```ts
lora.persist({
  topFestivals: "query ($n: Int) { festivals(limit: $n) { key name } }",
});

await lora.execute({ id: "topFestivals", variables: { n: 5 }, context });
```

`persist()` parses and validates every document once, so a broken one fails
at startup. At request time an id is looked up and executed without parsing
or validation. With `persistedOnly: true`, `execute()` refuses ad hoc
documents altogether.

`lora-graphql compile` does the validation at build time instead: it writes
a `manifest.json` for `lora.loadManifest()` and an `operations.d.ts` with
typed variables and results. See [typed tooling](/docs/graphql/smart-layer#s8-typed-tooling).

Two caveats:

- The Cypher text is not precompiled. It depends on variable values (absent
  filters are left out) and on the caller's claims (claim checks are
  folded into the statement), so it is compiled per request and cached per
  field, variables and claims.
- Servers that parse every request themselves produce new document nodes
  each time and do not benefit from the per-field compile cache. Use
  `execute()`, persisted operations, or a server with a document cache.

## Check it in CI

```bash
lora-graphql check schema.graphql --operations src/operations
```

`check` builds an in-memory database, asserts the schema, plans every
`@cypher` statement and every query in your operation files, and exits
non-zero on any finding: a label scan where a seek was expected, a
statement the engine rejects, a missing index. It needs `@loradb/lora-node`
installed.

## What is not supported

- Offset pagination. Connections use keyset cursors only.
- `update` and `delete` with an optional, unbounded `where`. Bulk writes
  need a `where` and are bounded by `limit`.
- Every operation on every type and every filter on every field. Opt in
  per type and per field.
- JWT verification. Your server verifies the token.
- Federation.
- Subscriptions across processes. Subscriptions and change events see the
  writes made through the library in this process only; `@cypher`
  mutations report a broad change with no write-set, and writes made
  elsewhere (other processes, `db.execute()`, or your own Cypher through
  `tx.execute()`) are not seen. An engine change feed that lifts this is
  in progress; until it ships, plan for in-process events only.

## Next

- [Directive reference](/docs/graphql/directives)
- [The generated API](/docs/graphql/generated-api)
- [The smart layer](/docs/graphql/smart-layer)
- [Authorization](/docs/graphql/authorization)
- [Translation rules](/docs/graphql/translation-rules)
- [Migrating from @neo4j/graphql](/docs/graphql/migrating-from-neo4j)
