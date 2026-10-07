# @loradb/lora-graphql examples

Small servers that serve one `LoraGraphQL` schema each:

| Example                           | Server               | Shows                                                                 |
| --------------------------------- | -------------------- | --------------------------------------------------------------------- |
| [`yoga/`](./yoga)                 | GraphQL Yoga 5       | `envelopPlugin()` guards, a response cache invalidated by `onWrite`   |
| [`apollo/`](./apollo)             | Apollo Server 5      | `validationRules()` and `parseOptions()` guards, request cancellation |
| [`graphql-http/`](./graphql-http) | graphql-http on node | The same guards with the reference GraphQL over HTTP server           |
| [`tutorial/`](./tutorial)         | Yoga, Apollo, node   | The website tutorial's finished project, with a test of every server  |

The first three verify a JWT in the server and put only the verified claims in
the context, seed two genres, and let a token with the `editor` role
create, update and delete festivals.

## Setup

The examples are not Yarn workspaces: each has its own `package.json` and
installs with npm, so the monorepo lockfile is unaffected.
`@loradb/lora-graphql` comes from this checkout (`file:../..`), so build it
first:

```sh
# from the repository root
corepack yarn install --immutable
corepack yarn workspace @loradb/lora-graphql build

cd packages/lora-graphql/examples/yoga # or apollo, graphql-http
npm install
npm start
```

Each example's `.npmrc` sets `install-links=true`, so npm installs the
package as a copy instead of a symlink. A symlinked copy would load
`graphql` from the monorepo and the server's from the example, and
graphql-js refuses to mix two instances. Rebuild and run `npm install`
again after changing the package.

`@loradb/lora-node` comes from npm. The two packages are released in
lockstep: between releases this checkout can depend on engine features
the latest published binding does not have yet, and the server then fails
at startup. To run against the binding in this checkout instead (needs a
Rust toolchain):

```sh
# from the repository root
corepack yarn workspace @loradb/lora-node build

cd packages/lora-graphql/examples/yoga
rm -rf node_modules/@loradb/lora-node
ln -s ../../../../../../crates/bindings/lora-node node_modules/@loradb/lora-node
```

## Try it

```sh
npm run token # prints a development token with the editor role

curl -s localhost:4000/graphql -H 'content-type: application/json' \
  -H "authorization: Bearer $(npm run -s token)" \
  -d '{"query":"mutation { createFestivals(input: [{ name: \"Sunland\", capacity: 5000, genre: { connect: { key: \"techno\" } } }]) { festivals { key name } } }"}'

curl -s localhost:4000/graphql -H 'content-type: application/json' \
  -d '{"query":"{ festivals(sort: [{ name: ASC }]) { key name genre { name } } }"}'
```

Apollo Server serves on `/` instead of `/graphql`.

## Production notes

- Set `JWT_SECRET` (the servers refuse to start in production without
  one) and use your identity provider's tokens and verification options
  (issuer, audience, key set) instead of `token.ts`.
- Set `CURSOR_SECRET` so cursors are signed.
- Run with `NODE_ENV=production`: database errors are masked and
  introspection is off.
- The database is in memory and starts empty on every restart.
