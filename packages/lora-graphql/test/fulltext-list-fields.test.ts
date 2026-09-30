// `@fulltext` over `[String!]` fields: each string of the list is indexed.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

describe("full-text search over a [String!] field", () => {
  test("@fulltext accepts list fields and finds each string", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type F @node @fulltext(indexes: [{ fields: ["name", "aka"] }]) { key: String! @key  name: String!  aka: [String!]! }`,
      seed: [
        "CREATE (:F {key: 'a', name: 'Norway', aka: ['Oslo summer', 'Øyafestivalen']}), (:F {key: 'b', name: 'Bergen', aka: []})",
      ],
    });
    const find = async (query: string) =>
      (
        await t.data<{ searchFs: Array<{ node: { key: string } }> }>(
          `query($q: String!) { searchFs(query: $q) { node { key } } }`,
          { q: query },
        )
      ).searchFs.map((m) => m.node.key);
    expect(await find("oslo")).toEqual(["a"]);
    expect(await find("oya*")).toEqual(["a"]);
    expect(await find("bergen")).toEqual(["b"]);
    // A list written through a mutation is indexed too.
    await t.db.execute(
      "MATCH (f:F {key: 'b'}) SET f.aka = ['Bergenfest', 'Oslo fjord']",
    );
    expect((await find("oslo")).sort()).toEqual(["a", "b"]);
  });

  test("a non-string list is still refused", async () => {
    await expect(
      createTestLoraGraphQL({
        typeDefs: `type F @node @fulltext(indexes: [{ fields: ["n"] }]) { key: String! @key  n: [Int!]! }`,
      }),
    ).rejects.toThrow(/is not a String or \[String\] field/);
  });
});
