// G-73: with `analyze()`, a filter an index answers by value is charged the
// nodes holding that value, not the whole label; a `@cypher` root without
// a declared cost is charged its statement's plan estimate.

import { describe, expect, test } from "vitest";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  enum Family {
    A
    B
    C
    D
    E
  }
  type E @node {
    key: String! @key
    family: Family! @filterable(byValue: [EQ, IN])
    genreLineage: [String!]! @filterable
    tag: String @filterable(byValue: [EQ])
  }
  type Query {
    techno: [E!]!
      @cypher(
        statement: "MATCH (e:E) WHERE 'techno' IN e.genreLineage RETURN e"
      )
    declared: [E!]! @cypher(statement: "MATCH (e:E) RETURN e", cost: 5)
  }
`;

// 6,000 nodes, skewed like a genre tree: every node under 'music', half
// under 'electronic', one in 20 'techno', one in 300 'acid'. Families
// A..E by i % 10 (A three in five). `tag`: 3,000 values, 't0' on 1 in 3.
const N = 6000;
const seed = [
  `UNWIND range(1, ${N}) AS i CREATE (:E {key: 'e' + toString(i),
     family: CASE i % 10 WHEN 1 THEN 'B' WHEN 2 THEN 'C' WHEN 3 THEN 'D'
                         WHEN 4 THEN 'E' ELSE 'A' END,
     genreLineage: ['music']
       + CASE WHEN i % 2 = 0 THEN ['electronic'] ELSE ['rock'] END
       + CASE WHEN i % 20 = 0 THEN ['techno'] ELSE [] END
       + CASE WHEN i % 300 = 0 THEN ['acid', 'acid'] ELSE [] END,
     tag: CASE WHEN i % 3 = 0 THEN 't0' ELSE 't' + toString(i % 3000) END})`,
];

const cost = (h: Harness, source: string) =>
  h.lora.compile(source).reduce((sum, f) => sum + f.compiled.cost, 0);

const counted = (where: string) =>
  `{ esConnection(where: ${where}, first: 10) { totalCount edges { node { key } } } }`;

async function check(h: Harness, where: string) {
  const { esConnection } = await h.data<{
    esConnection: { totalCount: number };
  }>(counted(where));
  // The page and its lookahead row, beside the count.
  const estimate = cost(h, counted(where)) - 11;
  return { actual: esConnection.totalCount, estimate };
}

describe("G-73: per-value statistics", () => {
  test("a count under a seek is charged the nodes holding the value", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    // Without statistics: the label, as before.
    const before = await check(h, `{ genreLineage: { includes: "techno" } }`);
    expect(before.actual).toBe(300);

    const stats = await h.lora.analyze();
    expect(stats.values?.["E.genreLineage"]).toEqual({
      distinct: 5,
      counts: {
        '"music"': 6000,
        '"electronic"': 3000,
        '"rock"': 3000,
        '"techno"': 300,
        '"acid"': 20,
      },
      rest: 0,
    });
    expect(stats.values?.["E.family"]?.counts['"A"']).toBe(3600);

    for (const where of [
      `{ genreLineage: { includes: "techno" } }`,
      `{ genreLineage: { includes: "music" } }`,
      `{ genreLineage: { includes: "acid" } }`,
      `{ OR: [{ genreLineage: { includes: "techno" } }, { genreLineage: { includes: "acid" } }] }`,
      `{ genreLineage: { includesAny: ["techno", "acid"] } }`,
      `{ family: { eq: B } }`,
      `{ family: { in: [B, C] } }`,
      `{ family: { eq: A }, genreLineage: { includes: "techno" } }`,
      `{ AND: [{ family: { eq: A } }, { genreLineage: { includes: "acid" } }] }`,
      `{ tag: { eq: "t0" } }`,
      `{ tag: { eq: "t7" } }`,
    ]) {
      const { actual, estimate } = await check(h, where);
      expect(estimate, where).toBeGreaterThanOrEqual(actual);
      expect(estimate, where).toBeLessThanOrEqual(Math.max(2 * actual, 2));
    }
    // A value no node holds costs (next to) nothing to seek.
    expect(
      (await check(h, `{ genreLineage: { includes: "jazz" } }`)).estimate,
    ).toBeLessThanOrEqual(1);
    // An OR the engine scans for is charged the label.
    expect(
      (
        await check(
          h,
          `{ OR: [{ genreLineage: { includes: "acid" } }, { family: { eq: B } }] }`,
        )
      ).estimate,
    ).toBeGreaterThanOrEqual(N);
  });

  test("past 1,024 values, an uncounted value is charged the largest uncounted count", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const stats = await h.lora.analyze();
    const tag = stats.values!["E.tag"]!;
    expect(tag.distinct).toBe(1025);
    expect(Object.keys(tag.counts)).toHaveLength(64);
    expect(tag.counts['"t0"']).toBe(2000);
    // Every other tag is on one or two nodes.
    expect(tag.rest).toBeLessThanOrEqual(2);
    expect(tag.rest).toBeGreaterThanOrEqual(1);
  });

  test("a @cypher root is charged its plan's estimate unless it declares one", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const plain = cost(h, `{ techno { key } }`);
    const declared = cost(h, `{ declared { key } }`);
    const stats = await h.lora.analyze();
    // The element seek's estimate (the average per element: 12,320
    // postings over 5 elements), and the label scan before it.
    expect(stats.cypher?.["Query.techno"]).toBeGreaterThan(0);
    expect(stats.cypher?.["Query.techno"]).toBeLessThanOrEqual(N);
    expect(stats.cypher?.["Query.declared"]).toBeUndefined();
    expect(cost(h, `{ techno { key } }`)).toBe(
      plain + stats.cypher!["Query.techno"]!,
    );
    expect(cost(h, `{ declared { key } }`)).toBe(declared);
  });
});
