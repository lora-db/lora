# Apollo Server example

Apollo Server 5 (standalone) serving a `LoraGraphQL` schema, with:

- the package's document guards: `lora.validationRules()` for depth,
  aliases, root fields and introspection, and `parseOptions()` for the
  lexer token limit;
- JWT verification in the server (`jose`), with only verified claims in
  the context;
- request cancellation: the context carries an `AbortSignal` that fires
  when the connection closes, so running statements stop.

Apollo Server 5 is the current major; 4 is end of life. The code is the
same for both: `ApolloServer`, `startStandaloneServer`, `validationRules`
and `parseOptions` have the same shape in each.

## Run

See [the examples README](../README.md#setup) for the one-time build, then:

```sh
npm install
npm start # http://localhost:4000/
```

`PORT`, `JWT_SECRET` and `CURSOR_SECRET` are read from the environment.
`npm run token` prints a development token with the `editor` role, which
the schema requires for writes.

## Notes

- If you configure guards on `LoraGraphQL` (`guards: { maxDepth: 8 }`),
  pass the same object to `parseOptions()`: `lora.validationRules()`
  follows the instance's guards, but `parseOptions` is a plain function.
- Apollo Server's standalone server does not serve subscriptions. Use
  GraphQL Yoga (see `../yoga`) or add `graphql-ws` for them.
- Apollo's response cache plugin caches by cache hints, not by the
  write-sets `lora.onWrite` reports. For write-driven invalidation, see the
  Yoga example.
