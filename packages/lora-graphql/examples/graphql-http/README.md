# graphql-http example

[graphql-http](https://github.com/graphql/graphql-http), the reference
GraphQL over HTTP server, on `node:http`, serving a `LoraGraphQL` schema
with:

- the package's document guards: `lora.validationRules()` (graphql-http
  appends them to the standard rules) and `parseOptions()` in a custom
  `parse` for the lexer token limit;
- JWT verification in the server (`jose`): an invalid token is answered
  with HTTP 401 before anything runs;
- request cancellation through an `AbortSignal` in the context.

## Run

See [the examples README](../README.md#setup) for the one-time build, then:

```sh
npm install
npm start # http://localhost:4000/graphql
```

`PORT`, `JWT_SECRET` and `CURSOR_SECRET` are read from the environment.
`npm run token` prints a development token with the `editor` role, which
the schema requires for writes.

## Notes

- If you configure guards on `LoraGraphQL` (`guards: { maxDepth: 8 }`),
  pass the same object to `parseOptions()`: `lora.validationRules()`
  follows the instance's guards, but `parseOptions` is a plain function.
- graphql-http parses every request, so each request produces new field
  nodes and the library's per-field compile cache does not apply. For
  first-party clients, `lora.execute({ id })` with persisted operations
  skips parsing and validation and hits the cache; it can sit behind a
  small route of your own next to this handler.
- graphql-http does not serve subscriptions.
