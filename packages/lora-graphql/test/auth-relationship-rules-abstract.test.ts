// Rules on a relationship field whose target is an interface or a union:
// they hold per member, with that member as the rule's `target`, and from
// whichever side a write comes.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const codes = (r: Result) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

const typeDefs = (target: string) => `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}
type Person @node @mutation {
  key: String! @key
  open: Boolean
  boards: [Board!]! @relationship(type: "MEMBER", direction: OUT, properties: "Seat")
}
type Team @node @mutation {
  key: String! @key
  open: Boolean
}
union Member = Person | Team
type Seat @relationshipProperties {
  role: String
}
type Board @node @mutation {
  key: String! @key
  owner: String
  members: [Member!]! @relationship(type: "MEMBER", direction: IN, properties: "Seat")
    @authorization(validate: [
      { operations: [CONNECT], where: { source: { owner: { eq: "$jwt.sub" } }, target: ${target} } }
      { operations: [DISCONNECT], where: { OR: [{ source: { owner: { eq: "$jwt.sub" } } }, { target: { isViewer: true } }] } }
      { operations: [UPDATE_EDGE], where: { target: { isViewer: true } } }
    ])
}`;
const open = typeDefs("{ open: { eq: true } }");
const seed = [
  "CREATE (:Board {key: 'b', owner: 'lou'}), (:Person {key: 'lou', open: true}), (:Person {key: 'bo', open: true}), (:Person {key: 'shy', open: false}), (:Team {key: 't', open: true}), (:Team {key: 'closed', open: false})",
];
const as = (sub: string) => ({ jwt: { sub } });
const setup = () => createTestLoraGraphQL({ typeDefs: open, seed });
const members = async (t: Awaited<ReturnType<typeof setup>>) =>
  (
    (await t.db.execute(
      "MATCH (m)-[s:MEMBER]->(:Board {key: 'b'}) RETURN m.key AS k, s.role AS role ORDER BY k",
    )) as { rows: Array<Record<string, unknown>> }
  ).rows;

const connect = (member: "Person" | "Team", key: string, edge = "") =>
  `mutation { updateBoard(key: "b", update: { members: { connect: { ${member}: [{ key: "${key}"${edge} }] } } }) { board { key } } }`;
const join = (who: string) =>
  `mutation { updatePerson(key: "${who}", update: { boards: { connect: [{ key: "b" }] } }) { person { key } } }`;

describe("CONNECT to a union", () => {
  test("each member answers as the target", async () => {
    const t = await setup();
    expect((await t.run(connect("Person", "bo"), {}, as("lou"))).errors).toBe(
      undefined,
    );
    expect((await t.run(connect("Team", "t"), {}, as("lou"))).errors).toBe(
      undefined,
    );
    expect(codes(await t.run(connect("Person", "shy"), {}, as("lou")))).toEqual(
      ["FORBIDDEN"],
    );
    expect(
      codes(await t.run(connect("Team", "closed"), {}, as("lou"))),
    ).toEqual(["FORBIDDEN"]);
    expect((await members(t)).map((m) => m["k"])).toEqual(["bo", "t"]);
  });

  test("the source's part holds too", async () => {
    const t = await setup();
    const r = await t.run(connect("Person", "bo"), {}, as("bo"));
    expect(codes(r)).toEqual(["FORBIDDEN"]);
    expect(r.errors?.[0]?.message).toBe("not allowed to connect Board.members");
    expect(await members(t)).toEqual([]);
  });

  test("holds from the member's own field", async () => {
    const t = await setup();
    // bo is not the board's owner: joining from Person.boards is refused
    // by Board.members' rule.
    expect(codes(await t.run(join("bo"), {}, as("bo")))).toEqual(["FORBIDDEN"]);
    expect((await t.run(join("lou"), {}, as("lou"))).errors).toBe(undefined);
    expect((await members(t)).map((m) => m["k"])).toEqual(["lou"]);
  });

  test("a create that connects answers to it", async () => {
    const t = await setup();
    const create = (key: string, owner: string, who: string) =>
      `mutation { createBoards(input: [{ key: "${key}", owner: "${owner}", members: { connect: { Person: [{ key: "${who}" }] } } }]) { boards { key } } }`;
    expect((await t.run(create("n1", "lou", "bo"), {}, as("lou"))).errors).toBe(
      undefined,
    );
    expect(
      codes(await t.run(create("n2", "lou", "shy"), {}, as("lou"))),
    ).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(create("n3", "bo", "bo"), {}, as("lou")))).toEqual(
      ["FORBIDDEN"],
    );
  });
});

describe("DISCONNECT and UPDATE_EDGE on a union", () => {
  const seeded = [
    ...seed,
    "MATCH (b:Board {key: 'b'}), (p:Person {key: 'bo'}), (t:Team {key: 't'}) CREATE (p)-[:MEMBER {role: 'guest'}]->(b), (t)-[:MEMBER]->(b)",
  ];
  const remove = (member: "Person" | "Team", key: string) =>
    `mutation { updateBoard(key: "b", update: { members: { disconnect: { ${member}: ["${key}"] } } }) { board { key } } }`;
  // A union field has no nested `update`: a re-connect that sets a
  // property is its edge update.
  const role = (key: string) =>
    connect("Person", key, ', edge: { role: "chair" }');

  test("isViewer on the target is true of the viewer's member only", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: open, seed: seeded });
    // bo leaves; bo may not remove the team (not the owner, and a Team is
    // never the viewer).
    expect(codes(await t.run(remove("Team", "t"), {}, as("bo")))).toEqual([
      "FORBIDDEN",
    ]);
    expect((await t.run(remove("Person", "bo"), {}, as("bo"))).errors).toBe(
      undefined,
    );
    expect((await t.run(remove("Team", "t"), {}, as("lou"))).errors).toBe(
      undefined,
    );
    expect(await members(t)).toEqual([]);
  });

  test("only the member changes their own seat", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: open, seed: seeded });
    expect(codes(await t.run(role("bo"), {}, as("lou")))).toEqual([
      "FORBIDDEN",
    ]);
    expect((await t.run(role("bo"), {}, as("bo"))).errors).toBe(undefined);
    expect(await members(t)).toEqual([
      { k: "bo", role: "chair" },
      { k: "t", role: null },
    ]);
  });
});

describe("startup", () => {
  test("a target field one member lacks names the member", () => {
    const bad = typeDefs("{ open: { eq: true } }").replace(
      "type Team @node @mutation {\n  key: String! @key\n  open: Boolean",
      "type Team @node @mutation {\n  key: String! @key",
    );
    let error: unknown;
    try {
      buildModel(bad);
    } catch (err) {
      error = err;
    }
    expect(error).toBeInstanceOf(ModelError);
    expect((error as ModelError).message).toMatch(/Board\.members/);
    expect((error as ModelError).message).toMatch(/Team/);
  });

  test("the same operation ruled from a member's side is refused", () => {
    const both = open.replace(
      'boards: [Board!]! @relationship(type: "MEMBER", direction: OUT, properties: "Seat")',
      'boards: [Board!]! @relationship(type: "MEMBER", direction: OUT, properties: "Seat") @authorization(validate: [{ operations: [CONNECT], where: { source: { isViewer: true } } }])',
    );
    expect(() => buildModel(both)).toThrow(ModelError);
  });
});

describe("an interface target", () => {
  const viaInterface = open
    .replace(
      "union Member = Person | Team",
      "interface Member { key: String!  open: Boolean }",
    )
    .replace(
      "type Person @node @mutation {",
      "type Person implements Member @node @mutation {",
    )
    .replace(
      "type Team @node @mutation {",
      "type Team implements Member @node @mutation {",
    );

  test("each implementation answers as the target", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: viaInterface, seed });
    expect((await t.run(connect("Team", "t"), {}, as("lou"))).errors).toBe(
      undefined,
    );
    expect(codes(await t.run(connect("Person", "shy"), {}, as("lou")))).toEqual(
      ["FORBIDDEN"],
    );
    expect(codes(await t.run(connect("Person", "bo"), {}, as("bo")))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(join("bo"), {}, as("bo")))).toEqual(["FORBIDDEN"]);
  });

  test("the access matrix and check() take the field", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: viaInterface, seed });
    expect(() => t.lora.accessMatrix()).not.toThrow();
    await expect(t.lora.check()).resolves.toBeDefined();
  });
});
