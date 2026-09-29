// GraphQL Yoga serving a LoraGraphQL schema, with the package's document
// guards as an Envelop plugin and a response cache invalidated by
// lora.onWrite. Run: npm start (see README.md).

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";
import {
  createInMemoryCache,
  useResponseCache,
} from "@graphql-yoga/plugin-response-cache";
import { isObjectType } from "graphql";
import { createGraphQLError, createYoga } from "graphql-yoga";
import { jwtVerify } from "jose";

const port = Number(process.env.PORT ?? 4000);
const production = process.env.NODE_ENV === "production";
if (production && !process.env.JWT_SECRET) {
  throw new Error("set JWT_SECRET in production");
}
const secret = new TextEncoder().encode(
  process.env.JWT_SECRET ?? "dev-only-secret-do-not-use",
);

// The library never verifies tokens: the server does, and puts only the
// verified claims in the context.
async function verifiedClaims(request: Request) {
  const header = request.headers.get("authorization");
  if (!header) return undefined; // anonymous
  const token = /^Bearer (.+)$/.exec(header)?.[1];
  try {
    if (!token) throw new Error("not a bearer token");
    const { payload } = await jwtVerify(token, secret, {
      algorithms: ["HS256"],
    });
    return payload as Record<string, unknown>;
  } catch {
    throw createGraphQLError("invalid token", {
      extensions: { code: "UNAUTHENTICATED", http: { status: 401 } },
    });
  }
}

const typeDefs = await readFile(
  new URL("./schema.graphql", import.meta.url),
  "utf8",
);
const db = await createDatabase();
const lora = new LoraGraphQL({
  typeDefs,
  driver: loraDriver(db),
  // Sign cursors so clients cannot forge them. Set it from a secret store.
  ...(process.env.CURSOR_SECRET
    ? { cursorSecret: process.env.CURSOR_SECRET }
    : {}),
  // maskErrors and introspection follow NODE_ENV by default.
});
await lora.assertSchema({ create: true });

await db.execute(
  "CREATE (:Genre {key: 'techno', name: 'Techno'}), (:Genre {key: 'jazz', name: 'Jazz'})",
);

const schema = lora.getSchema();

// Response cache. The plugin tags each cached result with the objects in
// it, identified by their `id` (the schema gives node types one with
// @relayId), and with the type of every empty list. Objects without an id
// are not tagged. Results depend on the caller's claims, so the cache is
// keyed per token (`session`); anonymous requests share one cache.
const cache = createInMemoryCache();

// Invalidate on every committed write made through the library: generated
// mutations, nested writes (both ends of a connect) and writes inside
// lora.begin() transactions, once they commit. Invalidation is per type:
// invalidating only change.entities (by type and key) would miss a cached,
// filtered list that an updated or created node now matches.
const nodeTypes = [...lora.model.nodes.keys()];
lora.onWrite((change) => {
  const types = change.broad ? nodeTypes : change.types;
  void cache.invalidate(types.map((typename) => ({ typename })));
});

// Counts and aggregates hold no node object to tag, so no write could
// invalidate them: never cache a document that selects one.
const uncached: Record<string, number> = {};
for (const type of Object.values(schema.getTypeMap())) {
  if (!isObjectType(type) || type.name.startsWith("__")) continue;
  for (const field of Object.keys(type.getFields())) {
    if (/^(totalCount|aggregate)$|(Aggregate|Grouped)$/.test(field)) {
      uncached[`${type.name}.${field}`] = 0;
    }
  }
}

const yoga = createYoga({
  schema,
  plugins: [
    // Depth, alias, root-field and token limits; introspection off in
    // production.
    lora.envelopPlugin(),
    useResponseCache({
      cache,
      session: (request) => request.headers.get("authorization"),
      ttlPerSchemaCoordinate: uncached,
      // onWrite covers every write, so skip the plugin's own
      // mutation-result based invalidation.
      invalidateViaMutation: false,
      // A backstop for what onWrite cannot see: writes made outside the
      // library (db.execute, other processes) and @cypher fields that read
      // types a write did not touch.
      ttl: 30_000,
    }),
  ],
  context: async ({ request }) => ({
    jwt: await verifiedClaims(request),
    signal: request.signal,
  }),
});

createServer(yoga).listen(port, () => {
  console.log(`GraphQL Yoga on http://localhost:${port}/graphql`);
});
