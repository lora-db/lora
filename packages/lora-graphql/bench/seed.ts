// The bench graph, per unit of scale: 20k festivals in 10 genres, 2k users
// following 50 festivals each (100k FOLLOWS). Shared by the read and
// mutation benches and the load harness (bench/load).

import { createDatabase } from "@loradb/lora-node";
import {
  LoraGraphQL,
  loraDriver,
  type LoraGraphQLOptions,
} from "../src/index.js";
import { appTypeDefs } from "../test/fixtures.js";

export interface GraphSize {
  festivals: number;
  users: number;
  followsPerUser: number;
}

export function graphSize(scale = 1): GraphSize {
  return {
    festivals: Math.round(20_000 * scale),
    users: Math.round(2_000 * scale),
    followsPerUser: 50,
  };
}

/** The subset of a lora-node `Database` seeding needs. */
interface Executor {
  execute(query: string, params?: never): Promise<unknown>;
}

/** Create the bench graph in `db`; the schema's indexes must exist first. */
export async function seedGraph(db: Executor, size: GraphSize): Promise<void> {
  const run = (query: string, params: Record<string, unknown> = {}) =>
    db.execute(query, params as never);
  await run(
    `UNWIND range(0, $n - 1) AS i
     CREATE (:Festival { key: 'f' + toString(i), name: 'Festival ' + toString(i), capacity: i % 5000 })`,
    { n: size.festivals },
  );
  await run(
    `UNWIND range(0, 9) AS g CREATE (:Genre { key: 'g' + toString(g), name: 'Genre ' + toString(g) })`,
  );
  await run(
    `MATCH (f:Festival), (g:Genre)
     WHERE g.key = 'g' + toString(toInteger(substring(f.key, 1)) % 10)
     CREATE (f)-[:IN_GENRE]->(g)`,
  );
  await run(
    `UNWIND range(0, $n - 1) AS i CREATE (:User { key: 'u' + toString(i), name: 'User ' + toString(i) })`,
    { n: size.users },
  );
  await run(
    `UNWIND range(0, $users - 1) AS u
     UNWIND range(0, $per - 1) AS j
     MATCH (a:User) WHERE a.key = 'u' + toString(u)
     MATCH (f:Festival) WHERE f.key = 'f' + toString((u * 7919 + j * 104729) % $festivals)
     CREATE (a)-[:FOLLOWS { since: 2000 + j }]->(f)`,
    {
      users: size.users,
      per: size.followsPerUser,
      festivals: size.festivals,
    },
  );
}

export async function seed(options: Partial<LoraGraphQLOptions> = {}) {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: loraDriver(db),
    ...options,
  });
  await lora.assertSchema({ create: true });
  await seedGraph(db, graphSize());
  return { db, lora };
}
