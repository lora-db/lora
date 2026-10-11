// check() reports stored values a full-text index cannot hold: the engine
// indexes strings and skips the rest in silence.

import { expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

test("a non-string value under a full-text field is reported", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: `type F @node @fulltext(indexes: [{ fields: ["name", "tags"] }]) { key: String! @key  name: String  tags: [String!]  size: Int }`,
    seed: [
      "CREATE (:F {key: 'ok', name: 'Oslo', tags: ['a', 'b'], size: 1})",
      "CREATE (:F {key: 'num', name: 42})",
      "CREATE (:F {key: 'mixed', name: 'Bergen', tags: ['a', 7]})",
      "CREATE (:F {key: 'none'})",
    ],
  });
  const report = await t.lora.check();
  expect(report.stored.map((v) => [v.key, v.field, v.rule]).sort()).toEqual([
    ["mixed", "tags", "fulltext"],
    ["num", "name", "fulltext"],
  ]);
  expect(report.stored[0]!.message).toContain("is not text");
  // The search confirms it: the number is not found.
  expect(await t.data(`{ searchFs(query: "42") { node { key } } }`)).toEqual({
    searchFs: [],
  });
  t.close();
});
