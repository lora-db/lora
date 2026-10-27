// G-72: several picks on a list field seek the list's elements once per
// pick, whether asked as `includesAny` or as an OR of `includes`.

import { describe, expect, test } from "vitest";
import { festivalHarness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Event @node {
    key: ID! @key
    genreLineage: [String!]! @filterable(byValue: [INCLUDES])
  }
`;

const seed = [
  `UNWIND range(1, 2000) AS i CREATE (:Event {key: 'e' + right('0000' + toString(i), 4),
     genreLineage: CASE WHEN i % 50 = 0 THEN ['electronic', 'techno']
                        WHEN i % 7 = 0 THEN ['electronic', 'house']
                        ELSE ['rock', 'rock-metal'] END})`,
];

const expected = Array.from({ length: 2000 }, (_, i) => i + 1)
  .filter((i) => i % 50 === 0 || i % 7 === 0)
  .map((i) => `e${String(i).padStart(4, "0")}`);

type Plan = {
  operator: string;
  details: Record<string, string>;
  children: Plan[];
};
const leaves = (plan: { tree: unknown }) => {
  const out: Plan[] = [];
  const walk = (n: Plan) => {
    out.push(n);
    for (const c of n.children) walk(c);
  };
  walk(plan.tree as Plan);
  return out;
};

describe("G-72: several picks on a list field", () => {
  for (const [form, where] of [
    ["includesAny", `{ genreLineage: { includesAny: ["techno", "house"] } }`],
    [
      "an OR of includes",
      `{ OR: [{ genreLineage: { includes: "techno" } }, { genreLineage: { includes: "house" } }] }`,
    ],
  ] as const) {
    test(`${form} seeks each pick and returns each event once`, async () => {
      const h = await festivalHarness({ typeDefs, seed });
      const query = `{ events(where: ${where}, sort: [{ key: ASC }], limit: 100) { key } }`;
      const [explained] = await h.lora.explain(query);
      const report = explained!.reports[0]!;
      expect(report.findings).toEqual([]);
      expect(report.operators).not.toContain("NodeByLabelScan");
      const seek = leaves(report.plan).find(
        (n) => n.details["mode"] === "any-element",
      );
      expect(seek?.operator).toBe("NodeByPropertyScan");
      expect(seek?.details["key"]).toBe("genreLineage");

      const { events } = await h.data<{ events: Array<{ key: string }> }>(
        query,
      );
      expect(events.map((e) => e.key)).toEqual(expected.slice(0, 100));
      const count = await h.data<{ eventsConnection: { totalCount: number } }>(
        `{ eventsConnection(where: ${where}) { totalCount } }`,
      );
      expect(count.eventsConnection.totalCount).toBe(expected.length);
    });
  }

  test("includesAny with no picks, or picks no list holds, returns nothing", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    for (const picks of [`[]`, `["jazz", "blues"]`]) {
      const { events } = await h.data<{ events: unknown[] }>(
        `{ events(where: { genreLineage: { includesAny: ${picks} } }) { key } }`,
      );
      expect(events).toEqual([]);
    }
  });

  test("includesAny takes at most maxListFilter picks", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const picks = JSON.stringify(
      Array.from({ length: 1001 }, (_, i) => `g${i}`),
    );
    const result = await h.run(
      `{ events(where: { genreLineage: { includesAny: ${picks} } }) { key } }`,
    );
    expect(result.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    expect(result.errors?.[0]?.message).toMatch(/at most 1000 items/);
  });
});
