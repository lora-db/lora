// Queries and mutations over HTTP with GraphQL Yoga, subscriptions over
// WebSocket with graphql-ws, on one port. Run: npx tsx ws.ts

import { createServer } from "node:http";
import { GraphQLError } from "graphql";
import { useServer } from "graphql-ws/use/ws";
import { createYoga } from "graphql-yoga";
import { WebSocketServer } from "ws";
import { verifiedClaims } from "./auth.js";
import { lora } from "./lora.js";

const yoga = createYoga({
  schema: lora.getSchema(),
  plugins: [lora.envelopPlugin()],
  context: async ({ request }) => {
    try {
      return {
        jwt: await verifiedClaims(request.headers.get("authorization")),
      };
    } catch {
      throw new GraphQLError("Invalid or expired token", {
        extensions: { code: "UNAUTHENTICATED", http: { status: 401 } },
      });
    }
  },
});

const httpServer = createServer(yoga);

// Browsers cannot set headers on a WebSocket, so the token travels in the
// connection parameters: createClient({ connectionParams: { authorization } }).
const claimsOf = (params: Record<string, unknown> | undefined) =>
  verifiedClaims(params?.authorization as string | undefined);

useServer(
  {
    schema: lora.getSchema(),
    // Refuse the connection itself for a bad token.
    onConnect: async (ctx) => {
      try {
        await claimsOf(ctx.connectionParams);
      } catch {
        return false;
      }
    },
    // One context per subscription. `connection` is what lora.ts counts
    // live subscriptions by (subscriptionScope).
    context: async (ctx) => ({
      jwt: await claimsOf(ctx.connectionParams),
      connection: ctx,
    }),
  },
  new WebSocketServer({ server: httpServer, path: "/graphql" }),
);

const port = Number(process.env.PORT ?? 4000);
httpServer.listen(port, () => {
  console.log(`listening on http://localhost:${port}/graphql`);
});
