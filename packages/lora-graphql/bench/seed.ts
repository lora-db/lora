// The bench graph: 20k festivals in 10 genres, 2k users following 50
// festivals each (100k FOLLOWS). Shared by the read and mutation benches.

import { createDatabase } from "@loradb/lora-node";
import {
  LoraGraphQL,
  loraDriver,
  type LoraGraphQLOptions,
} from "../src/index.js";
import { appTypeDefs } from "../test/fixtures.js";

export async function seed(options: Partial<LoraGraphQLOptions> = {}) {
  const FESTIVALS = 20_000;
  const USERS = 2_000;
  const FOLLOWS_PER_USER = 50; // 100k relationships

  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: loraDriver(db),
    ...options,
  });
  await lora.assertSchema({ create: true });
  await db.execute(
    `UNWIND range(0, $n - 1) AS i
     CREATE (:Festival { key: 'f' + toString(i), name: 'Festival ' + toString(i), capacity: i % 5000 })`,
    { n: FESTIVALS },
  );
  await db.execute(
    `UNWIND range(0, 9) AS g CREATE (:Genre { key: 'g' + toString(g), name: 'Genre ' + toString(g) })`,
  );
  await db.execute(
    `MATCH (f:Festival), (g:Genre)
     WHERE g.key = 'g' + toString(toInteger(substring(f.key, 1)) % 10)
     CREATE (f)-[:IN_GENRE]->(g)`,
  );
  await db.execute(
    `UNWIND range(0, $n - 1) AS i CREATE (:User { key: 'u' + toString(i), name: 'User ' + toString(i) })`,
    { n: USERS },
  );
  await db.execute(
    `UNWIND range(0, $users - 1) AS u
     UNWIND range(0, $per - 1) AS j
     MATCH (a:User) WHERE a.key = 'u' + toString(u)
     MATCH (f:Festival) WHERE f.key = 'f' + toString((u * 7919 + j * 104729) % $festivals)
     CREATE (a)-[:FOLLOWS { since: 2000 + j }]->(f)`,
    { users: USERS, per: FOLLOWS_PER_USER, festivals: FESTIVALS },
  );
  return { db, lora };
}
