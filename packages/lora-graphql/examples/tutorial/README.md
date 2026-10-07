# Tutorial example

The finished project of the
[tutorial](https://loradb.com/docs/graphql/tutorial): a small blog API
where anyone reads published posts and authors manage their own. The
[authentication](https://loradb.com/docs/graphql/authentication) page
quotes its servers.

| File             | What it is                                                  |
| ---------------- | ----------------------------------------------------------- |
| `schema.graphql` | The annotated SDL: the graph, the API and the access rules  |
| `seed.cypher`    | Data loaded at every start (the database is in memory)      |
| `lora.ts`        | Opens the database and builds the `LoraGraphQL` object      |
| `auth.ts`        | Verifies a bearer token signed with a shared secret (HS256) |
| `auth-jwks.ts`   | The same against an identity provider's published keys      |
| `token.ts`       | Mints a development token                                   |
| `server.ts`      | GraphQL Yoga, the server the tutorial builds                |
| `apollo.ts`      | Apollo Server over the same schema                          |
| `handler.ts`     | `node:http` and `lora.execute()`, with no server library    |
| `ws.ts`          | Yoga for HTTP plus `graphql-ws` subscriptions on one port   |
| `access.test.ts` | The access model as an `expectAccess` table                 |
| `test.ts`        | Starts every server and calls it with real signed tokens    |

## Run

See [the examples README](../README.md#setup) for the one-time build, then:

```sh
npm install
npm start          # http://localhost:4000/graphql
npm run token ada  # a token for the person with key "ada"
npm run check      # the CI gate
npm test
```

`PORT`, `JWT_SECRET`, `JWT_ISSUER`, `JWT_AUDIENCE`, `JWKS_URL` and
`CURSOR_SECRET` are read from the environment. The defaults are for
development only: the default secret is public.

## What the test covers

`npm test` starts `server.ts`, `apollo.ts` and `handler.ts` in turn and
holds each to the same contract over HTTP:

- an anonymous caller reads published posts only;
- a signed-in author also reads their own draft;
- an anonymous write is `UNAUTHENTICATED`;
- an author cannot create a post in someone else's name (`FORBIDDEN`);
- an expired token, a token for another audience, a token signed with
  another key and a malformed token each get a `401`, never an anonymous
  response.

It then subscribes over WebSocket through `ws.ts` and receives a post
created over HTTP, checks that a bad token cannot open a connection,
verifies a token through `auth-jwks.ts` against a key set it serves
locally, and runs `lora-graphql check` on the schema.

## Keeping the docs honest

The website quotes these files. `packages/lora-graphql/test/docs.test.ts`
fails when a quoted block no longer matches its file, and runs every
request on the tutorial page against the schema of its step. Change a
file here and that test tells you which page to update.
