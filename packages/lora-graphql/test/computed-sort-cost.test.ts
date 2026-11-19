// A @sortable @cypher field with a declared per-run cost: the model takes
// the cost as the author's word (no warning), and the cost limit charges a
// sort by it for every row it runs on, not once.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = (cost: string) => `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}
type Person @node { key: String! @key }
type Event @node @limit(default: 10, max: 50) {
  key: String! @key
  title: String! @sortable
  viewerScore: Float @sortable @filterable
    @cypher(statement: """
      RETURN coalesce(head([(p:Person {key: $viewer})-[r:LIKES]->(this) | r.weight]), 0.0) AS s
    """, columnName: "s"${cost})
}`;
const seed = [
  "CREATE (:Person {key: 'ann'})",
  "MATCH (p:Person) CREATE (p)-[:LIKES {weight: 0.5}]->(:Event {key: 'e1', title: 'b'}), (p)-[:LIKES {weight: 0.9}]->(:Event {key: 'e2', title: 'c'}), (:Event {key: 'e3', title: 'a'})",
];
const ann = { jwt: { sub: "ann" } };

describe("a sortable @cypher field with a declared cost", () => {
  test("is accepted without the scan warning", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(", cost: 2"),
      seed,
    });
    expect(t.lora.model.warnings).toEqual([]);
    expect(
      await t.data(
        "{ events(sort: [{ viewerScore: DESC }]) { key viewerScore } }",
        {},
        ann,
      ),
    ).toEqual({
      events: [
        { key: "e2", viewerScore: 0.9 },
        { key: "e1", viewerScore: 0.5 },
        { key: "e3", viewerScore: 0 },
      ],
    });
  });

  test("without one, the model still warns", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: typeDefs(""), seed });
    expect(t.lora.model.warnings.map((w) => w.message)).toEqual([
      "filtering or sorting by viewerScore runs its statement for every Event considered: no index applies. Declare @cypher(cost:) with what one run costs to accept that",
    ]);
  });

  test("a sort by it is charged per row considered", async () => {
    const costs = new Map<string, number>();
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(", cost: 2"),
      seed,
      onCost: ({ cost, context }) =>
        costs.set(String((context as { q: string }).q), cost),
    });
    const run = async (q: string) => {
      await t.lora.execute({ source: q, context: { ...ann, q } });
      return costs.get(q)!;
    };
    const plain = await run("{ events(sort: [{ title: ASC }]) { key } }");
    const scored = await run(
      "{ events(sort: [{ viewerScore: DESC }]) { key } }",
    );
    // No statistics: the label is taken as the page (10 rows), each
    // examined once and running the statement (cost 2) once.
    expect(scored - plain).toBeGreaterThanOrEqual(10 * (1 + 2));
    // A filter on it already computes it: the sort adds no second run.
    const filtered = await run(
      "{ events(where: { viewerScore: { gt: 0 } }) { key } }",
    );
    const both = await run(
      "{ events(where: { viewerScore: { gt: 0 } }, sort: [{ viewerScore: DESC }]) { key } }",
    );
    expect(both).toBe(filtered);
  });
});
