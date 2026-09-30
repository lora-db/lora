// `totalCount` on search connections: every match after `where` and the
// read rules, and no page read for a count-only selection.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

describe("search connections have totalCount", () => {
  const typeDefs = `type F @node @fulltext(indexes: [{ fields: ["name"] }]) { key: String! @key  name: String!  region: String @filterable }`;
  const seed = [
    "UNWIND range(1, 12) AS i CREATE (:F {key: 'f' + toString(i), name: 'Oslo summer ' + toString(i), region: CASE WHEN i % 2 = 0 THEN 'EU' ELSE 'AS' END})",
    "CREATE (:F {key: 'z', name: 'Bergen'})",
  ];

  test("counts every match after where, whatever the page", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const none = await t.data<{ searchFsConnection: { totalCount: number } }>(
      `{ searchFsConnection(query: "x") { totalCount } }`,
    );
    expect(none.searchFsConnection.totalCount).toBe(0);
    const page = await t.data<{
      searchFsConnection: {
        totalCount: number;
        edges: unknown[];
        pageInfo: { endCursor: string };
      };
    }>(
      `{ searchFsConnection(query: "oslo", first: 5) { totalCount edges { node { key } } pageInfo { endCursor } } }`,
    );
    expect(page.searchFsConnection.totalCount).toBe(12);
    expect(page.searchFsConnection.edges).toHaveLength(5);
    // The count ignores the cursor: it is the whole result.
    const next = await t.data<{ searchFsConnection: { totalCount: number } }>(
      `query($a: String) { searchFsConnection(query: "oslo", first: 5, after: $a) { totalCount edges { node { key } } } }`,
      { a: page.searchFsConnection.pageInfo.endCursor },
    );
    expect(next.searchFsConnection.totalCount).toBe(12);
    const eu = await t.data<{ searchFsConnection: { totalCount: number } }>(
      `{ searchFsConnection(query: "oslo", where: { region: { eq: "EU" } }) { totalCount } }`,
    );
    expect(eu.searchFsConnection.totalCount).toBe(6);
  });

  test("a count-only selection reads no page", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    t.statements.length = 0;
    await t.data(`{ searchFsConnection(query: "oslo") { totalCount } }`);
    const texts = t.statements.map((s) => s.statement.text).join("\n---\n");
    expect(texts).toContain("count(this)");
    expect(texts).not.toContain("ORDER BY");
  });

  test("the count honours the read rules", async () => {
    const J = "type Claims @jwt { sub: String!  roles: [String!] }\n";
    const t = await createTestLoraGraphQL({
      typeDefs:
        J +
        `type F @node @fulltext(indexes: [{ fields: ["name"] }])
          @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
          key: String! @key  name: String!  owner: String! }`,
      seed: [
        "CREATE (:F {key:'a', name:'Oslo', owner:'u'}), (:F {key:'b', name:'Oslo', owner:'v'}), (:F {key:'c', name:'Oslo', owner:'u'})",
      ],
    });
    const r = await t.data<{ searchFsConnection: { totalCount: number } }>(
      `{ searchFsConnection(query: "oslo") { totalCount } }`,
      {},
      { jwt: { sub: "u", roles: [] } },
    );
    expect(r.searchFsConnection.totalCount).toBe(2);
  });
});
