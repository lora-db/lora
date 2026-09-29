// graphql-http on node:http serving a LoraGraphQL schema, with the
// package's document guards as validation rules and parse options. Run:
// npm start (see README.md).

import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver, parseOptions } from "@loradb/lora-graphql";
import { GraphQLError, parse } from "graphql";
import { createHandler } from "graphql-http/lib/use/http";
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
      extensions: { code: "UNAUTHENTICATED" },
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

const handler = createHandler({
  schema: lora.getSchema(),
  // The lexer token limit, applied while parsing. Pass the same guard
  // options you give LoraGraphQL, if any.
  parse: (source) => parse(source, parseOptions()),
  // Depth, alias and root-field limits, and no introspection in
  // production. graphql-http appends an array to the standard rules.
  validationRules: lora.validationRules(),
  context: async (req) => {
    // Cancel the request's statements when the client goes away.
    const controller = new AbortController();
    req.context.res.once("close", () => controller.abort());
    const auth = req.raw.headers.authorization;
    let jwt: Record<string, unknown> | undefined;
    try {
      jwt = await verifiedClaims(auth);
    } catch (err) {
      // A Response returned from the context is sent as is.
      return [
        JSON.stringify({ errors: [(err as GraphQLError).toJSON()] }),
        {
          status: 401,
          statusText: "Unauthorized",
          headers: { "content-type": "application/json; charset=utf-8" },
        },
      ];
    }
    return { ...(jwt ? { jwt } : {}), signal: controller.signal };
  },
});

createServer((req, res) => {
  if (req.url?.startsWith("/graphql")) {
    handler(req, res);
  } else {
    res.writeHead(404).end();
  }
}).listen(port, () => {
  console.log(`graphql-http on http://localhost:${port}/graphql`);
});
