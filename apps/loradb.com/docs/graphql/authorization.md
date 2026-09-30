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
| `maxIntrospectionDepth` | 20 | Nesting under `__schema` / `__type` |
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

Clients then send only an id: the key you gave `persist()`, or for a
manifest compiled from `.graphql` files, `<file path>#<OperationName>` (see
[typed tooling](/docs/graphql/smart-layer#s8-typed-tooling)). With
`persistedOnly`, `execute()` answers any ad hoc document with
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
- Run `lora-graphql check` in CI: it fails on a `@mutation` type whose
  writes no rule guards (see [checking access](#checking-access)).

The [threat model](https://github.com/lora-db/lora/blob/main/docs/design/graphql-threat-model.md)
lists what the library trusts and where each check runs.

## Rules

Two directives express access rules, `@authentication` and
`@authorization`, and both compile into the statements rather than running
as resolver middleware. `@viewer`, named rules and schema-wide defaults
help write them.

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

### The caller's own node: @viewer {#viewer}

Mark the claim that identifies the caller with `@viewer(type:, field:)`.
The field must be `@key` or `@unique` on that type, so the claim names at
most one node. The key can then stay a readable slug while the claim is an
identity provider's opaque id:

```graphql
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "subject")
  roles: [String!]
}

type Person @node {
  key: String! @key
  subject: String! @unique
  verified: Boolean!
}
```

Rules can then say:

- `{ node: { isViewer: true } }` on the viewer type, or
  `{ node: { author: { isViewer: true } } }` through a relationship. This
  expands at startup to `{ subject: { eq: "$jwt.sub" } }` and compiles to
  exactly the same statement as writing that by hand.
- `{ viewer: { verified: { eq: true } } }` to test the caller's own node,
  with one seek by the claim.

Both are resolved from the verified claim, never from anything the client
sends. Without the claim both are unknown, so a `NOT` over `isViewer`
never grants a signed-out caller.
`isViewer` takes `true` only; use `NOT` for the opposite.

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

On a relationship field, `@authentication` guards writes through the
field too: `CREATE` and `UPDATE` cover setting it in a create or update
input, `CREATE_RELATIONSHIP` a `connect` or nested `create` through it, and
`DELETE_RELATIONSHIP` a `disconnect` or nested `delete`.

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

Rule defaults:

| Rule | `operations` | `when` | `requireAuthentication` |
| --- | --- | --- | --- |
| `filter` | `[READ, UPDATE, DELETE]` | | `true` |
| `validate` | `[READ, CREATE, UPDATE, DELETE]` | `[BEFORE, AFTER]` | `true` |

A rule with `requireAuthentication: true` does not grant anything to an
anonymous request.

A rule's `where` is `{ node, jwt, viewer, rule, AND, OR, NOT }`:

- `node` is a filter over the type. `"$jwt.path"` strings become the
  caller's claims, and `"$context.path"` strings values from the GraphQL
  context. Inside a longer string, write `${jwt.path}` or
  `${context.path}`: `key: { startsWith: "${jwt.sub}:" }` confines a user
  to keys that begin with their `sub` and `:`. The claim must be a string,
  number or boolean, otherwise the rule denies. Pick a separator no `sub`
  contains: with `-`, user `a` could take `a-b-...`, the key space of user
  `a-b`. [Owner-scoped keys](#owner-scoped-keys) do this check for you.
- `node` reaches relationship properties through
  `<field>Connection: { some: { node, edge } }`, where `node` filters the
  related node and `edge` the relationship's properties:

  ```graphql
  @authorization(
    filter: [
      {
        where: {
          node: {
            membersConnection: {
              some: { node: { isViewer: true }, edge: { role: { eq: "admin" } } }
            }
          }
        }
      }
    ]
  )
  ```

- `viewer` tests the caller's own node, and `isViewer` inside `node`
  matches it (see [@viewer](#viewer)).
- `rule` names a rule defined once (see [named rules](#named-rules)).
- `jwt` tests claims with `eq`, `in`, `includes`, `contains`, `startsWith`,
  `endsWith`, `lt`, `lte`, `gt`, `gte` and `exists`.
- `node` accepts the operators `eq`, `in`, `lt`, `lte`, `gt`, `gte`,
  `contains`, `startsWith`, `endsWith`, `withinBBox` and `distance` on
  fields, whether or not they are `@filterable`, and `some`, `all`,
  `none`, `single` and `count` on list relationships (a single
  relationship takes the target's filter directly). `@cypher` and
  `@customResolver` fields cannot be used in rules.

#### Filter rules hide nodes

`filter` rules (any passing rule grants) make other nodes invisible: in
lists, lookups, counts, aggregates, search, nested relationships,
relationship filters, subscriptions, and as targets of updates, deletes and
connects. A node the same mutation creates is not hidden from its own
connects, so a filter that depends on the new relationship does not block
a nested create. Filter rules for `CREATE_RELATIONSHIP` and `DELETE_RELATIONSHIP`
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
  rule. A write is what the input sets: a create that leaves the field out
  is not checked against it, even when `@default` or `@populatedBy` fills
  it.
- Field-level `@authentication` also guards filtering, sorting and
  aggregating on the field.
- Relationship and `@cypher` fields take field-level `@authorization` with
  READ validate rules: a row failing the rule reads the field as
  `FORBIDDEN`, and filtering through the field applies the rule too.
  Relationship fields also take rules for the relationship's own
  operations: see [rules on relationship fields](#relationship-rules).
- Reading a guarded field is checked per row, token or not: without the
  token a rule needs, each row reads the field as `UNAUTHENTICATED`. The
  statement has the same shape either way, so `compile()`, `explain()`,
  `check()` and `expectSeeks` plan-check such operations without a token.
  Pass `context` (per operation in `check({ operations })`) to compile
  them as a signed-in caller.

### Field masks {#field-masks}

A field-level READ rule fails the row. A mask substitutes a value
instead:

```graphql
type ConnectionRequest @node {
  key: String! @key
  to: Person! @relationship(type: "TO", direction: OUT)
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
}

type Person @node {
  key: String! @key
  lastSeenAt: DateTime @authorization(mask: [{ unless: { node: { isViewer: true } } }])
}
```

- A row failing `unless` reads the field as `value`. Left out, `value` is
  `null`, which a non-null field refuses. `value` is type-checked against
  the field.
- Filters compare the value the reader sees, so a mask never leaks
  through a filter. Such a filter cannot use the field's index.
- Rules, `onWrite` and the change feed see the stored value.
- Sorting, grouping and aggregating by a masked field are refused unless
  the claims settle the mask, so row order cannot hint at hidden values.
- Masks sit on scalar fields of `@node` types, other than the `@key`.

### Named rules {#named-rules}

Define a rule once and use it as `{ rule: "name" }` wherever a rule part
may stand:

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
  @authorization(filter: [{ where: { OR: [{ rule: "member" }, { rule: "admin" }] } }]) {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN)
}

type PackingItem
  @node
  @authorization(
    filter: [{ where: { OR: [{ node: { trip: { rule: "member" } } }, { rule: "admin" }] } }]
  ) {
  key: String! @key
  trip: Trip! @relationship(type: "PACKED_FOR", direction: OUT)
}
```

- Schema rules (`@authorizationRules` on `extend schema`) test claims
  only.
- Type rules (`@authorizationRule`, repeatable on a `@node` type) are
  found first, then the schema's.
- Inside a node filter (`trip: { rule: "member" }`), the name is a rule of
  that node's type, and that rule must test `node` only.
- Rules are inlined at startup, so they compile to exactly the
  hand-written statement.
- An unknown name, a type rule shadowing a schema rule, and a cycle
  (reported with its chain) are model errors.
- `bypass` and `mutations` in `@authorizationDefaults` may name rules too.

### Schema-wide defaults and bypass {#defaults}

```graphql
extend schema
  @authorizationDefaults(
    bypass: { jwt: { roles: { includes: "admin" } } }
    mutations: { jwt: { roles: { includes: "editor" } } }
  )
```

- A request passing `bypass` skips every filter and validate rule: type,
  field, mask, relationship and relationship property rules included.
  `@authentication` still applies.
- `bypass` tests claims only (`jwt`, `AND`, `OR`, `NOT`, or a named claims
  rule), so it is decided from the verified token before the statement is
  built. An admin's statement carries no rule predicate.
- A type keeps its rules for everyone with `@authorization(bypass: false)`.
- `mutations` is the write rule (`CREATE`, `UPDATE`, `DELETE`) of every
  `@mutation` type that declares no rule for those operations. A type's own
  rules replace it, they never merge with it.
- `check()` fails on a `@mutation` type whose generated writes nothing
  guards: no `@authentication` or `@authorization` rule for them and no
  `mutations` default. Declare writes every caller may make with
  `@authorization(public: [CREATE, ...])`.

### Rules on relationship fields {#relationship-rules}

A relationship field also takes validate rules for the relationship's own
operations, where both ends are known:

```graphql
type Trip @node @mutation {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]!
    @relationship(type: "MEMBER", direction: IN, properties: "TripInvite")
    @authorization(
      validate: [
        # the owner invites
        { operations: [CONNECT], where: { source: { owner: { isViewer: true } } } }
        # the owner removes anyone; a member removes only themselves
        {
          operations: [DISCONNECT]
          where: {
            OR: [{ source: { owner: { isViewer: true } } }, { target: { isViewer: true } }]
          }
        }
        # only the member answers their own invitation, and reads its marker
        { operations: [UPDATE_EDGE, READ_EDGE], where: { target: { isViewer: true } } }
      ]
    )
}
```

- `source` is the node declaring the field, `target` the related node and
  `edge` the relationship's properties. `jwt`, `viewer`, `rule`, `AND`,
  `OR` and `NOT` work as in any rule. A `node` part is a model error here,
  and so is a relationship operation on a type or scalar field, or in the
  same rule as `READ`.
- `CONNECT` covers a new relationship (connect, nested create).
  `DISCONNECT` covers a disconnect, including replacing a single
  relationship. `UPDATE_EDGE` covers `update: [{ edge }]` and a re-connect
  that sets properties. `READ_EDGE` covers reading the properties.
- `CONNECT` and `UPDATE_EDGE` are checked on the relationship after the
  write, `DISCONNECT` before it, in the mutation's transaction. A failure
  is `FORBIDDEN` and rolls the mutation back.
- The rules hold whichever side the write comes from: a connect through
  `Person.trips` (the same relationship type, the other direction) answers
  to `Trip.members`' rules, with source and target as declared on
  `Trip.members`. If both fields carry rules, both apply.
- A relationship failing `READ_EDGE` reads its properties as `FORBIDDEN`.
  Unless the claims alone settle the rule, nothing may filter, sort or
  aggregate by that relationship's properties, so a filter cannot probe a
  hidden value.
- Deleting a node removes its relationships without `DISCONNECT` rules:
  who may delete the node is the type's `DELETE` rule.

### Relationship properties

Fields of a `@relationshipProperties` type take field-level
`@authentication` and `@authorization(validate:)` for `READ`, `CREATE` and
`UPDATE`:

```graphql
type Membership @relationshipProperties {
  role: String
    @authorization(
      validate: [
        { operations: [CREATE, UPDATE], where: { jwt: { roles: { includes: "admin" } } } }
      ]
    )
  joinedAt: DateTime! @settable(onUpdate: false)
}
```

- One `@relationshipProperties` type can serve fields on both ends, so
  these property rules test claims (`jwt`) only, and a `node` part is a
  model error. To test the ends of the relationship, use `UPDATE_EDGE` and
  `READ_EDGE` [on the relationship field](#relationship-rules).
- Setting the property on connect or nested create checks `CREATE` for a
  new relationship and `UPDATE` for one that already exists.
  `update: { edge }` checks `UPDATE`.
- A request the READ rules refuse reads the property as `FORBIDDEN` and
  cannot filter, sort or aggregate by it.
- `@settable(onUpdate: false)` on a property also refuses a re-connect that
  would change it, not only an edge update.

### Owner-scoped keys {#owner-scoped-keys}

```graphql
type Trip @node @mutation {
  key: String! @key(scope: VIEWER, separator: ":")
}
```

`@key(scope: VIEWER)` keeps created keys in the caller's key space. A
create, nested creates and upsert-creates included, must use a key that
starts with the [`@viewer`](#viewer) claim and the separator (default
`:`), and is longer than that prefix: `lou:tomorrowland` for the caller
whose claim is `lou`.

- The check runs in JavaScript before any statement, so `FORBIDDEN` for
  `bob:x` reads the same whether `bob:x` exists or not.
- A claim containing the separator is refused, so user `a` cannot write
  into the key space of user `a:b`.
- It needs `@viewer`. The schema's `bypass` skips it.
- Keys shared by two owners (`f1:lou`) stay hand-written rules.

### Checking access {#checking-access}

- `lora-graphql access` (or `lora.accessMatrix()`) prints who may do what:
  for every type and guarded field, each operation as each kind of caller
  (anonymous, authenticated, and each role the rules test, such as
  `roles:admin`), with the verdict (`allowed`, `filtered`, `validated`,
  `masked`, `denied`, `unauthenticated`) and the rules that decide it. The
  order is stable, so a snapshot in CI turns access changes into diffs.
  See [the CLI](/docs/graphql/cli).
- `lora-graphql check` fails on unguarded `@mutation` writes (see
  [defaults](#defaults)) and lints authorization: a filter rule every
  signed-in caller passes, a rule whose default `requireAuthentication`
  refuses anonymous callers a branch that needs no claims, and field rules
  the schema's bypass skips.
- `expectAccess` probes the database as a given caller, in transactions
  that are rolled back: `"read Trip lou:tomorrowland"`,
  `"connect Trip.members lou:tomorrowland → f1"`, and so on. See
  [testing](/docs/graphql/testing).

### What rules do not cover

- `@cypher` statements. Rules on the owning type do not reach inside them;
  a `@cypher` field returning `@node` types applies the target type's read
  filters to the nodes it returns, but the statement itself runs with full
  access.
- Cypher you run yourself with `tx.execute`, including relationship
  writes that relationship rules would otherwise check.
- `onWrite` and `changes()`. They see every committed write.
- Deleted nodes in subscriptions. A deletion cannot be checked after the
  fact, so it goes only to subscribers following that `key` without a
  `where`.
