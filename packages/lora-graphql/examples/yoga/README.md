# GraphQL Yoga example

GraphQL Yoga 5 serving a `LoraGraphQL` schema, with:

- the package's document guards through `lora.envelopPlugin()`: depth,
  aliases, root fields, lexer tokens, and introspection off in production;
- JWT verification in the server (`jose`), with only verified claims in
  the context;
- a response cache (`@graphql-yoga/plugin-response-cache`) invalidated by
  `lora.onWrite`;
- subscriptions over server-sent events (`festivalChanged`), which Yoga
  serves without extra setup.

## Run

See [the examples README](../README.md#setup) for the one-time build, then:

```sh
npm install
npm start # http://localhost:4000/graphql
```

`PORT`, `JWT_SECRET` and `CURSOR_SECRET` are read from the environment.
`npm run token` prints a development token with the `editor` role, which
the schema requires for writes.

## Response cache invalidation

The cache plugin tags each cached result with the objects it contains,
identified by their `id` (the schema adds one with `@relayId`), and with
the type of every empty list. `lora.onWrite` receives the write-set of
every committed mutation made through the library, and the server
invalidates every cached result that holds a node of a touched type:

```ts
lora.onWrite((change) => {
  const types = change.broad ? nodeTypes : change.types;
  void cache.invalidate(types.map((typename) => ({ typename })));
});
```

Choices worth knowing:

- **Per type, not per entity.** Invalidating only `change.entities` (by
  type and key) is finer, but misses a cached, filtered list that an
  updated or created node now matches. Per type is always correct for
  results that contain the type.
- **Counts and aggregates are not cached.** A result holding only
  `totalCount` or an aggregate contains no node object to tag, so no write
  could invalidate it. The server sets a TTL of 0 for those fields.
- **Per token.** Authorization rules make results differ per caller, so
  the cache is keyed by the `authorization` header.
- **A TTL backstop (30 s).** `onWrite` only sees writes made through the
  library in this process. Writes from `db.execute()`, another process or
  your own Cypher in a transaction, and `@cypher` fields that read types a
  write did not touch, are only refreshed when the TTL expires.
- **`invalidateViaMutation: false`.** `onWrite` already covers every
  write, including both ends of a connect and writes committed through
  `lora.begin()` transactions.

## Subscriptions

```sh
curl -N -H 'accept: text/event-stream' \
  'http://localhost:4000/graphql?query=subscription%20%7B%20festivalChanged%20%7B%20operation%20key%20node%20%7B%20name%20%7D%20%7D%20%7D'
```

Events come from mutations made through this server process only.
