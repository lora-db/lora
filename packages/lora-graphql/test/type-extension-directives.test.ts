// Directives on `extend type` / `extend interface` / `extend union` /
// `extend schema` apply as if written on the definition. A non-repeatable
// directive written on both is a model error, never a silent pick.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const problems = (sdl: string): string[] => {
  try {
    buildModel(sdl);
    return [];
  } catch (err) {
    if (err instanceof ModelError) return err.problems.map((p) => p.message);
    throw err;
  }
};

describe("directives on type extensions", () => {
  test("@authorization on an extension guards the type", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Secret @node { key: String! @key  owner: String }
        extend type Secret @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }])`,
      seed: [
        "CREATE (:Secret {key: 'a', owner: 'ann'}), (:Secret {key: 'b', owner: 'bob'})",
      ],
    });
    const d = await t.data<{ secrets: Array<{ key: string }> }>(
      "{ secrets { key } }",
      {},
      { jwt: { sub: "ann" } },
    );
    expect(d.secrets).toEqual([{ key: "a" }]);
    // Without a token the rule's claim is missing: nothing is visible.
    const anon = await t.data<{ secrets: unknown[] }>("{ secrets { key } }");
    expect(anon.secrets).toEqual([]);
  });

  test("API, limit, search and uniqueness directives on an extension apply", () => {
    const model =
      buildModel(`type T @node { key: String! @key  a: String  b: String
        tags: [T!]! @relationship(type: "R", direction: OUT) }
      extend type T @mutation(operations: [CREATE]) @subscription(operations: [CREATE])
        @query(read: true, aggregate: true) @limit(default: 3, max: 7)
        @fulltext(indexes: [{ fields: ["a"] }]) @uniqueTogether(fields: ["a", "b"])
        @authorizationRule(name: "mine", where: { node: { a: { eq: "$jwt.sub" } } })
        @authorization(filter: [{ where: { rule: "mine" } }])`);
    const t = model.nodes.get("T")!;
    expect([...t.mutations]).toEqual(["CREATE"]);
    expect([...t.subscriptions]).toEqual(["CREATE"]);
    expect(t.aggregate).toBe(true);
    expect(t.limit).toEqual({ default: 3, max: 7 });
    expect(t.search.map((s) => s.kind)).toEqual(["fulltext"]);
    expect(t.uniqueTogether.map((u) => u.fields)).toEqual([["a", "b"]]);
    expect(t.authorization?.filter?.length).toBe(1);
  });

  test("interface and union extensions", () => {
    const model = buildModel(`interface I { key: String! }
      extend interface I @limit(default: 2, max: 4)
      type A implements I @node { key: String! @key }
      type B @node { key: String! @key }
      union U = A
      extend union U @limit(default: 5, max: 6)
      extend union U = B`);
    expect(model.abstracts.get("I")?.limit).toEqual({ default: 2, max: 4 });
    expect(model.abstracts.get("U")?.limit).toEqual({ default: 5, max: 6 });
  });

  test("a non-repeatable directive on the definition and an extension is a model error", () => {
    const found = problems(`type T @node @limit(max: 5) { key: String! @key }
      extend type T @limit(max: 9)`);
    expect(found.join("\n")).toMatch(/@limit/);
  });

  test("repeatable directives add up across the definition and extensions", () => {
    const model = buildModel(`type T @node @uniqueTogether(fields: ["a", "b"]) {
        key: String! @key  a: String  b: String }
      extend type T @uniqueTogether(fields: ["b", "a"])`);
    expect(model.nodes.get("T")!.uniqueTogether.length).toBe(2);
  });

  test("`extend type X @node` alone works", () => {
    const model = buildModel(`type X { key: String! @key }
      extend type X @node`);
    expect(model.nodes.has("X")).toBe(true);
  });

  test("an extension of an undefined type is a clear error", () => {
    expect(problems("extend type X @node").join("\n")).toMatch(/X/);
  });

  test("@authorizationDefaults on two schema extensions is a model error", () => {
    const found = problems(`type T @node { key: String! @key }
      extend schema @authorizationDefaults(bypass: { jwt: { sub: { eq: "a" } } })
      extend schema @authorizationDefaults(bypass: { jwt: { sub: { eq: "b" } } })`);
    expect(found.join("\n")).toMatch(/@authorizationDefaults/);
  });
});
