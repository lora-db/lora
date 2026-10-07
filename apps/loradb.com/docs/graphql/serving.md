---
title: Serving a LoraGraphQL Schema
sidebar_label: Serving
description: Serve @loradb/lora-graphql from GraphQL Yoga, Apollo Server, graphql-http, graphql-ws or your own handler, with the request context, document guards, cancellation, subscriptions and response caching wired in.
---

# Serving the schema

`@loradb/lora-graphql` is not a server. It gives you a standard
`GraphQLSchema` and an `execute()` helper, and your server does the HTTP,
the WebSocket and the token verification. This page shows the three
things every server needs (the context, the document guards and
cancellation) and how to wire them into the common servers.

The package ships runnable versions of these servers in
[`examples/`](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples).

## Two ways to run operations

| | `lora.getSchema()` | `lora.execute()` / `lora.subscribe()` |
| --- | --- | --- |
| Use with | Any `graphql-js` server | Your own request handler |
| Document guards | Wire them in (see [below](#document-guards)) | Applied |
| Parsed document cache | The server's, if any | Built in, by source text (500 documents) |
| Persisted operations | The server's mechanism | `persist()`, `loadManifest()`, `persistedOnly` |
| Cost in `extensions.cost`, timing in `extensions.timing` | No | Yes |
| Variable errors coded `BAD_USER_INPUT` | No | Yes |
| [Repeated errors collapsed](/docs/graphql/errors#repeated-errors) | With `lora.envelopPlugin()` | Yes |
| `mutationTransaction: "operation"` | With `lora.envelopPlugin()`, or a `transaction` in the context | Yes |
| Read-sets for cache invalidation (`readSet: true`) | No | Yes |
| Subscriptions | Yes, through the server's `subscribe` | `subscribe()`; `execute()` runs queries and mutations |

Both run the same resolvers, so authorization, limits, cost checks and
change tracking behave the same either way.

## The request context

Every operation reads these keys from the GraphQL context value:

| Key | Type | Meaning |
| --- | --- | --- |
| `jwt` | object | The caller's **verified** claims. Absent means anonymous |
| `signal` | `AbortSignal` | Cancels the request's statements, and ends its subscriptions, when aborted |
| `transaction` | `LoraTransaction` | Run every operation of the request in this transaction, from `lora.begin()` |

Any other key is yours. Authorization rules can read it with
`"$context.path"` strings, for example a tenant id your server resolved
from the host name. See [authorization](/docs/graphql/authorization).

If your claims live somewhere other than `context.jwt`, tell the library
where:

```ts
const lora = new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  jwt: (context) => (context as { auth?: { claims?: Record<string, unknown> } }).auth?.claims,
});
```

:::warning Verify tokens in the server
The library never verifies a token. Put only claims your server has
verified (signature, issuer, audience, expiry) in the context. Copying
decoded but unverified claims into `jwt` grants whatever they say.
:::

## Document guards

The document guards bound the document before anything is compiled:
nesting depth (12), aliases (30), root fields (20), lexer tokens (5000),
and introspection (off when `NODE_ENV` is `production`). `execute()`,
`subscribe()` and `persist()` apply them. Every other server needs them wired in, and the
table shows how:

| Server | Wiring |
| --- | --- |
| GraphQL Yoga, other Envelop servers | `plugins: [lora.envelopPlugin()]` |
| Apollo Server | `validationRules: lora.validationRules()`, `parseOptions: parseOptions(guards)` |
| graphql-http | `validationRules: lora.validationRules()`, `parse: (s) => parse(s, parseOptions(guards))` |
| Plain `graphql-js` | `validate(schema, doc, [...specifiedRules, ...lora.validationRules()])`, `parse(source, parseOptions(guards))` |

`lora.validationRules()` and `lora.envelopPlugin()` use the `guards`
option you passed to `new LoraGraphQL`. `parseOptions(guards)` is a
standalone export: pass it the same `guards` object (or nothing, for the
defaults). With `guards: false`, `validationRules()` returns no rules and
`envelopPlugin()` applies no limits.

A depth, alias or root field failure is `LIMIT_EXCEEDED`, and a blocked
introspection query is `FORBIDDEN`. The token limit stops the parser, so a
document over it comes back as a syntax error without a code.

`lora.envelopPlugin()` also checks, once, that the server executes with
the same copy of `graphql` the library loaded, and logs an error when it
does not. Two copies in `node_modules` make schema checks fail in ways
that are hard to trace; `npm ls graphql` shows them.

## Cancellation and timeouts

Every statement runs under `timeoutMs` (default 10 000 ms). Put an
`AbortSignal` in the context too, so a client that disconnects stops its
statements instead of letting them run to the timeout:

- GraphQL Yoga: `signal: request.signal`.
- Node `http` servers: create an `AbortController` and abort it on the
  response's `close` event, as in the examples below.

The signal also ends the request's subscription streams.

Two more limits keep one request from holding the database:

- **`operationTimeoutMs`** (default twice `timeoutMs`) is a deadline for
  all root fields of a query together. Past it, running statements are
  aborted and unfinished fields fail with `TIMEOUT`. Without it, twenty
  aliased root fields could run for twenty times `timeoutMs`.
- **`maxConcurrentStatements`** (default 2) is how many statements one
  operation runs at once. Reads run on Node's worker pool, four threads
  by default; this keeps one request's aliases from taking all of them.

A write also waits for LoraDB's single writer lock, for at most
`timeoutMs`, and then fails with `DATABASE_ERROR`.

## GraphQL Yoga

A complete, tested project with each of the servers below is in
[`examples/tutorial`](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples/tutorial),
and the [tutorial](/docs/graphql/tutorial) builds it step by step.

```ts title="server.ts"
import { createServer } from "node:http";
import { createYoga } from "graphql-yoga";

const yoga = createYoga({
  schema: lora.getSchema(),
  plugins: [lora.envelopPlugin()],
  context: async ({ request }) => ({
    jwt: await verifiedClaims(request),
    signal: request.signal,
  }),
});

createServer(yoga).listen(4000);
```

Yoga serves subscriptions over server-sent events with no extra setup.

## Apollo Server

```ts title="server.ts"
import { ApolloServer } from "@apollo/server";
import { startStandaloneServer } from "@apollo/server/standalone";
import { parseOptions, type LoraGraphQLContext } from "@loradb/lora-graphql";

const server = new ApolloServer<LoraGraphQLContext>({
  schema: lora.getSchema(),
  validationRules: lora.validationRules(),
  parseOptions: parseOptions(),
});

await startStandaloneServer(server, {
  listen: { port: 4000 },
  context: async ({ req, res }) => {
    const controller = new AbortController();
    res.once("close", () => controller.abort());
    const jwt = await verifiedClaims(req.headers.authorization);
    return { ...(jwt ? { jwt } : {}), signal: controller.signal };
  },
});
```

Apollo Server serves on `/`, not `/graphql`.

## graphql-http

```ts title="server.ts"
import { createServer } from "node:http";
import { parse } from "graphql";
import { createHandler } from "graphql-http/lib/use/http";
import { parseOptions } from "@loradb/lora-graphql";

const handler = createHandler({
  schema: lora.getSchema(),
  parse: (source) => parse(source, parseOptions()),
  validationRules: lora.validationRules(), // appended to the standard rules
  context: async (req) => {
    const controller = new AbortController();
    req.context.res.once("close", () => controller.abort());
    const jwt = await verifiedClaims(req.raw.headers.authorization);
    return { ...(jwt ? { jwt } : {}), signal: controller.signal };
  },
});

createServer((req, res) => {
  if (req.url?.startsWith("/graphql")) handler(req, res);
  else res.writeHead(404).end();
}).listen(4000);
```

graphql-http does not serve subscriptions.

## Your own handler with execute()

`execute()` is the shortest path when you want persisted operations only,
or when you already have an HTTP layer:

```ts
import { createServer } from "node:http";

createServer(async (req, res) => {
  const body = JSON.parse(await readBody(req)); // { id, variables } or { query, variables }
  const controller = new AbortController();
  res.once("close", () => controller.abort());

  const result = await lora.execute({
    id: body.id,
    source: body.query,
    variables: body.variables,
    operationName: body.operationName,
    context: { jwt: await verifiedClaims(req.headers.authorization), signal: controller.signal },
  });

  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(result));
}).listen(4000);
```

`execute()` takes either `id` (a persisted operation) or `source`. With
`persistedOnly: true` it refuses `source` with `PERSISTED_QUERY_ONLY`. The
result carries the operation's estimated cost in `extensions.cost`. See
[persisted operations](/docs/graphql#execute-and-persisted-operations)
and [`lora-graphql compile`](/docs/graphql/cli#compile).

A subscription runs with `subscribe()`, which takes the same arguments,
persisted `id` included, with the same document cache, guards and
`persistedOnly` rule. It returns an async iterable of results, one per
event, or a single result with the errors when the subscription cannot
start. Given a subscription, `execute()` returns a `WRONG_OPERATION_TYPE`
error, and so does `subscribe()` given a query or mutation. The cost limit
(`maxCost` or `budget`) applies to each event on its own, so a long-lived
subscription never runs out of it.

```ts
const stream = await lora.subscribe({
  id: body.id,
  variables: body.variables,
  context: { jwt, signal: controller.signal }, // aborting ends the stream
});
if (!(Symbol.asyncIterator in stream)) return send(stream); // it did not start
for await (const event of stream) sendEvent(event);
```

End a stream with the `signal` or with `return()` on the iterator (what
`for await` does on `break`). Either ends it at once, even while it waits
for an event that a quiet or filtered subscription may never get, and
drops its listener.

## Subscriptions over WebSockets

Subscription fields resolve through `graphql-js`'s `subscribe`, so any
transport that uses it works. With `graphql-ws`, pass the schema and build
the same context from the connection:

```ts
import { WebSocketServer } from "ws";
import { useServer } from "graphql-ws/use/ws";

useServer(
  {
    schema: lora.getSchema(),
    context: async (ctx) => ({
      jwt: await verifiedClaimsFromToken(ctx.connectionParams?.authorization),
    }),
  },
  new WebSocketServer({ server: httpServer, path: "/graphql" }),
);
```

`maxSubscriptions` (default 100) counts live subscriptions per context
object. `graphql-ws` calls a `context` function once per subscription, so
with the code above every subscription has its own context and the limit
never applies. Count per connection instead: put the connection in the
context (`connection: ctx`) and pass
`subscriptionScope: (context) => context.connection` to `new LoraGraphQL`.
Returning the user's id holder works the same way when a user may open
several connections.

`graphql-ws` does not apply the document guards itself. Validate
subscription documents with `[...specifiedRules, ...lora.validationRules()]`
in its `onSubscribe` hook if clients can send arbitrary documents.

What a subscriber sees depends on where events come from:

- By default, events come from mutations made through this `LoraGraphQL`
  instance. Writes made with raw Cypher, or by `@cypher` mutations (which
  have no known write-set), do not produce precise events.
- With `changeFeed: true` (`@loradb/lora-node` only), events come from
  the engine's committed change feed: every committed write to the
  database, whichever path made it (raw `db.execute()` Cypher, `@cypher`
  mutations, other `LoraGraphQL` instances on the same database), in
  commit order. The feed starts with the first subscriber and delivers
  commits from then on; it does not replay writes made before the
  process started.

A LoraDB database directory is open in one process at a time (a second
process gets `LORA_LOCKED`), so subscribers are served by the process
that owns the database. To fan events out across several server
processes, forward `lora.changes()` from that process to a pub/sub of
your choice, and apply read rules on the receiving side (the raw
write-set carries no authorization).

A subscriber that falls `maxQueuedChanges` (default 1000) events behind is
ended with `LIMIT_EXCEEDED` instead of buffering without bound. Clients
should resubscribe and reload. See
[subscriptions](/docs/graphql/generated-api#subscriptions).

## Response caching

Results depend on the caller's claims, so a response cache must be keyed
per caller (per token, or per set of claims). `lora.onWrite()` reports the
exact write-set of every committed mutation made through the library,
which is what a cache needs for invalidation:

```ts
const nodeTypes = [...lora.model.nodes.keys()];

lora.onWrite((change) => {
  const types = change.broad ? nodeTypes : change.types;
  void cache.invalidate(types.map((typename) => ({ typename })));
});
```

Three rules keep a cache correct:

- **Invalidate by type, not only by entity.** A created node, or an
  updated one that now matches a cached filter, changes lists that never
  held it. `change.entities` alone misses those.
- **Do not cache counts and aggregates.** `totalCount`, `aggregate`,
  `<plural>Aggregate` and `<plural>Grouped` hold no node to tag, so no
  write invalidates them.
- **Keep a TTL.** `onWrite` reports only this instance's mutations, even
  with `changeFeed: true`, so writes made with raw Cypher are invisible to
  it, and a `@cypher` field can read types a write did not touch. To see
  raw writes too, invalidate from `lora.changes()` with `changeFeed: true`
  instead.

The [Yoga example](https://github.com/lora-db/lora/tree/main/packages/lora-graphql/examples/yoga)
does all three with `@graphql-yoga/plugin-response-cache`. Types need a
`@relayId` field for the plugin to tag cached objects by `id`.

## Production checklist

- Verify tokens in the server; put only verified claims in `jwt`.
- Run with `NODE_ENV=production`: database errors are masked and
  introspection is off. Or set `maskErrors` and `guards.introspection`
  explicitly.
- Wire the document guards into the server.
- Pass a `signal` in the context.
- Set `cursorSecret` from a secret store so cursors are signed.
- Run `lora.assertSchema({ create: true })` at startup, or apply
  `lora-graphql requirements --ddl` in your migrations.
- Key any response cache per caller.
- Set `subscriptionScope` if your transport builds a context per
  subscription.
- Run `lora.analyze()` on production data, or declare `@cardinality`, so
  the cost limit reflects the real graph.
- Size the worker pool for your read load: see
  [limits and scaling](/docs/graphql/limitations#scaling).
- Call `lora.close()` on shutdown when `changeFeed` is on.

The [authorization](/docs/graphql/authorization#before-you-deploy) page
covers each of these in more depth.

## WASM and the browser

`loraDriver()` also wraps a `Database` from `@loradb/lora-wasm`. It serves
reads, including search and `@cypher` queries. It cannot serve:

- mutations, which need interactive transactions: they fail with
  `DATABASE_ERROR` ("mutations need a driver with interactive
  transactions");
- `explain()`, `check()` and `expectSeeks()`, which need the engine's
  planner through `@loradb/lora-node`;
- `changeFeed: true`.
