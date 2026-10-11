// maxTotalCount: a root or search connection's totalCount stops counting
// at the cap, so it costs a bounded read on a label of any size.

import { createDatabase } from "@loradb/lora-node";
import { expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";
import { LoraGraphQL, loraDriver, type StatementEvent } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Item @node @query(aggregate: true) {
    key: String! @key
    n: Int @filterable(byValue: [EQ, GT]) @sortable
    tags: [Tag!]! @relationship(type: "HAS", direction: OUT)
  }
  type Tag @node {
    key: String! @key
  }
`;

async function setup(maxTotalCount?: number) {
  const db = await createDatabase();
  await db.execute(
    "UNWIND range(1, 300) AS i CREATE (:Item {key: 'i' + toString(i), n: i})",
  );
  await db.execute(
    "MATCH (i:Item {key: 'i1'}) UNWIND range(1, 40) AS t CREATE (i)-[:HAS]->(:Tag {key: 't' + toString(t)})",
  );
  const statements: StatementEvent[] = [];
  const costs: number[] = [];
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    onStatement: (e) => statements.push(e),
    onCost: (e) => costs.push(e.cost),
    ...(maxTotalCount !== undefined ? { maxTotalCount } : {}),
  });
  await lora.assertSchema({ create: true });
  await lora.analyze();
  const run = async (source: string) => {
    statements.length = 0;
    costs.length = 0;
    const r = await lora.execute({ source });
    if (r.errors) throw r.errors[0];
    return r.data as Record<string, never>;
  };
  return { run, statements, costs };
}

test("without the option a count is every match", async () => {
  const { run } = await setup();
  expect(await run(`{ itemsConnection(first: 5) { totalCount } }`)).toEqual({
    itemsConnection: { totalCount: 300 },
  });
});

test("with it the count stops at the cap, and costs no more than the cap", async () => {
  const { run, costs, statements } = await setup(100);
  expect(await run(`{ itemsConnection(first: 5) { totalCount } }`)).toEqual({
    itemsConnection: { totalCount: 100 },
  });
  expect(costs).toEqual([100]);
  expect(statements.at(-1)!.statement.text).toMatch(/LIMIT \$p\d+/);
  // Fewer matches than the cap: the exact count.
  expect(
    await run(
      `{ itemsConnection(first: 5, where: { n: { gt: 290 } }) { totalCount edges { node { key } } } }`,
    ),
  ).toMatchObject({ itemsConnection: { totalCount: 10 } });
});

test("aggregates and a nested connection's totalCount are not capped", async () => {
  const { run } = await setup(20);
  expect(await run(`{ itemsAggregate { count n { max } } }`)).toEqual({
    itemsAggregate: { count: 300, n: { max: 300 } },
  });
  expect(
    await run(
      `{ item(key: "i1") { tagsConnection(first: 1) { totalCount } } }`,
    ),
  ).toEqual({ item: { tagsConnection: { totalCount: 40 } } });
  expect(
    await run(
      `{ itemsConnection(first: 1) { totalCount aggregate { node { n { max } } } } }`,
    ),
  ).toMatchObject({ itemsConnection: { totalCount: 300 } });
});

test("the option must be a positive integer", () => {
  for (const bad of [0, -1, 1.5, Number.NaN]) {
    expect(
      () =>
        new LoraGraphQL({
          typeDefs,
          driver: undefined as never,
          maxTotalCount: bad,
        }),
    ).toThrow("maxTotalCount must be a positive integer");
  }
});

test("a search connection's totalCount is capped too", async () => {
  const fulltext = `type F @node @fulltext(indexes: [{ fields: ["name"] }]) { key: String! @key  name: String! }`;
  const seed = [
    "UNWIND range(1, 12) AS i CREATE (:F {key: 'f' + toString(i), name: 'Oslo summer ' + toString(i)})",
  ];
  const capped = await createTestLoraGraphQL({
    typeDefs: fulltext,
    seed,
    maxTotalCount: 5,
  });
  expect(
    await capped.data(
      `{ searchFsConnection(query: "oslo", first: 2) { totalCount edges { node { key } } } }`,
    ),
  ).toMatchObject({ searchFsConnection: { totalCount: 5 } });
  capped.close();
  const open = await createTestLoraGraphQL({ typeDefs: fulltext, seed });
  expect(
    await open.data(`{ searchFsConnection(query: "oslo") { totalCount } }`),
  ).toEqual({ searchFsConnection: { totalCount: 12 } });
  open.close();
});
