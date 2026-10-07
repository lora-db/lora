// The database and the GraphQL schema object, shared by every server in
// this directory.

import { readFile } from "node:fs/promises";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "@loradb/lora-graphql";

const file = (name: string) => readFile(new URL(name, import.meta.url), "utf8");

export const db = await createDatabase();

export const lora = new LoraGraphQL({
  typeDefs: await file("./schema.graphql"),
  driver: loraDriver(db),
  // Sign cursors in production, from a secret store.
  ...(process.env.CURSOR_SECRET
    ? { cursorSecret: process.env.CURSOR_SECRET }
    : {}),
  // Count live subscriptions per WebSocket connection (see ws.ts).
  subscriptionScope: (context) =>
    (context as { connection?: object }).connection,
});

// Create the constraints and indexes the API needs, then load some data.
await lora.assertSchema({ create: true });
await db.execute(await file("./seed.cypher"));
