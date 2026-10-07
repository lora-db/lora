---
title: GraphQL Authorization and Security
sidebar_label: Authorization
description: Production security for @loradb/lora-graphql (verified JWTs, masked errors, document guards, persisted operations, signed cursors) and the @authentication and @authorization rules compiled into every statement.
---

# Authorization and security

This page is the reference: the production settings every deployment
needs, then every rule, operator and default. Two companion pages show
them at work:

- [Authentication](/docs/graphql/authentication) covers verifying tokens
  in your server and requiring a signed-in caller.
- [Authorization recipes](/docs/graphql/authorization-recipes) has
  complete schemas for common access models (private records, public
  reads, roles, tenants, teams, private fields), each with the real
  response every caller gets.

Read the first half of this page before you deploy. The rules in the
second half are only as good as the claims you give them.

## Which rule does what

| You want | Use | A caller who fails gets |
| --- | --- | --- |
| A signed-in caller, or one with a role, for a whole operation | [`@authentication`](#authentication) | `UNAUTHENTICATED` |
| Rows the caller should not know exist | [`filter`](#filter-rules-hide-nodes) | Nothing: the rows are absent |
| A write that must satisfy a condition | [`validate`](#validate-rules-fail-the-request) | `FORBIDDEN`, and the write is rolled back |
| One field refused on some rows | [Field-level `validate`](#field-level-rules) | `FORBIDDEN` on that field |
| One field blanked on some rows | [`mask`](#field-masks) | A substitute value, no error |
| Control over who links what | [Rules on the relationship field](#relationship-rules) | `FORBIDDEN` |
| A role that skips every rule | [`bypass`](#defaults) | |
| A write rule for every type at once | [`mutations`](#defaults) | `FORBIDDEN` |

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

[Authentication](/docs/graphql/authentication#step-1-verify-the-token-in-your-server)
has the same for a remote key set, the context function for each server,
and what to do with an expired token.

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
| `timeoutMs` | 10 000 | Every statement, and a write's wait for the writer lock; a `signal` in the context cancels |
| `operationTimeoutMs` | 2 × `timeoutMs` | All root fields of one query together; past it they fail with `TIMEOUT` |
| `maxConcurrentStatements` | 2 | Statements one operation runs at once, so aliased root fields cannot take every worker |
| `maxBatch` | 1000 | Nodes one mutation creates, updates or deletes, and ten times as many relationships; bulk `limit` |
| `maxLimit` | 100 | Page size; more is `LIMIT_EXCEEDED`, not a clamp |
| `maxFilterDepth` | 2 | Relationship levels in one `where` |
| `maxListFilter`, `maxStringFilter` | 1000, 10 000 | Items in an `in` list, characters in a string operand |
| `maxListArgument` | 1000 | Items in a list argument of a `@cypher` field |
| `maxSubscriptions` | 100 | Live subscriptions per connection or user |
| `maxQueuedChanges` | 1000 | How far a subscriber may fall behind |

The estimate behind `maxCost` assumes full pages until you give it better
numbers. Run `lora.analyze()` in production, or declare `@cardinality`, and
lower `maxCost` on public endpoints. See
[limits and scaling](/docs/graphql/limitations).

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
  exactly that statement. Once `@viewer` is declared, writing the
  expansion out by hand is a model error: the mapping lives in one place.
- `{ viewer: { verified: { eq: true } } }` to test the caller's own node,
  with one seek by the claim.

Both are resolved from the verified claim, never from anything the client
sends. Without the claim both are unknown, so a `NOT` over `isViewer`
never grants a signed-out caller.
`isViewer` takes `true` only; use `NOT` for the opposite. It works
through relationships and union members
(`author: { Person: { isViewer: true } }`). In a filter over an interface
it is a model error, since there is no single type to expand against.

In rule strings, `"${viewer.key}"` (any scalar field of the viewer type)
is the caller's own value: the claim itself for the field `@viewer` maps
to, otherwise read in the statement with one seek by the claim. It lets
keys built from the caller's key work while the claim is an opaque
subject:

```graphql
# one per person
key: { eq: "${viewer.key}" }
# a pair key ending in the caller's key
key: { endsWith: ":${viewer.key}" }
```

It stands for one value, not inside a list, and is unknown without the
claim.

### @authentication

`@authentication(operations:, jwt:)` requires an authenticated request for
the listed operations, and optionally claims that satisfy `jwt`:

```graphql
type Report
  @node
  @authentication(
    operations: [READ]
    jwt: { roles: { includes: "analyst" } }
  ) {
  key: String! @key
}
```

Operations are `READ`, `CREATE`, `UPDATE`, `DELETE`, `CREATE_RELATIONSHIP`,
`DELETE_RELATIONSHIP` and `SUBSCRIBE`; without `operations`, all seven. A
failure is `UNAUTHENTICATED`, both for a request without claims and for
one whose claims do not satisfy `jwt`.

On a relationship field, `@authentication` guards writes through the
field too: `CREATE` and `UPDATE` cover setting it in a create or update
input, `CREATE_RELATIONSHIP` a `connect` or nested `create` through it, and
`DELETE_RELATIONSHIP` a `disconnect` or nested `delete`.

[Authentication](/docs/graphql/authentication#step-3-require-a-caller)
shows each placement with the response an anonymous and a signed-in
caller get.

### @authorization

```graphql
extend schema @authorizationDefaults(requireAuthentication: false)

type Post
  @node
  @mutation
  @authentication(operations: [CREATE, UPDATE, DELETE])
  @authorization(
    filter: [
      { where: { node: { published: { eq: true } } } }
      { where: { node: { author: { key: { eq: "$jwt.sub" } } } } }
      {
        where: {
          AND: [
            { jwt: { sub: { exists: true } } }
            { node: { tenant: { eq: "$context.tenant" } } }
          ]
        }
      }
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

That schema combines four access paths in one type. The
[recipes](/docs/graphql/authorization-recipes) take them one at a time:
[private records](/docs/graphql/authorization-recipes#private-records),
[public read, owner write](/docs/graphql/authorization-recipes#public-read-owner-write),
[roles](/docs/graphql/authorization-recipes#roles) and
[tenants](/docs/graphql/authorization-recipes#multi-tenant).

Rule defaults:

| Rule | `operations` | `when` |
| --- | --- | --- |
| `filter` | `[READ, UPDATE, DELETE]` | |
| `validate` | `[READ, CREATE, UPDATE, DELETE]` | `[BEFORE, AFTER]` |

A rule takes `operations`, `when` (validate rules) and `where`. Any other
field is a model error, so a misspelt `operations` cannot fall back to the
default unnoticed.

#### Anonymous callers {#anonymous-callers}

Without a token every rule denies. That is the default, and it is one
setting for the whole schema:
`@authorizationDefaults(requireAuthentication: false)`. With `false`:

- a rule with a branch that reads no claims decides that branch for
  anonymous callers too (the published posts above);
- a validate rule that needs claims still asks for a token, so an
  anonymous caller gets `UNAUTHENTICATED`, not `FORBIDDEN`;
- a READ filter that needs claims admits no row for them: they read an
  empty list, and the access matrix calls it `denied`;
- a claim read without a token denies its branch, also under `NOT`.

The setting reaches every rule, writes included: a CREATE, UPDATE, DELETE
or CONNECT rule that reads no claims decides for signed-out callers too.
`lora-graphql check` names each such write rule that no `@authentication`
covers.

To keep a claim-free rule or branch for signed-in callers only, test a
claim beside it, as the tenant rule above does with
`{ jwt: { sub: { exists: true } } }`, or put `@authentication` on the type
or on the relationship field.
`requireAuthentication` is not a rule's field: on a rule it is a model
error, like any field a rule does not define.

A rule's `where` is `{ node, jwt, viewer, rule, AND, OR, NOT }`:

- `node` is a filter over the type. `"$jwt.path"` strings become the
  caller's claims, and `"$context.path"` strings values from the GraphQL
  context. Inside a longer string, write `${jwt.path}` or
  `${context.path}`: `key: { startsWith: "${jwt.sub}:" }` confines a user
  to keys that begin with their `sub` and `:`. The claim must be a string,
  number or boolean, otherwise the rule denies. Pick a separator no `sub`
  contains: with `-`, user `a` could take `a-b-...`, the key space of user
  `a-b`. [Owner-scoped keys](#owner-scoped-keys) do this check for you.
- A whole string starting with `$` must be a placeholder (`$jwt.<claim>`,
  `$context.<path>`). A misspelt one, such as `"$jtw.sub"`, is a model
  error rather than a literal that silently stops matching. Write a
  literal `$...` as `"\\$..."`.
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
guard both ends of connects and disconnects. Whether a rule applies to
anonymous requests is the schema's setting: see
[anonymous callers](#anonymous-callers).

#### Validate rules fail the request

`validate` rules fail the request with `FORBIDDEN`:

- `BEFORE` an update or delete;
- `AFTER` a create or update, rolling it back;
- for `READ`, on any returned node, and on cursors, counts and aggregates
  that cover one.

They do not hide nodes from filters. Use `filter` rules for that.

A common pairing is a `filter` for reads, updates and deletes plus a
`validate` for `CREATE`, where there is no row to filter yet. Without the
second, a caller could create a row they would then not be allowed to
see, such as a note owned by someone else. The
[private records recipe](/docs/graphql/authorization-recipes#private-records)
shows both, and what each refusal looks like to the client.

#### How rules are evaluated

- **Claim tests run in JavaScript at compile time.** An admin's statement
  carries no filter at all, so statements stay specialised and
  index-friendly. Node conditions are compiled into the statement and run
  in the database.
- **The rule's own node.** `"${node.path}"` in a `node` part reads the
  node the rule is about, so a rule can relate two of its paths. "The
  request's recipient is in the conversation it gates" is
  `{ node: { conversation: { participants: { some: { key: { eq: "${node.to.key}" } } } } } }`.
  The path steps through single relationships only and ends on a scalar
  field; the value is read in the statement and stands for one value, not
  inside a list. In a relationship field's rules, `${source.path}`,
  `${target.path}` and `${edge.property}` read its ends and properties the
  same way.
- **Claim operators.** `eq`, `in` and `includes` compare structurally.
  `lt`, `lte`, `gt` and `gte` only match number claims, and `contains`,
  `startsWith` and `endsWith` only string claims. `exists` is the only
  test that can decide on an absent claim, and even `exists: false` denies
  a request with no token at all. A `jwt` test compares with literals: a
  `${...}` placeholder in its operand is a model error.
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
        {
          operations: [READ]
          where: { node: { author: { key: { eq: "$jwt.sub" } } } }
        }
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
  lastSeenAt: DateTime
    @authorization(mask: [{ unless: { node: { isViewer: true } } }])
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

The [private fields recipe](/docs/graphql/authorization-recipes#private-fields)
puts a mask and a field rule side by side, with the response each gives.

### Named rules {#named-rules}

Define a rule once and use it as `{ rule: "name" }` wherever a rule part
may stand:

```graphql
extend schema
  @authorizationRules(
    rules: [
      { name: "admin", where: { jwt: { roles: { includes: "admin" } } } }
    ]
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
  ) {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN)
}

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

The [teams recipe](/docs/graphql/authorization-recipes#teams-and-membership)
uses a named rule from two types.

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
  `check` lists, in one line, every type with rules that says neither
  `bypass: false` nor `bypass: true`, and the relationship property types
  whose rules the bypass skips (they cannot opt out), so a new type does
  not join the bypass unnoticed.
- `requireAuthentication` (default `true`) decides whether rules apply to
  anonymous requests: see [anonymous callers](#anonymous-callers).
- `mutations` is the write rule (`CREATE`, `UPDATE`, `DELETE`) of every
  `@mutation` type, per operation: it guards each of those operations that
  none of the type's own rules covers. A `validate` rule covers the
  operations it lists. A `filter` rule covers `UPDATE` and `DELETE` when it
  lists them (by default it does), never `CREATE`, since there is no node
  to filter before it exists. So a type with only a `filter` rule keeps the
  default on `CREATE`, and one with a `validate` rule for `UPDATE` keeps it
  on `CREATE` and `DELETE`. Where a type's rule covers an operation it
  replaces the default for that operation; they never merge.
- The bypass also skips rules on root `@cypher` fields, which cannot opt
  out.
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
  `Trip.members`. Declare each operation's rules on one of the two
  fields: the same operation ruled on both is a model error, since both
  would have to pass without either saying so. Where two tests must both
  hold, combine them with `AND` in one rule. Different operations may sit
  on different sides.
- A relationship failing `READ_EDGE` reads its properties as `FORBIDDEN`.
  Unless the claims alone settle the rule, nothing may filter, sort or
  aggregate by that relationship's properties, so a filter cannot probe a
  hidden value.
- Deleting a node removes its relationships without `DISCONNECT` rules:
  who may delete the node is the type's `DELETE` rule.
- A connect or disconnect goes through an update of the declaring node,
  so the caller must first pass that type's `UPDATE` filter. A member who
  may remove themselves from a project has to be admitted by the
  project's `UPDATE` filter; narrow what else they can change with
  field-level rules. The
  [teams recipe](/docs/graphql/authorization-recipes#teams-and-membership)
  works through exactly this.

### Relationship properties

Fields of a `@relationshipProperties` type take field-level
`@authentication` and `@authorization(validate:)` for `READ`, `CREATE` and
`UPDATE`:

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
  joinedAt: DateTime! @settable(onUpdate: false)
}
```

- One `@relationshipProperties` type can serve fields on both ends, so
  `CREATE` and `UPDATE` rules on a property test claims (`jwt`) only, and
  a `node` part is a model error. To test the ends of the relationship on
  a write, use `UPDATE_EDGE` [on the relationship field](#relationship-rules).
- `READ` rules may also test the relationship's `source`, `target` and
  `edge`, and `viewer`, when every relationship field that uses the
  properties type declares the same two node types. They are decided per
  relationship: below, every member reads `rsvp`, and only the member
  themselves reads their own `lastReadAt`. A relationship that fails reads
  that property as `FORBIDDEN` while the others still read.

  ```graphql
  type Membership @relationshipProperties {
    rsvp: String
    lastReadAt: String
      @authorization(
        validate: [{ operations: [READ], where: { target: { isViewer: true } } }]
      )
  }
  ```
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
- When `@viewer` maps to a field other than the key (an opaque subject),
  the key space is the caller's node's `@key`, looked up once by the
  claim before anything is written. A token naming no node creates
  nothing.
- Keys shared by two owners (`f1:lou`) stay hand-written rules, written
  with `${viewer.key}`.

See it run in the
[owner-scoped keys recipe](/docs/graphql/authorization-recipes#owner-scoped-keys).

### What a refused write reveals

- A write whose rules the claims alone settle against (a role check, the
  `mutations` default) is refused before any statement runs, as
  `@authentication` is: `FORBIDDEN`, or `UNAUTHENTICATED` without a token.
- A create under CREATE rules answers the same whether its key or a
  `@unique` value is taken by a node the caller may not create: it gets
  the answer a free value gets. A create that would succeed answers
  `CONSTRAINT_VIOLATION`. On a type with no CREATE `validate` rule, a
  `@unique` value held by a hidden node still reads as taken.
- Update and delete targets, by key, bulk or nested, must pass the type's
  `READ` filter as well as its `UPDATE` or `DELETE` filter. A key the
  caller cannot read answers like a missing one (`null`, or
  `nodesDeleted: 0`), never `FORBIDDEN`.
- Write errors never name a node the caller cannot read. A delete that
  would leave such a node without a required relationship fails with
  `a Secret the caller can't read requires a Person (Secret.holder)`.
- Replacing a single relationship whose current target the caller cannot
  read is `FORBIDDEN` (`not allowed to replace F.genre`) and leaves it in
  place: a caller cannot remove a relationship of a node they cannot see.
- A field-rule filter is false, never null, on rows that fail the rule, so
  `NOT` over it reveals no more than the filter itself. The same holds for
  relationship and `@cypher` fields with READ rules, `<field>Exists` and
  `<field>Connection`.

### Checking access {#checking-access}

- `lora-graphql access` (or `lora.accessMatrix()`) prints who may do what:
  for every type and guarded field, each operation as each kind of caller
  (anonymous, authenticated, `viewer`, and each role the rules test, such
  as `roles:admin`), with the verdict (`allowed`, `filtered`, `validated`,
  `masked`, `denied`, `unauthenticated`) and the rules that decide it. The
  order is stable, so a snapshot in CI turns access changes into diffs.
  See [the CLI](/docs/graphql/cli).
- `lora.operationAccess(document)` answers the same question for one
  operation: which kinds of caller can run it, root field by root field,
  nested relationship writes included. Use it to check a client bundle at
  build time. See the [API reference](/docs/graphql/api-reference#operationaccessdocument-operationname-variables).
- `lora-graphql check` fails on unguarded `@mutation` writes (see
  [defaults](#defaults)) and lints authorization:
  - a filter rule every signed-in caller passes;
  - a rule that needs no claims in a schema that leaves
    `requireAuthentication` unset, and a write rule a signed-out caller
    passes once it is `false`;
  - an `UPDATE` rule that tests a single relationship the update input can
    re-point, and a `CREATE` rule that tests a field `UPDATE` can change:
    the caller passes the rule, then changes what it tested;
  - a write rule that tests a masked field, which lets a caller probe the
    hidden value by whether the write succeeds;
  - nested creates, updates or deletes offered into a type whose rules
    refuse everyone but admins;
  - field rules the schema's bypass skips, and one line naming every type
    the bypass reaches.
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
- Relationships removed by deleting a node. Relationship rules check
  connects and disconnects, not the relationships a delete takes with it.
- Deleted nodes in subscriptions, in part. On a type with READ or
  SUBSCRIBE rules a deletion goes only to subscribers following that
  `key`, checked on the node as it was inside the deleting transaction.
  With `changeFeed: true` only followers whose claims alone settle the
  rules get it.
- Relationship rules (`CONNECT`, `DISCONNECT`, `UPDATE_EDGE`, `READ_EDGE`)
  on a field whose target is an interface or union. They are refused at
  startup; guard such a field with `@authentication` or rules on the
  member types.
