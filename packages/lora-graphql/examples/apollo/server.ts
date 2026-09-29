// Apollo Server serving a LoraGraphQL schema, with the package's document
// guards as validation rules and parse options. Run: npm start (see
// README.md).

import { readFile } from "node:fs/promises";
import { ApolloServer } from "@apollo/server";
import { startStandaloneServer } from "@apollo/server/standalone";
import { createDatabase } from "@loradb/lora-node";
import {
  LoraGraphQL,
  loraDriver,
  parseOptions,
  type LoraGraphQLContext,
} from "@loradb/lora-graphql";
import { GraphQLError } from "graphql";
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
async function verifiedClaims(header: string | undefined) {
  if (!header) return undefined; // anonymous
  const token = /^Bearer (.+)$/.exec(header)?.[1];
  try {
    if (!token) throw new Error("not a bearer token");
    const { payload } = await jwtVerify(token, secret, {
      algorithms: ["HS256"],
    });
    return payload as Record<string, unknown>;
  } catch {
    throw new GraphQLError("invalid token", {
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

const server = new ApolloServer<LoraGraphQLContext>({
  schema: lora.getSchema(),
  // Depth, alias and root-field limits, and no introspection in
  // production (Apollo also turns it off there by default).
  validationRules: lora.validationRules(),
  // The lexer token limit, applied while parsing. Pass the same guard
  // options you give LoraGraphQL, if any.
  parseOptions: parseOptions(),
});

const { url } = await startStandaloneServer(server, {
  listen: { port },
  context: async ({ req, res }) => {
    // Cancel the request's statements when the client goes away.
    const controller = new AbortController();
    res.once("close", () => controller.abort());
    const jwt = await verifiedClaims(req.headers.authorization);
    return { ...(jwt ? { jwt } : {}), signal: controller.signal };
  },
});

console.log(`Apollo Server on ${url}`);
