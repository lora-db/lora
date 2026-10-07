---
title: "Tutorial: Build a GraphQL API on LoraDB"
sidebar_label: Tutorial
description: Build a small blog API with @loradb/lora-graphql from an empty directory. A schema, a server, relationships, writes, sign-in with JWTs, row-level rules, subscriptions, tests and a CI gate, with every step's requests and responses.
keywords: [graphql tutorial, loradb graphql, build graphql api, jwt graphql tutorial, graphql yoga tutorial]
---

# Tutorial: build a GraphQL API

In about half an hour you will build the API of a small blog: people,
posts and tags, where anyone reads published posts and authors manage
their own. You start from an empty directory and finish with sign-in,
row-level rules, a test and a CI gate.

Each step changes one thing and shows the requests it makes possible,
with the responses the server gives. Nothing here is hypothetical:

- the finished project is
  [`examples/tutorial`](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples/tutorial)
  in the repository, with a test that starts the server and calls it over
  HTTP with real signed tokens;
- the package's test suite runs every request on this page against the
  schema of its step.

You need Node.js 20 or later. No database to install: LoraDB runs inside
the server process.

## Step 1: a schema and a server

```bash
mkdir blog && cd blog
npm init -y && npm pkg set type=module
npm install @loradb/lora-graphql @loradb/lora-node graphql graphql-yoga jose
npm install --save-dev tsx typescript @types/node
```

Describe the data. A type with `@node` is a kind of node in the graph,
and its `@key` is how every node of that kind is addressed.

```graphql title="schema.graphql"
type Person @node {
  key: String! @key
  name: String! @sortable
}

type Post @node {
  slug: String! @key
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  body: String
  published: Boolean! @default(value: false) @filterable
}
```

Nothing is filterable or sortable unless you say so. Here clients may
sort people by name, and filter posts by title and by `published`.

Some data to start with, in Cypher, LoraDB's query language. The
database is in memory, so the server loads this file every time it
starts:

```cypher title="seed.cypher"
CREATE (ada:Person {key: 'ada', name: 'Ada Lovelace'}),
       (grace:Person {key: 'grace', name: 'Grace Hopper'}),
       (graphs:Tag {name: 'graphs'}),
       (history:Tag {name: 'history'}),
       (engines:Post {
         slug: 'engines', title: 'On analytical engines',
         body: 'The engine weaves algebraic patterns.', published: true
       }),
       (draft:Post {
         slug: 'notes', title: 'Notes on Bernoulli numbers',
         body: 'Unfinished.', published: false
       }),
       (cobol:Post {
         slug: 'cobol', title: 'Why COBOL reads like English',
         body: 'So that people can read it.', published: true
       }),
       (ada)-[:WROTE]->(engines),
       (ada)-[:WROTE]->(draft),
       (grace)-[:WROTE]->(cobol),
       (engines)-[:TAGGED]->(history),
       (engines)-[:TAGGED]->(graphs),
       (cobol)-[:TAGGED]->(history)
```

Now the two TypeScript files. The first opens the database and turns the
schema into an API:

```ts title="lora.ts" file="lora.ts"
// The database and the GraphQL schema object, shared by every server in
// this directory.

import { readFile } from "node:fs/promises";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";

const file = (name: string) => readFile(new URL(name, import.meta.url), "utf8");

export const db = await createDatabase();

export const lora = new LoraGraphQL({
  typeDefs: await file("./schema.graphql"),
  driver: loraDriver(db),
  // Sign cursors in production, from a secret store.
  ...(process.env.CURSOR_SECRET
    ? { cursorSecret: process.env.CURSOR_SECRET }
    : {}),
  // Count live subscriptions per WebSocket connection (see ws.ts).
  subscriptionScope: (context) =>
    (context as { connection?: object }).connection,
});

// Create the constraints and indexes the API needs, then load some data.
await lora.assertSchema({ create: true });
await db.execute(await file("./seed.cypher"));
```

`assertSchema({ create: true })` creates the constraints and indexes the
API needs: a key constraint per type, and an index behind every sort and
range filter. You never write index definitions by hand.

The second serves it. `lora.getSchema()` is a standard GraphQL schema, so
any GraphQL server can serve it; this one is GraphQL Yoga.

```ts title="server.ts"
import { createServer } from "node:http";
import { createYoga } from "graphql-yoga";
import { lora } from "./lora.js";

const yoga = createYoga({
  schema: lora.getSchema(),
  plugins: [lora.envelopPlugin()],
});

const port = Number(process.env.PORT ?? 4000);
createServer(yoga).listen(port, () => {
  console.log(`listening on http://localhost:${port}/graphql`);
});
```

```bash
npx tsx server.ts
```

Open `http://localhost:4000/graphql` for Yoga's query editor, or use
`curl`:

```bash
curl -s http://localhost:4000/graphql \
  -H 'content-type: application/json' \
  -d '{"query":"{ posts(sort: [{ title: ASC }]) { slug title } }"}'
```

From here on the page shows the request and the response without the
`curl` around them.

```graphql title="Request"
{
  posts(sort: [{ title: ASC }]) {
    slug
    title
  }
}
```

```json
{
  "data": {
    "posts": [
      { "slug": "notes", "title": "Notes on Bernoulli numbers" },
      { "slug": "engines", "title": "On analytical engines" },
      { "slug": "cobol", "title": "Why COBOL reads like English" }
    ]
  }
}
```

Every type got a list, a lookup by key and a paginated connection:

```graphql title="Request"
{
  post(slug: "cobol") {
    title
    body
  }
  drafts: posts(where: { published: { eq: false } }) {
    title
  }
  postsConnection(first: 1, sort: [{ title: ASC }]) {
    totalCount
    pageInfo {
      hasNextPage
    }
  }
}
```

```json
{
  "data": {
    "post": {
      "title": "Why COBOL reads like English",
      "body": "So that people can read it."
    },
    "drafts": [{ "title": "Notes on Bernoulli numbers" }],
    "postsConnection": {
      "totalCount": 3,
      "pageInfo": { "hasNextPage": true }
    }
  }
}
```

## Step 2: relationships

The seed already connects people to posts and posts to tags. The schema
does not mention those relationships yet, so the API cannot see them.
Declare them:

```graphql title="schema.graphql"
type Person @node {
  key: String! @key
  name: String! @sortable
  posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
}

type Post @node {
  slug: String! @key
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  body: String
  published: Boolean! @default(value: false) @filterable
  author: Person! @relationship(type: "WROTE", direction: IN) @filterable
  tags: [Tag!]!
    @relationship(type: "TAGGED", direction: OUT)
    @filterable
    @cardinality(max: 10)
}

type Tag @node {
  name: String! @key
  posts: [Post!]! @relationship(type: "TAGGED", direction: IN)
}
```

- `Person.posts` and `Post.author` are the same `WROTE` relationship seen
  from its two ends. `author: Person!` says a post has exactly one.
- `Post.tags` and `Tag.posts` are a many-to-many. There is no join table:
  the relationship is the link. See
  [many-to-many relationships](/docs/graphql/many-to-many).
- `@cardinality(max: 10)` tells the cost estimate that a post has at most
  ten tags.

Restart the server (`Ctrl+C`, then `npx tsx server.ts` again) and query
across the graph:

```graphql title="Request"
{
  posts(sort: [{ title: ASC }]) {
    title
    author {
      name
    }
    tags(sort: [{ name: ASC }]) {
      name
    }
  }
}
```

```json
{
  "data": {
    "posts": [
      {
        "title": "Notes on Bernoulli numbers",
        "author": { "name": "Ada Lovelace" },
        "tags": []
      },
      {
        "title": "On analytical engines",
        "author": { "name": "Ada Lovelace" },
        "tags": [{ "name": "graphs" }, { "name": "history" }]
      },
      {
        "title": "Why COBOL reads like English",
        "author": { "name": "Grace Hopper" },
        "tags": [{ "name": "history" }]
      }
    ]
  }
}
```

`@filterable` on a relationship lets clients filter by what a node is
connected to:

```graphql title="Request"
{
  byGrace: posts(where: { author: { key: { eq: "grace" } } }) {
    title
  }
  onGraphs: posts(where: { tags: { some: { name: { eq: "graphs" } } } }) {
    title
  }
  tag(name: "history") {
    posts(sort: [{ title: ASC }]) {
      title
    }
  }
}
```

```json
{
  "data": {
    "byGrace": [{ "title": "Why COBOL reads like English" }],
    "onGraphs": [{ "title": "On analytical engines" }],
    "tag": {
      "posts": [
        { "title": "On analytical engines" },
        { "title": "Why COBOL reads like English" }
      ]
    }
  }
}
```

Each of those root fields is one database statement, however deep the
selection goes.

## Step 3: writes

Reads are generated by default. Writes are opt in, per type, with
`@mutation`. `@query(aggregate: true)` adds counts and aggregates while
you are there.

```graphql title="schema.graphql"
type Person @node {
  key: String! @key
  name: String! @sortable
  posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
}

type Post @node @mutation @query(aggregate: true) {
  slug: String! @key
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  body: String
  published: Boolean! @default(value: false) @filterable
  author: Person! @relationship(type: "WROTE", direction: IN) @filterable
  tags: [Tag!]!
    @relationship(
      type: "TAGGED"
      direction: OUT
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @filterable
    @cardinality(max: 10)
}

type Tag @node {
  name: String! @key
  posts: [Post!]! @relationship(type: "TAGGED", direction: IN)
}
```

`nestedOperations: [CONNECT, DISCONNECT]` lets a post be linked to and
unlinked from existing tags, and nothing else: no creating or deleting
tags through a post.

Create a post, connected to its author and a tag, in one request and one
transaction:

```graphql title="Request"
mutation {
  createPosts(
    input: [
      {
        slug: "compilers"
        title: "The first compiler"
        author: { connect: { key: "grace" } }
        tags: { connect: [{ name: "history" }] }
      }
    ]
  ) {
    posts {
      slug
      published
      author {
        name
      }
    }
    info {
      nodesCreated
      relationshipsCreated
    }
  }
}
```

```json
{
  "data": {
    "createPosts": {
      "posts": [
        {
          "slug": "compilers",
          "published": false,
          "author": { "name": "Grace Hopper" }
        }
      ],
      "info": { "nodesCreated": 1, "relationshipsCreated": 2 }
    }
  }
}
```

`published` was not in the input and came out `false`, from `@default`.
Update it, and count:

```graphql title="Request"
mutation {
  updatePost(slug: "compilers", update: { published: true }) {
    post {
      published
    }
  }
}
```

```json
{ "data": { "updatePost": { "post": { "published": true } } } }
```

```graphql title="Request"
{
  postsAggregate(where: { published: { eq: true } }) {
    count
  }
}
```

```json
{ "data": { "postsAggregate": { "count": 3 } } }
```

The library checks the write, not just its shape. A post must have an
author, and the author must exist:

```graphql title="Request"
mutation {
  createPosts(input: [{ slug: "orphan", title: "Nobody wrote this" }]) {
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
      "message": "Post.author is required: connect or create one",
      "extensions": { "code": "BAD_USER_INPUT" }
    }
  ]
}
```

There is a problem, and the library will tell you about it. Add the CLI's
check to `package.json` and run it:

```bash
npm pkg set scripts.check="lora-graphql check schema.graphql"
npm run check
```

```text
lint     Person.posts: no @cardinality and no statistics: cost estimates assume each parent has a full page of these; run analyze() or declare @cardinality(max:)
lint     Tag.posts: no @cardinality and no statistics: cost estimates assume each parent has a full page of these; run analyze() or declare @cardinality(max:)
error    Post: CREATE, UPDATE, DELETE have no @authentication or @authorization rule: any caller that reaches the API can make them. Guard them, or declare @authorization(public: [CREATE, UPDATE, DELETE])
failed
```

The `lint` lines are advice and do not fail the run. The `error` does:
anyone who can reach the server can create, change and delete any post.
The next two steps fix that.

## Step 4: sign-in

The library never verifies tokens. Your server does, and passes the
verified claims along. Create the function that does it:

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

And a script that mints tokens for development, standing in for your
identity provider:

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

Put the claims in the GraphQL context. This is the finished `server.ts`:

```ts title="server.ts" file="server.ts"
// GraphQL Yoga serving the schema. Run: npm start

import { createServer } from "node:http";
import { GraphQLError } from "graphql";
import { createYoga } from "graphql-yoga";
import { verifiedClaims } from "./auth.js";
import { lora } from "./lora.js";

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

const port = Number(process.env.PORT ?? 4000);
createServer(yoga).listen(port, () => {
  console.log(`listening on http://localhost:${port}/graphql`);
});
```

A request without a token is anonymous. A request with a bad token is
rejected with a `401`, never quietly treated as anonymous.

Now use the claims in the schema. `@jwt` declares them, and
`@authentication` requires a signed-in caller for the writes:

```graphql title="schema.graphql"
type Claims @jwt {
  sub: String!
  roles: [String!]
}

type Person @node {
  key: String! @key
  name: String! @sortable
  posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
}

type Post
  @node
  @mutation
  @query(aggregate: true)
  @authentication(operations: [CREATE, UPDATE, DELETE]) {
  slug: String! @key
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  body: String
  published: Boolean! @default(value: false) @filterable
  author: Person! @relationship(type: "WROTE", direction: IN) @filterable
  tags: [Tag!]!
    @relationship(
      type: "TAGGED"
      direction: OUT
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @filterable
    @cardinality(max: 10)
}

type Tag @node {
  name: String! @key
  posts: [Post!]! @relationship(type: "TAGGED", direction: IN)
}
```

Restart, mint a token and send it:

```bash
TOKEN=$(npx tsx token.ts ada)
curl -s http://localhost:4000/graphql \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"query":"mutation { deletePost(slug: \"notes\") { nodesDeleted } }"}'
```

The rest of the page marks who is calling. These are the claims behind
the names:

```json title="callers.json"
{
  "ada": { "sub": "ada", "roles": [] },
  "grace": { "sub": "grace", "roles": [] }
}
```

Without a token, reads work and writes do not:

```graphql title="Anonymous"
mutation {
  deletePost(slug: "cobol") {
    nodesDeleted
  }
}
```

```json
{
  "data": null,
  "errors": [
    {
      "message": "delete on Post needs an authenticated request",
      "extensions": { "code": "UNAUTHENTICATED" }
    }
  ]
}
```

With one, they do. `npm run check` passes now. But look at what Ada can
do:

```graphql title="As ada"
mutation {
  deletePost(slug: "cobol") {
    nodesDeleted
  }
}
```

```json
{ "data": { "deletePost": { "nodesDeleted": 1 } } }
```

That was Grace's post. Being signed in is not the same as being allowed.
And everyone, signed in or not, can still read Ada's unpublished draft:

```graphql title="Anonymous"
{
  post(slug: "notes") {
    title
    published
  }
}
```

```json
{
  "data": {
    "post": { "title": "Notes on Bernoulli numbers", "published": false }
  }
}
```

## Step 5: rules

Authentication says who is calling. Authorization says what that caller
may see and change, row by row. This is the finished schema:

```graphql title="schema.graphql" file="schema.graphql"
extend schema @authorizationDefaults(requireAuthentication: false)

type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}

type Person @node {
  key: String! @key
  name: String! @sortable
  posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
}

type Post
  @node
  @mutation
  @subscription
  @query(aggregate: true)
  @authentication(operations: [CREATE, UPDATE, DELETE, SUBSCRIBE])
  @authorization(
    filter: [
      { operations: [READ], where: { node: { published: { eq: true } } } }
      { where: { node: { author: { isViewer: true } } } }
    ]
    validate: [
      {
        operations: [CREATE]
        where: { node: { author: { isViewer: true } } }
      }
    ]
  ) {
  slug: String! @key
  title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
  body: String
  published: Boolean! @default(value: false) @filterable
  author: Person!
    @relationship(type: "WROTE", direction: IN)
    @settable(onUpdate: false)
    @filterable
  tags: [Tag!]!
    @relationship(
      type: "TAGGED"
      direction: OUT
      nestedOperations: [CONNECT, DISCONNECT]
    )
    @filterable
    @cardinality(max: 10)
}

type Tag @node {
  name: String! @key
  posts: [Post!]! @relationship(type: "TAGGED", direction: IN)
}
```

What changed:

- **`@viewer`** on the `sub` claim says it is the key of the caller's
  `Person`. Rules can now say "the caller" with `isViewer`.
- **The filter** decides which posts exist for a caller. A post is
  readable when it is published, and readable, editable and deletable
  when the caller wrote it. For anyone else it is simply not there.
- **The validate rule** covers creating, where there is no row to filter
  yet: a new post's author must be the caller.
- **`@settable(onUpdate: false)`** on `author` keeps an update from
  handing a post to someone else.
- **`requireAuthentication: false`** lets the "published" rule, which
  needs no claim, apply to anonymous callers too. Without it, rules deny
  everyone who has no token.
- **`@subscription`** is for the next step.

Restart and try the three holes from before. An anonymous caller sees
published posts only, and the draft is gone:

```graphql title="Anonymous"
{
  posts(sort: [{ slug: ASC }]) {
    slug
  }
  post(slug: "notes") {
    title
  }
}
```

```json
{
  "data": {
    "posts": [{ "slug": "cobol" }, { "slug": "engines" }],
    "post": null
  }
}
```

Ada sees her draft, and the count of what she can see follows:

```graphql title="As ada"
{
  posts(sort: [{ slug: ASC }]) {
    slug
    published
  }
  postsAggregate {
    count
  }
}
```

```json
{
  "data": {
    "posts": [
      { "slug": "cobol", "published": true },
      { "slug": "engines", "published": true },
      { "slug": "notes", "published": false }
    ],
    "postsAggregate": { "count": 3 }
  }
}
```

Ada cannot delete Grace's post any more. To her it is readable but not
deletable, so the delete finds nothing to delete:

```graphql title="As ada"
mutation {
  deletePost(slug: "cobol") {
    nodesDeleted
  }
}
```

```json
{ "data": { "deletePost": { "nodesDeleted": 0 } } }
```

She cannot publish in Grace's name:

```graphql title="As ada"
mutation {
  createPosts(
    input: [
      {
        slug: "forged"
        title: "Grace never wrote this"
        author: { connect: { key: "grace" } }
      }
    ]
  ) {
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
      "message": "not allowed to create this Post",
      "extensions": { "code": "FORBIDDEN" }
    }
  ]
}
```

And she can still do everything with her own:

```graphql title="As ada"
mutation {
  updatePost(
    slug: "notes"
    update: { published: true, tags: { connect: [{ name: "graphs" }] } }
  ) {
    post {
      slug
      published
      tags {
        name
      }
    }
  }
}
```

```json
{
  "data": {
    "updatePost": {
      "post": {
        "slug": "notes",
        "published": true,
        "tags": [{ "name": "graphs" }]
      }
    }
  }
}
```

The rules are not a layer in front of the database. They are compiled
into each statement: the filter becomes part of the `WHERE` clause, so
lists, counts, nested relationships and paging all obey it, and there is
no query that forgets to.

## Step 6: live updates

`@subscription` on `Post` generated a `postChanged` subscription. Yoga
serves subscriptions over server-sent events with no extra setup. Listen
in one terminal:

```bash
curl -N http://localhost:4000/graphql \
  -H "authorization: Bearer $(npx tsx token.ts ada)" \
  -H 'accept: text/event-stream' \
  -H 'content-type: application/json' \
  -d '{"query":"subscription { postChanged { operation slug node { title } } }"}'
```

and create a post as Ada in another. The first terminal prints:

```text
event: next
data: {"data":{"postChanged":{"operation":"CREATE","slug":"live","node":{"title":"Live"}}}}
```

Subscribers receive only the posts they are allowed to read: the same
filter applies to every event. The schema requires a signed-in caller to
subscribe (`SUBSCRIBE` in `@authentication`).

For WebSockets, the project's
[`ws.ts`](https://github.com/lora-db/lora/blob/main/packages/lora-graphql/examples/tutorial/ws.ts)
serves the same schema with `graphql-ws`, taking the token from the
connection parameters.

## Step 7: prove it

Two commands turn "I think the rules are right" into something CI
checks.

**The gate.** `lora-graphql check` builds the schema against an
in-memory database and fails on unguarded writes, on indexes the API
would need and not have, and on custom Cypher the engine rejects. It
passes now:

```bash
npm run check
```

**The matrix.** `lora-graphql access` prints who may do what:

```bash
npx lora-graphql access schema.graphql
```

```text
Person  READ    anonymous      allowed          no rule
Person  READ    authenticated  allowed          no rule
Person  READ    viewer         allowed          no rule
Post    READ    anonymous      filtered         filter[0], filter[1]
Post    READ    authenticated  filtered         filter[0], filter[1]
Post    READ    viewer         allowed          filter[0], filter[1]
Post    CREATE  anonymous      unauthenticated  @authentication, validate[0]
Post    CREATE  authenticated  validated        @authentication, validate[0]
Post    CREATE  viewer         allowed          @authentication, validate[0]
Post    UPDATE  anonymous      unauthenticated  @authentication, filter[1]
Post    UPDATE  authenticated  filtered         @authentication, filter[1]
Post    UPDATE  viewer         allowed          @authentication, filter[1]
Post    DELETE  anonymous      unauthenticated  @authentication, filter[1]
Post    DELETE  authenticated  filtered         @authentication, filter[1]
Post    DELETE  viewer         allowed          @authentication, filter[1]
Tag     READ    anonymous      allowed          no rule
Tag     READ    authenticated  allowed          no rule
Tag     READ    viewer         allowed          no rule
```

`filtered` means "only the rows the rules admit", `validated` "allowed
when the rule holds", and `viewer` is the caller on rows that are
theirs. Commit this output and diff it in CI, and a change in who can do
what becomes visible in review.

**A test.** State the model as a table and run it against a real
database. Each probe runs in a transaction that is rolled back.

```ts title="access.test.ts" file="access.test.ts"
// The access model, stated as a table and run against a real database.
// Each probe runs in a transaction that is rolled back.

import { readFile } from "node:fs/promises";
import { test } from "node:test";
import {
  createTestLoraGraphQL,
  expectAccess,
} from "@loradb/lora-graphql/testing";

const file = (name: string) => readFile(new URL(name, import.meta.url), "utf8");

test("posts: published for everyone, drafts for their author", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: await file("./schema.graphql"),
    seed: await file("./seed.cypher"),
  });

  await expectAccess(t, {
    as: undefined, // an anonymous caller
    allowed: ["read Post engines"],
    denied: ["read Post notes", "delete Post engines"],
  });

  await expectAccess(t, {
    as: { sub: "ada" },
    allowed: ["read Post notes", "update Post notes", "delete Post notes"],
    denied: ["update Post cobol", "delete Post cobol"],
  });

  await expectAccess(t, {
    as: { sub: "grace" },
    allowed: ["read Post engines", "update Post cobol"],
    denied: ["read Post notes", "update Post engines"],
  });

  t.close();
});
```

```bash
npx tsx --test access.test.ts
```

## Step 8: before production

The API works and is guarded. Five settings remain between this and a
deployment:

| Do | Why |
| --- | --- |
| Set `JWT_SECRET` from a secret store, or verify against your provider's key set | The development secret in `auth.ts` is public |
| Run with `NODE_ENV=production` | Masks database errors and turns introspection off |
| Set `CURSOR_SECRET` | Signs pagination cursors so clients cannot forge them |
| Open a persistent database: `createDatabase("blog", { databaseDir: "./data" })` | The in-memory database is empty on every start |
| Run `npm run check` in CI | Fails the build on an unguarded write or a missing index |

With a persistent database, remove the seed from `lora.ts`, or it will
try to create the same nodes on every start and fail on the key
constraint.

[Authentication](/docs/graphql/authentication) covers verifying tokens
from an identity provider, and
[limits and scaling](/docs/graphql/limitations) what to expect under
load.

## Where to go next

- [Queries and mutations by example](/docs/graphql/examples): forty more
  requests, from paging to bulk writes.
- [Authorization recipes](/docs/graphql/authorization-recipes): roles,
  tenants, teams and private fields, in the same style.
- [Relationships](/docs/graphql/relationships): everything a
  `@relationship` generates.
- [@cypher fields](/docs/graphql/cypher-fields): when the generated API
  is not enough.
- [Serving the schema](/docs/graphql/serving): Apollo Server, your own
  handler, WebSockets and response caching.
