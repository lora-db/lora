// `@cypher` arguments keep their SDL defaults on graphql 16 and 17, and an
// explicit null reaches the engine, which refuses `LIMIT null`.

import { printSchema } from "graphql";
import { describe, expect, test } from "vitest";
import { createDatabase } from "@loradb/lora-node";
import { createTestLoraGraphQL } from "../src/testing.js";

describe("@cypher argument defaults are kept; LIMIT null is an error", () => {
  const typeDefs = `type N @node { key: String! @key
    peers(limit: Int = 2, from: String = "n1"): [N!]! @cypher(statement: "MATCH (o:N) WHERE o.key <> $from RETURN o ORDER BY o.key LIMIT $limit") }`;
  const seed = ["UNWIND range(1, 5) AS i CREATE (:N {key: 'n' + toString(i)})"];

  test("the published schema and execution keep the SDL default", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(printSchema(t.schema)).toContain(
      'peers(limit: Int = 2, from: String = "n1"): [N!]!',
    );
    const r = await t.run(`{ n(key:"n1") { peers { key } } }`);
    expect(r.errors).toBeUndefined();
    expect(
      (r.data as { n: { peers: Array<{ key: string }> } }).n.peers.map(
        (p) => p.key,
      ),
    ).toEqual(["n2", "n3"]);
    const three = await t.data<{ n: { peers: unknown[] } }>(
      `{ n(key:"n1") { peers(limit: 3) { key } } }`,
    );
    expect(three.n.peers).toHaveLength(3);
  });

  test("an explicit null reaches the engine, which refuses LIMIT null", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(`{ n(key:"n1") { peers(limit: null) { key } } }`);
    expect(r.errors?.[0]?.message).toMatch(/LIMIT/);
    const db = await createDatabase();
    await expect(
      db.execute("RETURN 1 AS x LIMIT $l", { l: null }),
    ).rejects.toThrow(/LIMIT expects a non-negative integer, got null/);
  });
});
