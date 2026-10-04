// `nodes(ids:)`: node(id:) for many ids at once, in order, each under its
// type's READ rules.

import { describe, expect, test } from "vitest";
import { toGlobalId } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type Claims @jwt { sub: String! }
type Doc @node
  @authorization(filter: [{ operations: [READ], where: { node: { owner: { eq: "\${jwt.sub}" } } } }]) {
  key: String! @key @relayId
  owner: String!
}
type Tag @node { key: String! @key @relayId  name: String }`;
const seed = [
  "CREATE (:Doc {key: 'a', owner: 'lou'}), (:Doc {key: 'b', owner: 'bob'}), (:Tag {key: 't1', name: 'One'})",
];
const query = `query ($ids: [ID!]!) {
  nodes(ids: $ids) { __typename id ... on Doc { key } ... on Tag { name } }
}`;
const lou = { jwt: { sub: "lou" } };

describe("nodes(ids:)", () => {
  test("keeps the order, mixes types, and is null for hidden or unknown ids", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const ids = [
      toGlobalId("Tag", "t1"),
      toGlobalId("Doc", "b"),
      "bogus",
      toGlobalId("Doc", "a"),
      toGlobalId("Doc", "missing"),
      toGlobalId("Tag", "t1"),
    ];
    const d = await t.data<{ nodes: unknown[] }>(query, { ids }, lou);
    expect(d.nodes).toEqual([
      { __typename: "Tag", id: ids[0], name: "One" },
      null,
      null,
      { __typename: "Doc", id: ids[3], key: "a" },
      null,
      { __typename: "Tag", id: ids[0], name: "One" },
    ]);
    expect(await t.data(query, { ids: [] })).toEqual({ nodes: [] });
  });

  test("takes at most maxLimit ids", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed, maxLimit: 3 });
    const ids = Array.from({ length: 4 }, () => toGlobalId("Tag", "t1"));
    t.statements.length = 0;
    const r = await t.run(query, { ids }, lou);
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    expect(r.errors?.[0]?.message).toContain("at most 3 ids");
    expect(t.statements).toHaveLength(0);
    expect(
      (await t.data<{ nodes: unknown[] }>(query, { ids: ids.slice(1) }, lou))
        .nodes,
    ).toHaveLength(3);
  });

  test("is absent without @relayId types", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: "type Thing @node { key: String! @key }",
    });
    expect(t.schema.getQueryType()!.getFields()["nodes"]).toBeUndefined();
  });
});
