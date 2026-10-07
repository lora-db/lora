// Apollo Server serving the same schema. Run: npx tsx apollo.ts

import { ApolloServer } from "@apollo/server";
import { startStandaloneServer } from "@apollo/server/standalone";
import { parseOptions } from "@loradb/lora-graphql";
import { GraphQLError } from "graphql";
import { verifiedClaims } from "./auth.js";
import { lora } from "./lora.js";

const server = new ApolloServer({
  schema: lora.getSchema(),
  // Apollo has no Envelop: wire the document guards in by hand.
  validationRules: lora.validationRules(),
  parseOptions: parseOptions(),
});

const { url } = await startStandaloneServer(server, {
  listen: { port: Number(process.env.PORT ?? 4000) },
  context: async ({ req }) => {
    try {
      return { jwt: await verifiedClaims(req.headers.authorization) };
    } catch {
      throw new GraphQLError("Invalid or expired token", {
        extensions: { code: "UNAUTHENTICATED", http: { status: 401 } },
      });
    }
  },
});
console.log(`listening on ${url}`);
