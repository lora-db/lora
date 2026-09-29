---
title: GraphQL Authorization and Security
sidebar_label: Authorization
description: Production security for @loradb/lora-graphql (verified JWTs, masked errors, document guards, persisted operations, signed cursors) and the @authentication and @authorization rules compiled into every statement.
---

# Authorization and security

Read the first half of this page before you deploy. The rules in the
second half are only as good as the claims you give them.

## Before you deploy

:::warning The library does not verify tokens
Your server verifies the JWT (signature, issuer, audience, expiry) and
puts only the verified claims in the GraphQL context as `jwt`. A server
that copies unverified claims into the context grants whatever those
claims say.
:::

### Verify the JWT in the server

```ts
import { jwtVerify } from "jose";

const secret = new TextEncoder().encode(process.env.JWT_SECRET);

async function verifiedClaims(request: Request) {
  const header = request.headers.get("authorization");
  if (!header?.startsWith("Bearer ")) return undefined; // anonymous
  const { payload } = await jwtVerify(header.slice(7), secret, {
    issuer: "https://auth.example.com/",
    audience: "festivals-api",
  });
  return payload;
}
```

A request without `jwt` in the context is unauthenticated. If your claims
live somewhere else in the context, pass `jwt: (context) => claims` to
`new LoraGraphQL`.

### Mask database errors {#mask-database-errors}

Database errors can name labels, properties and Cypher. With `maskErrors`
the client gets `extensions.code: "DATABASE_ERROR"` and an `id`, and your
`onError` callback gets the detail under the same `id`:

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  maskErrors: true, // default when NODE_ENV is "production"
  onError: ({ id, field, message }) => log.error({ id, field, message }),
});
```

Errors the library writes itself (`FORBIDDEN`, `NOT_FOUND`,
`CONSTRAINT_VIOLATION`) stay readable, without the engine error they were
mapped from.

### Keep the document guards on

`maxCost` bounds the rows an operation touches. The document guards bound
the document before that:

| Guard | Default | Limit |
| --- | --- | --- |
| `maxDepth` | 12 | Field nesting, through fragments |
| `maxAliases` | 30 | Aliased fields per document |
| `maxRootFields` | 20 | Root fields per operation |
| `maxTokens` | 5000 | Lexer tokens per document, checked while parsing |
| `introspection` | production | Off when `NODE_ENV` is `production` |

`execute()` and `persist()` apply them. Other servers need them wired in:

- GraphQL Yoga and other Envelop servers: `plugins: [lora.envelopPlugin()]`.
- Apollo Server: `validationRules: lora.validationRules()` and
  `parseOptions: parseOptions(guards)`.
- graphql-http: `validationRules: lora.validationRules()` (appended to the
  standard rules) and `parse: (source) => parse(source, parseOptions(guards))`.
- Plain `graphql-js`: validate with `[...specifiedRules, ...lora.validationRules()]`
  and parse with `parseOptions(guards)`.

`guards` is the object you pass to `new LoraGraphQL({ guards })`, or
nothing for the defaults.

Hiding introspection is not access control: the public schema is whatever
the SDL exposes.

### Prefer persisted operations

For first-party clients, register the operations they use and refuse
everything else:

```ts
const lora = new LoraGraphQL({ typeDefs, driver, persistedOnly: true });
lora.loadManifest(manifest); // from lora-graphql compile, or lora.persist({...})
```

With `persistedOnly`, `execute()` answers any ad hoc document with
`PERSISTED_QUERY_ONLY`. This applies to `execute()`: if you serve
`getSchema()` through another server, that server decides what it
accepts.

### Sign cursors

Without `cursorSecret`, cursors are tagged with their sort but not signed.
A client can craft one, but it only moves the start of a page within rows
the caller may read (sorting by fields with row rules is refused). Set
`cursorSecret` from a secret store when cursors must not be forgeable:
cursors then carry an HMAC-SHA-256 signature, and any cursor the server did
not issue is rejected.

### Bound the work

| Option | Default | Bounds |
| --- | --- | --- |
| `maxCost` | 50 000 | Estimated rows per operation, checked before it runs |
| `budget(context)` | | The same limit, per request |
| `timeoutMs` | 10 000 | Every statement; a `signal` in the context cancels |
| `maxBatch` | 1000 | Nodes one mutation creates or deletes; bulk `limit` |
| `maxLimit` | 100 | Page size; more is `LIMIT_EXCEEDED`, not a clamp |
| `maxQueuedChanges` | 1000 | How far a subscriber may fall behind |

### Checklist

- Verify the JWT in the server and put only verified claims in the
  context.
- Run with `NODE_ENV=production`, or set `maskErrors` and
  `guards.introspection` explicitly.
- Wire the document guards into your server.
- Prefer `persistedOnly` for first-party clients.
- Set `cursorSecret` from a secret store.
- Cache responses per user: rules make results differ per caller.
- Review every `@cypher` statement as trusted code. It runs with the
  database's full access, and rules on the owning type do not reach inside
  it.
- Treat `onWrite` and `changes()` as server-side hooks: they carry the full
  write-set without read rules.

The [threat model](https://github.com/lora-db/lora/blob/main/docs/design/graphql-threat-model.md)
lists what the library trusts and where each check runs.

## Rules

Two directives express access rules, and both compile into the statements
rather than running as resolver middleware.

### Declare the claims

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!] @jwtClaim(path: "app_metadata.roles")
}
```

With a `@jwt` type, rules may only use declared claims, and an unknown
claim is a startup error. `@jwtClaim(path:)` maps a claim to where it lives
in the token.

### @authentication

`@authentication(operations:, jwt:)` requires an authenticated request for
the listed operations, and optionally claims that satisfy `jwt`:

```graphql
type Report @node @authentication(operations: [READ], jwt: { roles: { includes: "analyst" } }) {
  key: String! @key
}
```

Operations are `READ`, `CREATE`, `UPDATE`, `DELETE`, `CREATE_RELATIONSHIP`,
`DELETE_RELATIONSHIP` and `SUBSCRIBE`. A failure is `UNAUTHENTICATED` or
`FORBIDDEN`.

### @authorization

```graphql
type Post
  @node
  @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE])
  @authorization(
    filter: [
      { where: { node: { published: { eq: true } } }, requireAuthentication: false }
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
  author: User! @relationship(type: "WROTE", direction: IN)
}
```

A rule is `{ node, jwt, AND, OR, NOT }`:

- `node` is a filter over the type. `"$jwt.path"` strings become the
  caller's claims, and `"$context.path"` strings values from the GraphQL
  context.
- `jwt` tests claims with `eq`, `in`, `includes`, `contains`, `startsWith`,
  `endsWith`, `lt`, `lte`, `gt`, `gte` and `exists`.

#### Filter rules hide nodes

`filter` rules (any passing rule grants) make other nodes invisible: in
lists, lookups, counts, aggregates, search, nested relationships,
relationship filters, subscriptions, and as targets of updates, deletes and
connects. Filter rules for `CREATE_RELATIONSHIP` and `DELETE_RELATIONSHIP`
guard both ends of connects and disconnects. `requireAuthentication: false`
lets a rule apply to anonymous requests.

#### Validate rules fail the request

`validate` rules fail the request with `FORBIDDEN`:

- `BEFORE` an update or delete;
- `AFTER` a create or update, rolling it back;
- for `READ`, on any returned node, and on cursors, counts and aggregates
  that cover one.

They do not hide nodes from filters. Use `filter` rules for that.

#### How rules are evaluated

- **Claim tests run in JavaScript at compile time.** An admin's statement
  carries no filter at all, so statements stay specialised and
  index-friendly. Node conditions are compiled into the statement and run
  in the database.
- **A test that needs a claim or context value the request lacks is
  unknown.** It is false where it stands, and a `NOT` over it is false too,
  so negation can never turn a missing claim into a grant.
- Claim and context paths read own properties only, so `constructor`,
  `__proto__` and other inherited names resolve to nothing.
- Node conditions follow Cypher: `NOT` over a null property is not true.
- Rules are checked against the model at startup. An unknown field,
  operator or declared claim, or a test that is empty or null, is an error,
  not an open door.

### Field-level rules

```graphql
type Post @node {
  key: ID! @key
  notes: String @authentication(operations: [READ])
  royalties: Int
    @authorization(
      validate: [
        { operations: [READ], where: { node: { author: { key: { eq: "$jwt.sub" } } } } }
      ]
    )
  author: User! @relationship(type: "WROTE", direction: IN)
}
```

- Field-level `@authorization(validate:)` guards one field. Reading it on a
  row that fails is `FORBIDDEN`, filtering by it only matches rows that
  pass, sorting or aggregating by it is refused, and writing it checks the
  rule.
- Field-level `@authentication` also guards filtering, sorting and
  aggregating on the field.
- Relationship and `@cypher` fields take field-level `@authorization` with
  READ validate rules: a row failing the rule reads the field as
  `FORBIDDEN`, and filtering through the field applies the rule too.

### What rules do not cover

- `@cypher` statements. Rules on the owning type do not reach inside them;
  a `@cypher` field returning `@node` types applies the target type's read
  filters to the nodes it returns, but the statement itself runs with full
  access.
- `onWrite` and `changes()`. They see every committed write.
- Deleted nodes in subscriptions. A deletion cannot be checked after the
  fact, so it goes only to subscribers following that `key` without a
  `where`.
