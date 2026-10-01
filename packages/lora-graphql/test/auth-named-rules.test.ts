// Named rules: defined once (`@authorizationRules` for claims,
// `@authorizationRule` on a type), referenced as `{ rule: "name" }`, also
// through a relationship to the defining type, inlined at startup.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";
import { expectSameStatements } from "./equivalence.js";

const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
extend schema
  @authorizationRules(rules: [{ name: "admin", where: { jwt: { roles: { includes: "admin" } } } }])
`;
const named = `type Person @node { key: String! @key }
type Trip @node
  @authorizationRule(name: "member", where: { OR: [
    { node: { members: { some: { isViewer: true } } } }
    { node: { owner: { isViewer: true } } } ] })
  @authorization(filter: [{ where: { OR: [{ rule: "member" }, { rule: "admin" }] } }]) {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN)
}
type PackingItem @node
  @authorization(filter: [{ where: { OR: [{ node: { trip: { rule: "member" } } }, { rule: "admin" }] } }]) {
  key: String! @key
  trip: Trip! @relationship(type: "PACKS", direction: OUT)
}`;
const handWritten = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node { key: String! @key }
type Trip @node
  @authorization(filter: [{ where: { OR: [
    { OR: [
      { node: { members: { some: { key: { eq: "$jwt.sub" } } } } }
      { node: { owner: { key: { eq: "$jwt.sub" } } } } ] }
    { jwt: { roles: { includes: "admin" } } } ] } }]) {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN)
}
type PackingItem @node
  @authorization(filter: [{ where: { OR: [
    { node: { trip: { OR: [
      { members: { some: { key: { eq: "$jwt.sub" } } } }
      { owner: { key: { eq: "$jwt.sub" } } } ] } } }
    { jwt: { roles: { includes: "admin" } } } ] } }]) {
  key: String! @key
  trip: Trip! @relationship(type: "PACKS", direction: OUT)
}`;
const seed = [
  "CREATE (lou:Person {key: 'lou'}), (bo:Person {key: 'bo'}), (cy:Person {key: 'cy'}), (t:Trip {key: 't'}), (lou)-[:OWNS]->(t), (bo)-[:MEMBER]->(t), (:PackingItem {key: 'tent'})-[:PACKS]->(t)",
];
const as = (sub: string, roles: string[] = []) => ({ jwt: { sub, roles } });

describe("named rules", () => {
  test("apply on their type and through a relationship", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + named, seed });
    const see = async (ctx: Record<string, unknown>) =>
      t.data<{ trips: unknown[]; packingItems: unknown[] }>(
        "{ trips { key } packingItems { key } }",
        {},
        ctx,
      );
    for (const who of ["lou", "bo"]) {
      expect(await see(as(who))).toEqual({
        trips: [{ key: "t" }],
        packingItems: [{ key: "tent" }],
      });
    }
    expect(await see(as("cy"))).toEqual({ trips: [], packingItems: [] });
    expect(await see(as("cy", ["admin"]))).toEqual({
      trips: [{ key: "t" }],
      packingItems: [{ key: "tent" }],
    });
  });

  test("compile exactly as the hand-written rules", async () => {
    await expectSameStatements(claims + named, handWritten, [
      { query: "{ trips { key } }", context: as("bo") },
      { query: "{ trips { key } }", context: as("cy", ["admin"]) },
      { query: "{ packingItems { key trip { key } } }", context: as("bo") },
      { query: "{ packingItems { key } }", context: as("cy", ["admin"]) },
      { query: '{ trip(key: "t") { key } }', context: as("lou") },
    ]);
  });
});

describe("with @authorizationDefaults", () => {
  test("bypass and the mutations default can name rules", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        `extend schema @authorizationDefaults(bypass: { rule: "admin" }, mutations: { rule: "admin" })\n` +
        named +
        "\ntype Genre @node @mutation { key: String! @key }",
      seed,
    });
    const genre = `mutation { createGenres(input: [{ key: "g" }]) { genres { key } } }`;
    expect(
      (await t.run(genre, {}, as("bo"))).errors?.[0]?.extensions?.["code"],
    ).toBe("FORBIDDEN");
    expect(
      (await t.run(genre, {}, as("bo", ["admin"]))).errors,
    ).toBeUndefined();
    const [compiled] = t.lora.compile(
      "{ trips { key } }",
      {},
      {
        context: as("cy", ["admin"]),
      },
    );
    expect(compiled!.compiled.statements[0]!.text).not.toMatch(/MEMBER|OWNS/);
  });
});

describe("model checks", () => {
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test("an unknown name is refused", () => {
    expect(
      problems(
        claims +
          named.replace(
            '{ node: { trip: { rule: "member" } } }, { rule: "admin" }',
            '{ node: { trip: { rule: "member" } } }, { rule: "nope" }',
          ),
      ),
    ).toContain(
      '@authorization: unknown rule "nope" (not a rule of PackingItem or of the schema)',
    );
    expect(
      problems(
        claims +
          named.replace('trip: { rule: "member" }', 'trip: { rule: "guest" }'),
      ),
    ).toContain('@authorization: unknown rule "guest" (not a rule of Trip)');
  });

  test("a cycle is refused with its chain", () => {
    const sdl = `type A @node @authorizationRule(name: "r", where: { node: { b: { rule: "r" } } }) {
      key: String! @key  b: B @relationship(type: "AB", direction: OUT) }
    type B @node @authorizationRule(name: "r", where: { node: { a: { rule: "r" } } }) {
      key: String! @key  a: A @relationship(type: "AB", direction: IN) }`;
    // Reported once, and nothing else about it.
    expect(problems(sdl)).toEqual([
      "@authorization: rule cycle: A.r → B.r → A.r",
    ]);
  });

  test("shadowing and misplaced rules are refused", () => {
    expect(
      problems(
        claims +
          named.replace(
            '@authorizationRule(name: "member"',
            '@authorizationRule(name: "admin", where: { node: { key: { eq: "x" } } })\n  @authorizationRule(name: "member"',
          ),
      ),
    ).toContain(
      '@authorizationRule: rule "admin" shadows the schema rule of that name; rename one',
    );
    expect(
      problems(
        claims.replace(
          '{ jwt: { roles: { includes: "admin" } } }',
          '{ node: { key: { eq: "x" } } }',
        ) + named,
      ),
    ).toContain(
      '@authorizationRules: rule "admin" tests claims only (jwt, AND, OR, NOT); node is not allowed (a node rule belongs on its type, with @authorizationRule)',
    );
    expect(
      problems(
        claims +
          named.replace(
            "{ node: { owner: { isViewer: true } } } ] })",
            '{ jwt: { roles: { includes: "x" } } } ] })',
          ),
      ),
    ).toContain(
      "@authorization: rule Trip.member tests jwt; only a rule that tests node alone can be used inside a node filter",
    );
  });
});
