// An `OR` branch or a `NOT` left empty once absent and null filters are
// left out adds nothing; a literal `OR: []` still matches nothing.

import { describe, expect, test } from "vitest";
import { emptyQuantifierRule } from "../src/quantifier-rule.js";
import { createTestLoraGraphQL } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const keysOf = (r: Result, field: string) => {
  expect(r.errors).toBeUndefined();
  return ((r.data as Record<string, Array<{ key: string }>>)[field] ?? []).map(
    (x) => x.key,
  );
};

describe("an OR branch or NOT emptied by absent variables is left out", () => {
  const typeDefs = `type F @node { key: String! @key  region: String @filterable  country: String @filterable
    friends: [F!]! @relationship(type: "KNOWS", direction: OUT) @filterable }`;
  const seed = [
    "CREATE (:F {key:'a', region:'EU', country:'nl'}), (:F {key:'b', region:'AS', country:'jp'}), (:F {key:'c', region:'AS', country:'kr'})",
    "MATCH (a:F {key:'a'}), (b:F {key:'b'}), (c:F {key:'c'}) CREATE (a)-[:KNOWS]->(b), (b)-[:KNOWS]->(c)",
  ];
  const q = `query($r:[String!],$c:[String!]){ fs(where:{OR:[{region:{in:$r}},{country:{in:$c}}]}, sort: [{ key: ASC }]){ key } }`;

  test("OR keeps only the branches that have a filter", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(keysOf(await t.run(q, { r: ["EU"], c: ["jp"] }), "fs")).toEqual([
      "a",
      "b",
    ]);
    expect(keysOf(await t.run(q, { r: ["EU"] }), "fs")).toEqual(["a"]);
    expect(keysOf(await t.run(q, { c: ["kr"] }), "fs")).toEqual(["c"]);
    expect(keysOf(await t.run(q, {}), "fs")).toEqual(["a", "b", "c"]);
    expect(keysOf(await t.run(q, { r: null, c: null }), "fs")).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  test("an emptied NOT adds nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const not = `query($c:[String!]){ fs(where:{NOT:{country:{in:$c}}}, sort: [{ key: ASC }]){ key } }`;
    expect(keysOf(await t.run(not), "fs")).toEqual(["a", "b", "c"]);
    expect(keysOf(await t.run(not, { c: ["jp"] }), "fs")).toEqual(["a", "c"]);
  });

  test("a literal OR: [] still matches nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      keysOf(await t.run(`{ fs(where: { OR: [] }) { key } }`), "fs"),
    ).toEqual([]);
  });

  test("the same holds inside a relationship filter", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const rel = `query($r:[String!],$c:[String!]){ fs(where:{friends:{some:{OR:[{region:{in:$r}},{country:{in:$c}}]}}}, sort: [{ key: ASC }]){ key } }`;
    // b is known by a; c (region AS, country kr) is known by b.
    expect(keysOf(await t.run(rel, { c: ["kr"] }), "fs")).toEqual(["b"]);
    expect(keysOf(await t.run(rel, { r: ["AS"] }), "fs")).toEqual(["a", "b"]);
    const relNot = `query($c:[String!]){ fs(where:{friends:{some:{NOT:{country:{in:$c}}}}}, sort: [{ key: ASC }]){ key } }`;
    // An emptied quantifier filter is left out (documented): every row.
    expect(keysOf(await t.run(relNot), "fs")).toEqual(["a", "b", "c"]);
    expect(keysOf(await t.run(relNot, { c: ["jp"] }), "fs")).toEqual(["b"]);
  });
});

describe("a literal empty quantifier is refused", () => {
  const typeDefs = `type C @node { key: String! @key
      vs: [V!]! @relationship(type: "IN", direction: IN, properties: "At") @filterable }
    type V @node { key: String! @key }
    type At @relationshipProperties { since: Int @filterable }`;
  const seed =
    "CREATE (a:C {key: 'a'}), (:C {key: 'b'}), (v:V {key: 'v'}), (v)-[:IN]->(a)";

  test.each([
    ["some", "count: { gt: 0 }"],
    ["none", "count: { eq: 0 }"],
    ["single", "count: { eq: 1 }"],
  ])("`%s: {}` names %s", async (quantifier, instead) => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      `{ cs(where: { vs: { ${quantifier}: {} } }) { key } }`,
    );
    expect(r.errors?.[0]?.message).toContain(`vs: { ${quantifier}: {} }`);
    expect(r.errors?.[0]?.message).toContain(instead);
  });

  test("an object holding only empty objects is empty too", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      `{ cs(where: { vsConnection: { some: { node: {}, edge: {} } } }) { key } }`,
    );
    expect(r.errors?.[0]?.message).toContain("vsConnection: { some: {} }");
  });

  test("check() reports it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.lora.check({
      operations: [
        { document: "query Q { cs(where: { vs: { some: {} } }) { key } }" },
      ],
    });
    expect(r.ok).toBe(false);
    expect(r.errors[0]?.message).toContain("vs: { some: {} }");
  });

  test("an empty `all`, and a filter emptied by variables, stay left out", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const all = await t.run(
      `{ cs(where: { vs: { all: {} } }, sort: [{ key: ASC }]) { key } }`,
    );
    expect(keysOf(all, "cs")).toEqual(["a", "b"]);
    const emptied = `query($k: String) { cs(where: { vs: { some: { key: { eq: $k } } } }, sort: [{ key: ASC }]) { key } }`;
    expect(keysOf(await t.run(emptied), "cs")).toEqual(["a", "b"]);
    expect(keysOf(await t.run(emptied, { k: "v" }), "cs")).toEqual(["a"]);
  });

  test("a manifest refuses it, as generate does", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(() =>
      t.lora.buildManifest({
        Q: "query Q { cs(where: { vs: { none: {} } }) { key } }",
      }),
    ).toThrow(/vs: \{ none: \{\} \}.*count: \{ eq: 0 \}/);
  });

  test("the Envelop plugin adds the rule", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const rules: unknown[] = [];
    t.lora
      .envelopPlugin()
      .onValidate({ addValidationRule: (r) => rules.push(r) });
    expect(rules).toContain(emptyQuantifierRule);
  });
});
