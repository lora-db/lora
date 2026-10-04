// An `OR` branch or a `NOT` left empty once absent and null filters are
// left out adds nothing; a literal `OR: []` still matches nothing. A
// relationship quantifier whose filter is left empty matches any related
// node: it narrows to "has one" (`some`), never widens to every row.

import { describe, expect, test } from "vitest";
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
    // An emptied quantifier filter matches any related node: `some` is
    // "has a friend" (c has none), never every row.
    expect(keysOf(await t.run(relNot), "fs")).toEqual(["a", "b"]);
    expect(keysOf(await t.run(relNot, { c: ["jp"] }), "fs")).toEqual(["b"]);
  });
});

describe("a quantifier emptied by null variables never widens to every row", () => {
  const typeDefs = `type Person @node { key: String! @key
      trips: [Trip!]! @relationship(type: "ON", direction: OUT, properties: "Seat") @filterable
      boss: Person @relationship(type: "BOSS", direction: OUT) @filterable }
    type Trip @node { key: String! @key @filterable }
    type Seat @relationshipProperties { row: Int @filterable }`;
  const seed = [
    "CREATE (ann:Person {key:'ann'}), (bob:Person {key:'bob'}), (cy:Person {key:'cy'}), (t1:Trip {key:'t1'}), (t2:Trip {key:'t2'})",
    "MATCH (ann:Person {key:'ann'}), (bob:Person {key:'bob'}), (t1:Trip {key:'t1'}), (t2:Trip {key:'t2'}) CREATE (ann)-[:ON {row: 1}]->(t1), (bob)-[:ON {row: 2}]->(t2), (bob)-[:BOSS]->(ann)",
  ];
  const decl = (where: string) => {
    const vars = [
      ...(where.includes("$t") ? ["$t: String"] : []),
      ...(where.includes("$n") ? ["$n: Int"] : []),
    ];
    return vars.length > 0 ? `(${vars.join(", ")})` : "";
  };
  const run = async (where: string, vars: Record<string, unknown> = {}) => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    return keysOf(
      await t.run(
        `query${decl(where)} { persons(where: ${where}, sort: [{ key: ASC }]) { key } }`,
        vars,
      ),
      "persons",
    );
  };

  test("some / none / single / all over a null variable", async () => {
    const t = { t: null };
    expect(await run("{ trips: { some: { key: { eq: $t } } } }", t)).toEqual([
      "ann",
      "bob",
    ]);
    expect(await run("{ trips: { none: { key: { eq: $t } } } }", t)).toEqual([
      "cy",
    ]);
    expect(await run("{ trips: { single: { key: { eq: $t } } } }", t)).toEqual([
      "ann",
      "bob",
    ]);
    expect(await run("{ trips: { all: { key: { eq: $t } } } }", t)).toEqual([
      "ann",
      "bob",
      "cy",
    ]);
    // With a value, the quantifier filters by it.
    expect(
      await run("{ trips: { some: { key: { eq: $t } } } }", { t: "t1" }),
    ).toEqual(["ann"]);
  });

  test("a literal empty quantifier means the same", async () => {
    expect(await run("{ trips: { some: {} } }")).toEqual(["ann", "bob"]);
    expect(await run("{ trips: { none: {} } }")).toEqual(["cy"]);
  });

  test("an absent or null quantifier is still left out", async () => {
    const all = ["ann", "bob", "cy"];
    expect(await run("{ trips: { some: null } }")).toEqual(all);
    expect(await run("{ trips: {} }")).toEqual(all);
  });

  test("a single relationship and a connection quantifier", async () => {
    expect(await run("{ boss: { key: { eq: $t } } }", { t: null })).toEqual([
      "bob",
    ]);
    expect(
      await run(
        "{ tripsConnection: { some: { edge: { row: { eq: $n } } } } }",
        { n: null },
      ),
    ).toEqual(["ann", "bob"]);
    expect(
      await run(
        "{ tripsConnection: { none: { node: { key: { eq: $t } } } } }",
        { t: null },
      ),
    ).toEqual(["cy"]);
    expect(
      await run(
        "{ tripsConnection: { some: { edge: { row: { eq: $n } } } } }",
        { n: 2 },
      ),
    ).toEqual(["bob"]);
  });
});
