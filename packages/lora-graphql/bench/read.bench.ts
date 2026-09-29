// Latency of generated operations on a seeded graph, and of each
// translation choice against the naive Cypher it replaces.
//   yarn bench

import { graphql, parse } from "graphql";
import { bench, describe } from "vitest";
import { seed } from "./seed.js";

const { db, lora } = await seed();

const schema = lora.getSchema();
{
  const r = await graphql({
    schema,
    source: `{ festivals(limit: 10, where: { followers: { some: { key: { eq: "u42" } } } }) { key } }`,
    contextValue: {},
  });
  if (r.errors) console.error("anchored query failed:", r.errors[0]);
}
const run = async (source: string) => {
  const r = await graphql({ schema, source, contextValue: {} });
  if (r.errors) throw r.errors[0];
};
const cypher = async (text: string, params: Record<string, unknown> = {}) => {
  await db.execute(text, params as never);
};

const nested = `{
  festivals(limit: 20, sort: [{ name: ASC }]) {
    key name followerCount genre { name }
    followersConnection(first: 5) { totalCount edges { properties { since } node { key name } } }
  }
}`;

describe("translation", () => {
  const doc = parse(nested);
  bench("compile a nested operation (no database)", () => {
    lora.compile(doc);
  });
});

describe("operations, end to end", () => {
  bench("festivals(limit: 20): ordered by key", () =>
    run(`{ festivals(limit: 20) { key name } }`),
  );
  bench("nested page with connection, @cypher and relationship", () =>
    run(nested),
  );
  bench("text search (TEXT index)", () =>
    run(
      `{ festivalsConnection(first: 10, where: { name: { contains: "val 19" } }) { edges { node { key } } } }`,
    ),
  );
  bench("range filter (RANGE index)", () =>
    run(`{ festivals(limit: 10, where: { capacity: { gt: 4990 } }) { key } }`),
  );
  bench("followed by a user (anchored)", () =>
    run(
      `{ festivals(limit: 10, where: { followers: { some: { key: { eq: "u42" } } } }) { key } }`,
    ),
  );
});

describe("through execute(): documents and compiles cached", () => {
  const exec = async (source: string) => {
    const r = await lora.execute({ source, context: {} });
    if (r.errors) throw r.errors[0];
  };
  bench("festivals(limit: 20): ordered by key", () =>
    exec(`{ festivals(limit: 20) { key name } }`),
  );
  bench("nested page with connection, @cypher and relationship", () =>
    exec(nested),
  );
});

describe("key order: bounded vs sorted", () => {
  bench("generated: WHERE key >= '' streams from the index", () =>
    cypher(
      `MATCH (this:Festival) WHERE this.key >= "" WITH this ORDER BY this.key ASC LIMIT 20 RETURN this.key AS k`,
    ),
  );
  bench("naive: ORDER BY key LIMIT 20 sorts the label", () =>
    cypher(
      `MATCH (this:Festival) WITH this ORDER BY this.key ASC LIMIT 20 RETURN this.key AS k`,
    ),
  );
});

describe("relationship filter: anchored vs scanned", () => {
  bench("generated: start from the user, expand", () =>
    cypher(
      `MATCH (u:User) WHERE u.key = $u MATCH (u)-[:FOLLOWS]->(this:Festival) WITH DISTINCT this RETURN this.key AS k`,
      { u: "u42" },
    ),
  );
  bench("naive: scan festivals, test each", () =>
    cypher(
      `MATCH (this:Festival) WHERE size([(this)<-[:FOLLOWS]-(u:User) WHERE u.key = $u | 1]) > 0 RETURN this.key AS k`,
      { u: "u42" },
    ),
  );
});

describe("key IN list: unwound vs IN", () => {
  const keys = Array.from({ length: 20 }, (_, i) => `f${i * 997}`);
  bench("generated: UNWIND + equality seeks", () =>
    cypher(
      `UNWIND $keys AS k MATCH (this:Festival) WHERE this.key = k RETURN this.key AS key`,
      { keys },
    ),
  );
  bench("naive: WHERE key IN $keys", () =>
    cypher(
      `MATCH (this:Festival) WHERE this.key IN $keys RETURN this.key AS key`,
      { keys },
    ),
  );
});
