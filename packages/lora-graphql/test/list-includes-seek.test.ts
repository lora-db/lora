// G-63: `includes` on a list field infers a RANGE index, and the engine
// seeks the list's elements in it instead of scanning the label.

import { describe, expect, test } from "vitest";
import { buildModel, inferRequirements, requirementDdl } from "../src/index.js";
import { festivalHarness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Event @node {
    key: ID! @key
    city: String @filterable(byValue: [EQ])
    genreLineage: [String!]! @filterable(byValue: [INCLUDES])
    tags: [String!] @filterable
  }
`;

const seed = [
  `UNWIND range(1, 2000) AS i CREATE (:Event {key: 'e' + right('0000' + toString(i), 4),
     city: CASE WHEN i % 2 = 0 THEN 'Berlin' ELSE 'Oslo' END,
     genreLineage: CASE WHEN i % 50 = 0 THEN ['electronic', 'techno']
                        WHEN i % 7 = 0 THEN ['electronic', 'house']
                        ELSE ['rock', 'rock-metal'] END,
     tags: CASE WHEN i % 3 = 0 THEN ['free', 'free'] ELSE null END})`,
];

const leaves = (plan: { tree: unknown }) => {
  const out: Array<{ operator: string; details: Record<string, string> }> = [];
  const walk = (n: {
    operator: string;
    details: Record<string, string>;
    children: unknown[];
  }) => {
    out.push(n);
    for (const c of n.children) walk(c as typeof n);
  };
  walk(plan.tree as Parameters<typeof walk>[0]);
  return out;
};

describe("G-63: INCLUDES on a list field", () => {
  test("infers a RANGE index on the list", () => {
    const reqs = inferRequirements(buildModel(typeDefs));
    expect(reqs.map((r) => [requirementDdl(r), r.reason])).toEqual([
      [
        "CREATE CONSTRAINT `event_key_key` IF NOT EXISTS FOR (n:`Event`) REQUIRE n.`key` IS NODE KEY",
        "Event.key is the @key",
      ],
      [
        "CREATE INDEX `event_genre_lineage_range` IF NOT EXISTS FOR (n:`Event`) ON (n.`genreLineage`)",
        "Event.genreLineage is @filterable by INCLUDES",
      ],
      [
        "CREATE INDEX `event_tags_range` IF NOT EXISTS FOR (n:`Event`) ON (n.`tags`)",
        "Event.tags is @filterable by INCLUDES",
      ],
    ]);
  });

  test("an includes filter seeks the list's elements", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const query = `query($g: String!) { events(where: { genreLineage: { includes: $g } }, sort: [{ key: ASC }], limit: 100) { key } }`;
    const [explained] = await h.lora.explain(query, { g: "techno" });
    const report = explained!.reports[0]!;
    expect(report.findings).toEqual([]);
    expect(report.operators).not.toContain("NodeByLabelScan");
    const seek = leaves(report.plan).find(
      (n) => n.details["mode"] === "element",
    );
    expect(seek?.operator).toBe("NodeByPropertyScan");
    expect(seek?.details["key"]).toBe("genreLineage");

    const { events } = await h.data<{ events: Array<{ key: string }> }>(query, {
      g: "techno",
    });
    expect(events.map((e) => e.key)).toEqual(
      Array.from(
        { length: 40 },
        (_, i) => `e${String((i + 1) * 50).padStart(4, "0")}`,
      ),
    );
    // Duplicates in a list return the node once; a value no list holds
    // returns nothing.
    const free = await h.data<{ eventsConnection: { totalCount: number } }>(
      `{ eventsConnection(where: { tags: { includes: "free" } }) { totalCount } }`,
    );
    expect(free.eventsConnection.totalCount).toBe(666);
    const none = await h.data<{ events: unknown[] }>(
      `{ events(where: { genreLineage: { includes: "jazz" } }) { key } }`,
    );
    expect(none.events).toEqual([]);
  });

  test("beside another seekable test and on a keyset page, the check passes", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const both = `{ events(where: { city: { eq: "Berlin" }, genreLineage: { includes: "techno" } }, limit: 100) { key } }`;
    const [explained] = await h.lora.explain(both);
    expect(explained!.reports[0]!.findings).toEqual([]);
    expect((await h.data<{ events: unknown[] }>(both)).events).toHaveLength(40);

    const page = `query($after: String) { eventsConnection(first: 7, after: $after, where: { genreLineage: { includes: "techno" } }, sort: [{ key: ASC }]) { edges { node { key } } pageInfo { endCursor hasNextPage } } }`;
    const [pageExplained] = await h.lora.explain(page);
    expect(pageExplained!.reports[0]!.findings).toEqual([]);
    type EventsPage = {
      eventsConnection: {
        edges: Array<{ node: { key: string } }>;
        pageInfo: { endCursor: string | null; hasNextPage: boolean };
      };
    };
    const keys: string[] = [];
    let after: string | null = null;
    for (;;) {
      const { eventsConnection }: EventsPage = await h.data<EventsPage>(page, {
        after,
      });
      keys.push(...eventsConnection.edges.map((e) => e.node.key));
      if (!eventsConnection.pageInfo.hasNextPage) break;
      after = eventsConnection.pageInfo.endCursor;
    }
    expect(keys).toHaveLength(40);
    expect(keys).toEqual([...keys].sort());
  });
});
