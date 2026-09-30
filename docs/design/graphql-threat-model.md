# GraphQL: Threat Model

What `packages/lora-graphql` defends against, what it trusts, and where
each check runs. The user-facing rules are in the package
[README](../../packages/lora-graphql/README.md) and on the site's
[authorization](../../apps/loradb.com/docs/graphql/authorization.md) page.

## Trust boundaries

| Input                              | Trusted?      | Why                                                                                                                                                                                           |
| ---------------------------------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| The annotated SDL (`typeDefs`)     | Yes           | Written by the application. It decides what is exposed and which rules apply.                                                                                                                 |
| `@cypher` statements               | Yes           | Trusted code. They run with the database's full access and can read `$jwt`. Rules on the owning type do not reach inside them. Review them like any other server code.                        |
| `@populatedBy` callbacks           | Yes           | Application code, run in the server process.                                                                                                                                                  |
| The GraphQL context and its `jwt`  | Yes, as given | The library never verifies tokens. The server verifies the JWT and puts the verified claims in the context. A server that copies unverified claims into the context grants whatever they say. |
| Documents, variables, cursors, ids | No            | Everything a client sends. Parsed, validated, bounded and bound as parameters, never spliced into Cypher text.                                                                                |

## What a client can and cannot do

- **Cypher injection.** Values are bound as `$pN` parameters. Labels,
  types and property names come from the model, never from the request.
- **Read past the rules.** `@authorization` filter rules and READ validate
  rules are compiled into every statement that reads the type: lists,
  connections, `totalCount`, aggregates, relationship fields, search,
  `node(id:)` and subscription events. `test/auth-properties.test.ts`
  checks this against a reference evaluator over random graphs, claims,
  contexts and nested filters.
- **Read a guarded field without a token.** Field-level READ rules and
  `@authentication` are checked per row in the statement, with or without
  a token; without one every row reads the field as `UNAUTHENTICATED`.
  The statement's shape does not depend on the token, which is what lets
  plans be checked anonymously (G-7).
- **Probe with claims.** A rule that needs a claim or context value the
  request lacks denies, and a `NOT` cannot turn that into a grant. Claim
  and context paths read own properties only, so `constructor`,
  `__proto__` and other inherited names resolve to nothing. `eq`, `in`
  and `includes` on claims compare structurally.
- **Write a guarded field.** Field-level rules apply on every path that
  writes the field. They guard the caller's writes: a value the server
  fills (`@default`, `@populatedBy`, a generated key) on a create whose
  input leaves the field out is not checked against the field's CREATE
  rule (G-22). The server's defaults are part of the schema, not of the
  request. Relationship properties are covered on connect,
  nested create, re-connect and edge update; a re-connect is checked
  against the UPDATE rules when the relationship already exists and
  against CREATE otherwise, after the `MERGE`, and a refusal rolls the
  mutation back. A `@relationshipProperties` type can sit under fields
  on both ends, so its rules test claims only, and the model refuses a
  `node` part instead of guessing which end it means. Reading, filtering,
  sorting and aggregating such a property follow its READ rules
  (`test/consumer-regressions.test.ts`, G-9). `@settable` and `@readonly`
  on a relationship field take it out of the create or update input, and
  an upsert of an existing node keeps the relationship it was created
  with (G-10). Directives about stored values (`@default`, `@timestamp`,
  `@private`, …) on a relationship field are model errors, never
  silently ignored.
- **Squat another user's keys.** A rule can build a string from claims
  (`key: { startsWith: "${jwt.sub}:" }`), so a client-chosen key can be
  tied to its owner. The built string is a bound parameter. A claim that
  is absent or not a scalar makes the rule deny, under `NOT` too. The
  separator must be one no subject contains, or one user's space
  contains another's (G-19): with `-`, user `a` could take `a-b-…`, the
  key space of user `a-b`.
- **Probe for keys the caller cannot see.** A `create…` or `upsert…`
  (nested creates included) under a key that exists but is hidden from
  the caller by a READ filter gets the answer the same key would get if
  it were free, never a CONSTRAINT_VIOLATION that confirms it exists.
  An upsert looks for the existing node through the READ filter as well
  as the UPDATE one, so a node its UPDATE rules would allow but READ
  hides takes the create path too, rather than being updated.
  Before creating, the mutation moves each such node to a placeholder key
  inside its transaction, so everything it checks (rules, connect
  targets, cardinality, required relationships) runs on a graph that
  differs from the free-key case only in that hidden key, and raises the
  free key's error. Where the free key would be created, the mutation
  fails with the FORBIDDEN of a denied create instead. Either way the
  transaction rolls back and the key is restored (G-20). What a caller
  can still learn: that a key the rules would let them create is not
  free, as a FORBIDDEN where a free key succeeds. A collision with a node
  the caller can read stays CONSTRAINT_VIOLATION, which tells them
  nothing new. A non-key `@unique` field is not covered: a value held by
  a hidden node still reads as taken. Do not make a field `@unique` if
  whether a value is taken must stay private; key it instead.
- **Forge cursors.** Without `cursorSecret`, cursors are tagged with
  their sort, not signed: a client can craft one, but it only moves a
  page's start within rows the caller may read, because sorting on fields
  with row rules is refused. With `cursorSecret`, cursors carry an
  HMAC-SHA-256 signature and any other cursor is rejected.
- **Exhaust the server.** `maxCost` bounds the rows an operation touches
  before it runs. The document guards bound the work before that: depth,
  aliases, root fields, lexer tokens. `persistedOnly` removes ad-hoc
  documents altogether. `timeoutMs` bounds every statement, and a
  mutation's size is bounded by `maxBatch`.
- **Learn the schema.** Introspection is off by default when `NODE_ENV`
  is `production` (`guards.introspection`). The public schema is still
  whatever the SDL exposes; hiding introspection is not access control.
- **Read internals from errors.** Database errors can name labels,
  properties and Cypher. With `maskErrors` (on by default in production)
  the client gets `DATABASE_ERROR` and an `id`, and `onError` gets the
  detail. Errors the package writes itself (`FORBIDDEN`, `NOT_FOUND`,
  constraint violations) stay readable, without the engine error they
  were mapped from.

## Writes and change events

- `isViewer` and `viewer` (with `@viewer`) are resolved from the claim the
  server verified, like `$jwt.sub`: the library never looks the caller up
  by anything the client sends. The `@viewer` field must be `@key` or
  `@unique`, so a claim names at most one node. A missing claim is unknown
  and never grants, including under `NOT`.

- A directive in a position the model would not apply is a model error at
  startup (`src/model/positions.ts`), so a rule or visibility directive
  written to protect data is never accepted and ignored. `@authentication`
  on a relationship field guards writing the relationship (connect,
  disconnect, nested create and delete), not only reading it, and a
  relationship property settable on create only cannot be changed by a
  re-connect.
- A mutation runs in one transaction. BEFORE rules are checked before
  the first write and AFTER rules before commit; a failure rolls back.
- `onWrite` listeners and `changes()` receive the exact write-set
  (types, keys, relationship refs) of committed writes, without applying
  READ rules. They are server-side hooks: do not forward them to clients
  unfiltered.
- Subscription events are filtered per subscriber with the READ and
  SUBSCRIBE rules, in the database, after the write. A deleted node
  cannot be checked after the fact: its deletion only reaches subscribers
  that follow that key, and only unfiltered ones.
- A `@cypher` mutation has no known write-set. Its `onWrite` event is one
  broad event (`broad: true`) that names nothing. Without `changeFeed`,
  that broad event is also what subscriptions and `changes()` get. With
  `changeFeed: true` they are fed from the engine's committed change feed
  instead, which names the nodes and relationships the statement
  actually wrote.

## Where each check runs

| Check                                             | When                                        |
| ------------------------------------------------- | ------------------------------------------- |
| Model validity, rule fields, `@cypher` parameters | Startup (`new LoraGraphQL`)                 |
| `@cypher` read-only for queries                   | Startup; planned with `check()`             |
| Persisted documents                               | `persist()`                                 |
| Document guards                                   | Parse and validation, per request           |
| `@authentication`, claim-only rule parts          | Compile time, per request                   |
| Relationship property rules (claims only)         | Compile time; a connect's after its `MERGE` |
| Node rules (filter, validate)                     | In the database, in the statement           |
| Cost limit                                        | Compile time, before the statement          |
| Timeouts, cancellation                            | In the database                             |

## Operator checklist

- Verify the JWT in the server and put only verified claims in the
  context.
- Set `cursorSecret` from a secret store when cursors must not be
  forgeable.
- Run with `NODE_ENV=production`, or set `maskErrors` and
  `guards.introspection` explicitly.
- Prefer `persistedOnly` for first-party clients.
- Review every `@cypher` statement as trusted code.
