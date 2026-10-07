// GraphQL Yoga serving the schema. Run: npm start

import { createServer } from "node:http";
import { GraphQLError } from "graphql";
import { createYoga } from "graphql-yoga";
import { verifiedClaims } from "./auth.js";
import { lora } from "./lora.js";

const yoga = createYoga({
  schema: lora.getSchema(),
  // Document guards: depth, aliases, root fields, tokens, introspection.
  plugins: [lora.envelopPlugin()],
  context: async ({ request }) => {
    try {
      return {
        jwt: await verifiedClaims(request.headers.get("authorization")),
        signal: request.signal,
      };
    } catch {
      throw new GraphQLError("Invalid or expired token", {
        extensions: { code: "UNAUTHENTICATED", http: { status: 401 } },
      });
    }
  },
});

const port = Number(process.env.PORT ?? 4000);
createServer(yoga).listen(port, () => {
  console.log(`listening on http://localhost:${port}/graphql`);
});
