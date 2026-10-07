---
title: GraphQL Authentication
sidebar_label: Authentication
description: How a request becomes a caller in @loradb/lora-graphql. Verify the JWT in your server, declare its claims, and require a signed-in caller or a claim per type, operation, field, relationship or custom query, with the real response each caller gets.
keywords: [graphql authentication, jwt, jose, jwks, bearer token, unauthenticated, viewer]
---

# Authentication

Authentication answers one question: who is calling? In
`@loradb/lora-graphql` that takes three steps, and only the last one
involves the library's directives.

1. **Your server verifies the token** and puts its claims in the GraphQL
   context as `jwt`.
2. **Your schema declares the claims** it will use, with `@jwt`.
3. **`@authentication` says where a caller must be signed in**, and
   optionally which claim they need.

What a signed-in caller may then see and change, row by row, is
[authorization](/docs/graphql/authorization), and the
[recipes](/docs/graphql/authorization-recipes) put both together.

Every request and response on this page is run by the package's test
suite. The callers are these three sets of verified claims:

```json title="callers.json"
{
  "ada": { "sub": "ada", "roles": ["member"] },
  "grace": { "sub": "grace", "roles": ["member", "analyst"] },
  "root": { "sub": "root", "roles": ["admin"] }
}
```

A block titled **As ada** runs with Ada's claims in the context. One
titled **Anonymous** runs with no `jwt` at all.

## Step 1: verify the token in your server

:::warning The library never verifies tokens
It trusts whatever is in `context.jwt`. A server that decodes a token
without checking its signature, or copies a header into the context,
grants whatever that token or header claims.
:::

Verify the signature, issuer, audience and expiry with a JWT library.
These examples use [`jose`](https://github.com/panva/jose).

**A shared secret (HS256)**, typical when your own backend issues the
tokens:

```ts title="auth.ts" file="auth.ts"
import { jwtVerify, type JWTPayload } from "jose";

const secret = new TextEncoder().encode(
  process.env.JWT_SECRET ?? "dev-only-secret-do-not-use",
);

export const issuer = process.env.JWT_ISSUER ?? "https://auth.example.com/";
export const audience = process.env.JWT_AUDIENCE ?? "blog-api";

/**
 * The verified claims of a request. Undefined without a token; throws for
 * a token that is expired, forged, malformed or meant for someone else.
 */
export async function verifiedClaims(
  authorization: string | null | undefined,
): Promise<JWTPayload | undefined> {
  if (!authorization) return undefined;
  const token = /^Bearer (.+)$/.exec(authorization)?.[1];
  if (!token) throw new Error("not a bearer token");
  const { payload } = await jwtVerify(token, secret, {
    issuer,
    audience,
    algorithms: ["HS256"],
  });
  return payload;
}
```

**A key set (RS256 or ES256)**, typical with an identity provider such as
Auth0, Clerk, Cognito, Keycloak or Entra ID:

```ts title="auth.ts" file="auth-jwks.ts"
import { createRemoteJWKSet, jwtVerify, type JWTPayload } from "jose";

// Your identity provider publishes its public keys at a well-known URL.
// jose fetches them on first use, caches them, and refetches when a token
// names a key it has not seen (key rotation).
const keys = createRemoteJWKSet(
  new URL(
    process.env.JWKS_URL ?? "https://auth.example.com/.well-known/jwks.json",
  ),
);

const issuer = process.env.JWT_ISSUER ?? "https://auth.example.com/";
const audience = process.env.JWT_AUDIENCE ?? "blog-api";

export async function verifiedClaims(
  authorization: string | null | undefined,
): Promise<JWTPayload | undefined> {
  if (!authorization) return undefined;
  const token = /^Bearer (.+)$/.exec(authorization)?.[1];
  if (!token) throw new Error("not a bearer token");
  const { payload } = await jwtVerify(token, keys, {
    issuer,
    audience,
    algorithms: ["RS256", "ES256"],
  });
  return payload;
}
```

Always pass `algorithms`. Without it a verifier accepts whatever
algorithm the token's own header names, which is how a token signed with
a public key as if it were a shared secret gets through.

Three outcomes matter, and they should stay distinct:

| Request | `verifiedClaims` | The caller is |
| --- | --- | --- |
| No `Authorization` header | returns `undefined` | Anonymous |
| A valid token | returns the claims | Signed in |
| An expired, forged or malformed token | throws | Nobody: reject the request |

Do not turn the third case into the first. A client with an expired token
should get a `401` and refresh it, not a quietly anonymous response that
looks like missing data.

### Put the claims in the context

The context function is where the three outcomes meet your server.

```ts title="GraphQL Yoga" file="server.ts"
const yoga = createYoga({
  schema: lora.getSchema(),
  // Document guards: depth, aliases, root fields, tokens, introspection.
  plugins: [lora.envelopPlugin()],
  context: async ({ request }) => {
    try {
      return {
        jwt: await verifiedClaims(request.headers.get("authorization")),
        signal: request.signal,
      };
    } catch {
      throw new GraphQLError("Invalid or expired token", {
        extensions: { code: "UNAUTHENTICATED", http: { status: 401 } },
      });
    }
  },
});
```

```ts title="Apollo Server" file="apollo.ts"
const server = new ApolloServer({
  schema: lora.getSchema(),
  // Apollo has no Envelop: wire the document guards in by hand.
  validationRules: lora.validationRules(),
  parseOptions: parseOptions(),
});

const { url } = await startStandaloneServer(server, {
  listen: { port: Number(process.env.PORT ?? 4000) },
  context: async ({ req }) => {
    try {
      return { jwt: await verifiedClaims(req.headers.authorization) };
    } catch {
      throw new GraphQLError("Invalid or expired token", {
        extensions: { code: "UNAUTHENTICATED", http: { status: 401 } },
      });
    }
  },
});
console.log(`listening on ${url}`);
```

```ts title="Your own handler with execute()" file="handler.ts"
const server = createServer(async (req, res) => {
  let jwt;
  try {
    jwt = await verifiedClaims(req.headers.authorization);
  } catch {
    res.writeHead(401, { "www-authenticate": "Bearer" }).end();
    return;
  }

  let body = "";
  for await (const chunk of req) body += chunk;
  const { query, variables, operationName } = JSON.parse(body);

  // Stop the request's statements when the client goes away.
  const controller = new AbortController();
  res.on("close", () => controller.abort());

  const result = await lora.execute({
    source: query,
    variables,
    operationName,
    context: { jwt, signal: controller.signal },
  });
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(result));
});
```

```ts title="graphql-ws (subscriptions)" file="ws.ts"
// Browsers cannot set headers on a WebSocket, so the token travels in the
// connection parameters: createClient({ connectionParams: { authorization } }).
const claimsOf = (params: Record<string, unknown> | undefined) =>
  verifiedClaims(params?.authorization as string | undefined);

useServer(
  {
    schema: lora.getSchema(),
    // Refuse the connection itself for a bad token.
    onConnect: async (ctx) => {
      try {
        await claimsOf(ctx.connectionParams);
      } catch {
        return false;
      }
    },
    // One context per subscription. `connection` is what lora.ts counts
    // live subscriptions by (subscriptionScope).
    context: async (ctx) => ({
      jwt: await claimsOf(ctx.connectionParams),
      connection: ctx,
    }),
  },
  new WebSocketServer({ server: httpServer, path: "/graphql" }),
);
```

These four blocks are excerpts of the servers in
[`examples/tutorial`](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples/tutorial).
That project's test starts each server and calls it with real signed
tokens: valid, expired, signed with the wrong key, meant for another
audience and malformed. The first is accepted and the other four get a
`401` from every server.

A WebSocket outlives its token. Decide what happens when the token
expires: the simplest policy is to close the connection at the token's
`exp` and let the client reconnect with a fresh one.

[Serving the schema](/docs/graphql/serving) has the complete servers.

### Claims that live somewhere else

`context.jwt` is the default. If your context is shaped differently, tell
the library where to look:

```ts
new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  jwt: (context) => context.session?.claims,
});
```

The function may return claims from any source you have verified: a
session cookie, an API key looked up in a table, a service identity from
mutual TLS. The library only needs an object of claims, or `undefined` for
an anonymous caller.

### Development tokens

For local work, mint a token with the same secret the server verifies:

```ts title="token.ts" file="token.ts"
// Mints a development token, signed with the secret auth.ts verifies.
// Real deployments get tokens from their identity provider.
// Usage: npm run token -- <subject> [role...]

import { SignJWT } from "jose";
import { audience, issuer } from "./auth.js";

const secret = new TextEncoder().encode(
  process.env.JWT_SECRET ?? "dev-only-secret-do-not-use",
);
const [subject = "ada", ...roles] = process.argv.slice(2);

console.log(
  await new SignJWT({ roles })
    .setProtectedHeader({ alg: "HS256" })
    .setSubject(subject)
    .setIssuer(issuer)
    .setAudience(audience)
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(secret),
);
```

```bash
curl http://localhost:4000/graphql \
  -H "authorization: Bearer $(npx tsx token.ts ada)" \
  -H "content-type: application/json" \
  -d '{"query":"{ notes { key text } }"}'
```

## Step 2: declare the claims

A `@jwt` type lists the claims your rules use. With it, a rule that names
any other claim is an error at startup, so a typo cannot become a rule
that never matches.

```graphql
type Claims @jwt {
  sub: String!
  roles: [String!]
  tenant: String @jwtClaim(path: "app_metadata.tenant_id")
}
```

`@jwtClaim(path:)` maps a short name to where the value lives in the
token, with dots stepping into nested objects. Rules then say `tenant`.

:::note Claim names that contain dots
Some providers namespace custom claims with a URL, such as
`https://example.com/roles`. A path is split on dots, so such a name
cannot be reached with `@jwtClaim`. Copy it to a plain name in your verify
function instead:

```ts
const { payload } = await jwtVerify(token, keys, options);
return { ...payload, roles: payload["https://example.com/roles"] };
```
:::

The type's name does not matter and it is not part of the public schema.

## Step 3: require a caller

### A signed-in caller, for everything on a type

`@authentication` with no arguments requires a signed-in caller for every
operation on the type.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Note @node @mutation @authentication {
  key: String! @key
  text: String!
}
```

```cypher title="seed"
CREATE (:Note {key: 'n1', text: 'Ship the docs'})
```

```graphql title="Anonymous"
{
  notes {
    key
    text
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "read on Note needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

```graphql title="As ada"
{
  notes {
    key
    text
  }
}
```

```json
{ "data": { "notes": [{ "key": "n1", "text": "Ship the docs" }] } }
```

A refusal happens before any statement runs: the database is not touched.

### A caller with a claim

The `jwt` argument adds a test on the claims. Being signed in is then not
enough.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Report
  @node
  @authentication(jwt: { roles: { includes: "analyst" } }) {
  key: String! @key
  revenue: Int!
}
```

```cypher title="seed"
CREATE (:Report {key: 'q3', revenue: 120000})
```

Ada is signed in but is not an analyst:

```graphql title="As ada"
{
  reports {
    key
    revenue
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "read on Report needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

Grace is:

```graphql title="As grace"
{
  reports {
    key
    revenue
  }
}
```

```json
{ "data": { "reports": [{ "key": "q3", "revenue": 120000 }] } }
```

The code is `UNAUTHENTICATED` in both failing cases, with or without a
token. The message says which operation needs what; branch on the code.

The `jwt` test takes `eq`, `in`, `includes`, `contains`, `startsWith`,
`endsWith`, `lt`, `lte`, `gt`, `gte` and `exists`, combined with `AND`,
`OR` and `NOT`:

```graphql
@authentication(
  jwt: {
    OR: [
      { roles: { includes: "admin" } }
      { AND: [{ roles: { includes: "analyst" } }, { tenant: { eq: "acme" } }] }
    ]
  }
)
```

### Public reads, signed-in writes

`operations` narrows the directive. This is the usual shape for content
anyone may read and only signed-in users may change.

```graphql title="schema.graphql"
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
}
```

```cypher title="seed"
CREATE (:Article {slug: 'hello', title: 'Hello'})
```

```graphql title="Anonymous"
{
  articles {
    slug
    title
  }
}
```

```json
{ "data": { "articles": [{ "slug": "hello", "title": "Hello" }] } }
```

```graphql title="Anonymous"
mutation {
  createArticles(input: [{ slug: "spam", title: "Buy now" }]) {
    info {
      nodesCreated
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "create on Article needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

```graphql title="As ada"
mutation {
  createArticles(input: [{ slug: "graphs", title: "Graphs" }]) {
    articles {
      slug
    }
  }
}
```

```json
{ "data": { "createArticles": { "articles": [{ "slug": "graphs" }] } } }
```

The operations are:

| Operation | Covers |
| --- | --- |
| `READ` | Queries, nested reads, filters, aggregates |
| `CREATE`, `UPDATE`, `DELETE` | The generated mutations |
| `CREATE_RELATIONSHIP` | `connect` and nested `create` |
| `DELETE_RELATIONSHIP` | `disconnect` and nested `delete` |
| `SUBSCRIBE` | Subscriptions on the type |

Several `@authentication` needs on one type go in one directive. For
different claims per operation, use
[`@authorization` rules](/docs/graphql/authorization), which take a `jwt`
test per rule.

### One field

On a field, `@authentication` guards that field alone. The type stays
readable; the field is not.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Person @node {
  key: String! @key
  name: String!
  email: String @authentication(operations: [READ])
}
```

```cypher title="seed"
CREATE (:Person {key: 'ada', name: 'Ada', email: 'ada@example.com'}),
       (:Person {key: 'grace', name: 'Grace', email: 'grace@example.com'})
```

An anonymous caller reads the names:

```graphql title="Anonymous"
{
  persons {
    name
  }
}
```

```json
{ "data": { "persons": [{ "name": "Ada" }, { "name": "Grace" }] } }
```

Asking for the email fails that field on every row, and the rest of the
response still arrives:

```graphql title="Anonymous"
{
  persons {
    name
    email
  }
}
```

```json
{
  "data": {
    "persons": [
      { "name": "Ada", "email": null },
      { "name": "Grace", "email": null }
    ]
  },
  "errors": [
    {
      "message": "Person.email needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

The two rows produce one error, not two: `execute()` and the Envelop
plugin [collapse repeated errors](/docs/graphql/errors#repeated-errors).

A signed-in caller reads it:

```graphql title="As ada"
{
  persons {
    name
    email
  }
}
```

```json
{
  "data": {
    "persons": [
      { "name": "Ada", "email": "ada@example.com" },
      { "name": "Grace", "email": "grace@example.com" }
    ]
  }
}
```

Field-level `@authentication` also guards filtering, sorting and
aggregating by the field, so a guarded value cannot be probed through a
`where`. To let each person read only their own email, you need a rule
that looks at the row: see
[private fields](/docs/graphql/authorization-recipes#private-fields).

### Writes through a relationship

On a relationship field, `@authentication` guards the links. Here anyone
may see who is on a team, and only signed-in callers may change that.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Team
  @node
  @mutation(operations: [UPDATE])
  @authorization(public: [UPDATE]) {
  key: String! @key
  members: [Person!]!
    @relationship(
      type: "MEMBER_OF"
      direction: IN
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @authentication(operations: [CREATE_RELATIONSHIP, DELETE_RELATIONSHIP])
}

type Person @node {
  key: String! @key
}
```

```cypher title="seed"
CREATE (:Team {key: 'core'}), (:Person {key: 'ada'}), (:Person {key: 'grace'})
```

```graphql title="Anonymous"
mutation {
  updateTeam(
    key: "core"
    update: { members: { connect: [{ key: "ada" }] } }
  ) {
    info {
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "Team.members needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

```graphql title="As ada"
mutation {
  updateTeam(
    key: "core"
    update: { members: { connect: [{ key: "ada" }] } }
  ) {
    team {
      members {
        key
      }
    }
  }
}
```

```json
{ "data": { "updateTeam": { "team": { "members": [{ "key": "ada" }] } } } }
```

`@authorization(public: [UPDATE])` declares that updating a team needs no
rule of its own: the only thing an update can change here is `members`,
which carries the requirement. `lora-graphql check` fails on a write
nothing accounts for, and `public` is how you say an open one is
intended.

The field's directive says a caller must be signed in. It does not say
which caller may add whom: that is a
[rule on the relationship](/docs/graphql/authorization-recipes#teams-and-membership).

### Custom queries and mutations

A root `@cypher` field has no type to inherit from, so guard it directly.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Order @node {
  key: String! @key
  total: Int!
}

type Query {
  revenue: Int!
    @authentication(jwt: { roles: { includes: "admin" } })
    @cypher(statement: "MATCH (o:Order) RETURN sum(o.total) AS revenue")
}
```

```cypher title="seed"
CREATE (:Order {key: 'o1', total: 40}), (:Order {key: 'o2', total: 60})
```

```graphql title="As ada"
{
  revenue
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "Query.revenue needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

```graphql title="As root"
{
  revenue
}
```

```json
{ "data": { "revenue": 100 } }
```

An unguarded root `@cypher` field is open to everyone, whatever rules the
types it reads have: the statement runs with full access. See
[@cypher fields](/docs/graphql/cypher-fields#guarding-root-fields).

### Subscriptions

`SUBSCRIBE` guards opening a subscription on the type. The type's `READ`
requirement applies too, since a subscription delivers the node.

```graphql
type Order
  @node
  @subscription
  @authentication(operations: [READ, SUBSCRIBE]) {
  key: String! @key
  total: Int!
}
```

The claims are the ones in the context when the subscription starts. See
the `graphql-ws` example [above](#put-the-claims-in-the-context) for
getting them from the connection.

## Who am I: the @viewer claim {#who-am-i}

Most applications store their users as nodes. `@viewer` names the claim
that identifies the caller's node, and from then on rules and statements
can speak of "the caller" instead of a claim.

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}

type Person @node {
  key: String! @key
  name: String!
}

type Query {
  me: Person
    @cypher(statement: "MATCH (p:Person) WHERE p.key = $viewer RETURN p")
}
```

```cypher title="seed"
CREATE (:Person {key: 'ada', name: 'Ada Lovelace'}),
       (:Person {key: 'grace', name: 'Grace Hopper'})
```

```graphql title="As ada"
{
  me {
    key
    name
  }
}
```

```json
{ "data": { "me": { "key": "ada", "name": "Ada Lovelace" } } }
```

```graphql title="Anonymous"
{
  me {
    key
    name
  }
}
```

```json
{ "data": { "me": null } }
```

`me` is `null` for an anonymous caller, not an error, because the field
has no `@authentication`. Add it if a missing token should be refused.

The field `@viewer` points at must be the type's `@key` or a `@unique`
field, so one claim names at most one node. Pointing it at a separate
unique field lets the key stay a readable slug while the claim is your
identity provider's opaque id:

```graphql
type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "subject")
}

type Person @node {
  key: String! @key # "ada"
  subject: String! @unique # "auth0|652f1c..."
}
```

`@viewer` is the basis of most authorization rules: `isViewer`, `viewer`
and `${viewer.key}`. See [the caller's own node](/docs/graphql/authorization#viewer).

## Anonymous callers and rules

`@authentication` is the explicit way to require a token. Authorization
rules have a default of their own: without a token, **every
`@authorization` rule denies**, including rules that test no claim at
all. A type with a rule "published posts are visible" is therefore
invisible to anonymous callers until you say otherwise, once, for the
whole schema:

```graphql
extend schema @authorizationDefaults(requireAuthentication: false)
```

The [public read recipe](/docs/graphql/authorization-recipes#public-read-owner-write)
shows this setting at work, and
[anonymous callers](/docs/graphql/authorization#anonymous-callers) has the
exact semantics.

## Test as a caller

The testing helper runs operations with any context, so a test can be
each caller in turn with no token and no server:

```ts title="notes.test.ts"
import { expect, test } from "vitest";
import { createTestLoraGraphQL } from "@loradb/lora-graphql/testing";

const ada = { jwt: { sub: "ada", roles: ["member"] } };

test("notes need a signed-in caller", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs,
    seed: "CREATE (:Note {key: 'n1', text: 'Ship the docs'})",
  });

  const anonymous = await t.run(`{ notes { key } }`);
  expect(anonymous.errors?.[0]?.extensions?.code).toBe("UNAUTHENTICATED");

  const signedIn = await t.data(`{ notes { key } }`, {}, ada);
  expect(signedIn).toEqual({ notes: [{ key: "n1" }] });
});
```

To see the whole picture at once, print the access matrix. It lists every
type and guarded field, each operation, and the verdict for each kind of
caller:

```bash
lora-graphql access schema.graphql
```

See [testing](/docs/graphql/testing#asserting-access) and
[checking access](/docs/graphql/authorization#checking-access).

## Common mistakes

- **Decoding instead of verifying.** `jwt.decode()` and `atob()` read a
  token without checking who signed it. Only a verify call that checks the
  signature, issuer, audience and expiry produces claims you can trust.
- **Treating a bad token as anonymous.** It hides expiry from the client
  and turns an attack into a normal-looking response. Reject it.
- **Trusting a header.** `x-user-id` set by the client is not an identity.
  If a gateway sets it, make sure nothing else can reach the server.
- **Forgetting custom fields.** Rules on a type do not guard a root
  `@cypher` field. `lora-graphql access` lists every one, guarded or not.
- **Caching across callers.** Responses differ per caller. Key any
  response cache by the token or the user.
- **Expecting `FORBIDDEN` from `@authentication`.** It always answers
  `UNAUTHENTICATED`. `FORBIDDEN` comes from `@authorization` rules.

## Next

- [Authorization](/docs/graphql/authorization): rules that look at the
  row, not just the caller.
- [Authorization recipes](/docs/graphql/authorization-recipes): complete,
  tested schemas for common access models.
- [Serving the schema](/docs/graphql/serving): the servers these context
  functions plug into.
