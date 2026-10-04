// A field-level READ rule that is unknown (NULL) on a row must not let a
// negated filter on that field reveal the field's value: the guarded
// predicate is false, never NULL, on a row failing the rule, so `NOT`
// over it is true on every such row whatever the hidden value.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

const rule = `@authorization(validate: [{ operations: [READ], where: { node: { visible: { eq: true } } } }])`;
const typeDefs = `type Emp @node {
  key: String! @key
  visible: Boolean
  salary: Int @filterable(byValue: [GT]) ${rule}
  score: Int @filterable(byValue: [GT]) ${rule}
    @cypher(statement: "RETURN this.salary AS s", columnName: "s")
  boss: Emp @relationship(type: "BOSS", direction: OUT) @filterable ${rule}
  reports: [Emp!]! @relationship(type: "REPORTS", direction: OUT, properties: "Since") @filterable ${rule}
}
type Since @relationshipProperties { year: Int @filterable(byValue: [GT]) }`;
// `visible` unset on every row: the rule is NULL, not false.
const seed = [
  `CREATE (big:Emp {key: 'big', salary: 999}),
          (low:Emp {key: 'low', salary: 50}), (high:Emp {key: 'high', salary: 200}),
          (high)-[:BOSS]->(big), (high)-[:REPORTS {year: 2020}]->(big)`,
];

type Emps = { emps: Array<{ key: string }> };
const keys = async (
  t: Awaited<ReturnType<typeof createTestLoraGraphQL>>,
  where: string,
) =>
  (
    await t.data<Emps>(
      `{ emps(where: ${where}, sort: [{ key: ASC }]) { key } }`,
      {},
      { jwt: { sub: "u" } },
    )
  ).emps
    .map((e) => e.key)
    .sort();

describe("negated filters over guarded fields", () => {
  test("NOT over a guarded scalar filter matches every row failing the rule", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(await keys(t, "{ salary: { gt: 100 } }")).toEqual([]);
    expect(await keys(t, "{ NOT: { salary: { gt: 100 } } }")).toEqual([
      "big",
      "high",
      "low",
    ]);
  });

  test("NOT over a guarded @cypher filter", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(await keys(t, "{ score: { gt: 100 } }")).toEqual([]);
    expect(await keys(t, "{ NOT: { score: { gt: 100 } } }")).toEqual([
      "big",
      "high",
      "low",
    ]);
  });

  test("NOT over a guarded single relationship, Exists and Connection", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const all = ["big", "high", "low"];
    expect(await keys(t, '{ boss: { key: { eq: "big" } } }')).toEqual([]);
    expect(await keys(t, '{ NOT: { boss: { key: { eq: "big" } } } }')).toEqual(
      all,
    );
    expect(await keys(t, "{ bossExists: true }")).toEqual([]);
    expect(await keys(t, "{ NOT: { bossExists: true } }")).toEqual(all);
    expect(await keys(t, "{ NOT: { bossExists: false } }")).toEqual(all);
    expect(
      await keys(t, '{ reports: { some: { key: { eq: "big" } } } }'),
    ).toEqual([]);
    expect(
      await keys(t, '{ NOT: { reports: { some: { key: { eq: "big" } } } } }'),
    ).toEqual(all);
    expect(
      await keys(
        t,
        "{ NOT: { reportsConnection: { some: { edge: { year: { gt: 2000 } } } } } }",
      ),
    ).toEqual(all);
  });

  test("a row passing the rule still filters by its value", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed: [
        ...seed,
        "MATCH (e:Emp) WHERE e.key IN ['low', 'high'] SET e.visible = true",
      ],
    });
    expect(await keys(t, "{ salary: { gt: 100 } }")).toEqual(["high"]);
    expect(await keys(t, "{ NOT: { salary: { gt: 100 } } }")).toEqual([
      "big",
      "low",
    ]);
  });
});
