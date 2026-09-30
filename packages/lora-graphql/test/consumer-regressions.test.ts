// Regressions from a production integration (items G-1 to G-19 of the
// Festimap brief). Each block names its item; the suite runs on graphql
// 16 and 17 (`yarn test`, `yarn test:graphql17`).

import { describe, expect, test } from "vitest";
import { LoraGraphQL, ModelError } from "../src/index.js";
import { createTestLoraGraphQL, expectSeeks } from "../src/testing.js";

const J = "type Claims @jwt { sub: String!  roles: [String!] }\n";
const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
type Test = Awaited<ReturnType<typeof createTestLoraGraphQL>>;
/** Rows of a raw Cypher query against the test database. */
const cypher = async (t: Test, query: string) =>
  ((await t.db.execute(query)) as { rows: Array<Record<string, unknown>> })
    .rows;
const admin = { jwt: { sub: "root", roles: ["admin"] } };
const member = { jwt: { sub: "a", roles: [] } };

describe("G-1: compile, explain, check and expectSeeks take variables", () => {
  const typeDefs = `type F @node { key: String! @key  name: String @filterable }`;
  const seed =
    "UNWIND range(1, 30) AS i CREATE (:F {key: toString(i), name: 'n' + toString(i)})";

  test("on a root argument", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const [root] = t.lora.compile(
      "query Q($k: String!) { f(key: $k) { key } }",
      { k: "7" },
    );
    expect(Object.values(root!.compiled.statements[0]!.params)).toContain("7");
    await expectSeeks(t.lora, "query Q($k: String!) { f(key: $k) { key } }", {
      k: "7",
    });
    t.close();
  });

  test("inside an input object", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const [root] = t.lora.compile(
      "query Q($n: String) { fs(where: { name: { eq: $n } }) { key } }",
      { n: "n3" },
    );
    expect(root!.compiled.statements[0]!.text).toMatch(/\.name = \$p\d/);
    expect(Object.values(root!.compiled.statements[0]!.params)).toContain("n3");
    const report = await t.lora.check({
      operations: [
        {
          document:
            "query Q($n: String) { fs(where: { name: { eq: $n } }) { key } }",
          variables: { n: "n3" },
        },
      ],
    });
    expect(report.errors).toEqual([]);
    // The filter reached the statement, so the plan seeks the name index.
    expect(report.plans[0]!.reports[0]!.statement.text).toMatch(
      /\.name = \$p\d/,
    );
    expect(
      report.plans.flatMap((p) => p.reports.flatMap((r) => r.findings)),
    ).toEqual([]);
    t.close();
  });
});

describe("G-9: field rules on relationship properties are enforced", () => {
  const typeDefs =
    J +
    `type Person @node @mutation(operations: [CREATE, UPDATE]) {
      key: String! @key
      channels: [Channel!]!
        @relationship(type: "MEMBER_OF", direction: OUT, properties: "Membership")
    }
    type Channel @node { key: String! @key }
    type Membership @relationshipProperties {
      role: String
        @authorization(validate: [{ operations: [CREATE, UPDATE], where: { jwt: { roles: { includes: "admin" } } } }])
      note: String
      secret: String @filterable @sortable
        @authorization(validate: [{ operations: [READ], where: { jwt: { roles: { includes: "admin" } } } }])
    }`;
  const seed = [
    "CREATE (:Person {key:'a'}), (:Channel {key:'c'}), (:Channel {key:'d'})",
  ];
  const connect = (edge: string) =>
    `mutation { updatePerson(key: "a", update: { channels: { connect: [{ key: "c", edge: ${edge} }] } })
      { person { channelsConnection { edges { properties { role note } } } } } }`;
  const roles = (t: Test) =>
    cypher(
      t,
      "MATCH (:Person {key:'a'})-[r:MEMBER_OF]->(c:Channel) RETURN c.key AS c, r.role AS role ORDER BY c",
    );

  test("a non-admin cannot set the property on a new relationship", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(connect(`{ role: "owner" }`), {}, member);
    expect(codes(r)).toEqual(["FORBIDDEN"]);
    expect(r.errors![0]!.message).toMatch(/Membership\.role/);
    expect(await roles(t)).toEqual([]);
    // Anonymous: the rule needs a token.
    expect(codes(await t.run(connect(`{ role: "owner" }`)))).toEqual([
      "UNAUTHENTICATED",
    ]);
    // Other properties stay writable.
    const ok = await t.run(connect(`{ note: "hi" }`), {}, member);
    expect(ok.errors).toBeUndefined();
    t.close();
  });

  test("a non-admin cannot change it on an existing relationship", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(connect(`{ role: "member" }`), {}, admin);
    const reconnect = await t.run(connect(`{ role: "owner" }`), {}, member);
    expect(codes(reconnect)).toEqual(["FORBIDDEN"]);
    const update = await t.run(
      `mutation { updatePerson(key: "a", update: { channels: { update: [{ key: "c", edge: { role: "owner" } }] } }) { person { key } } }`,
      {},
      member,
    );
    expect(codes(update)).toEqual(["FORBIDDEN"]);
    expect(await roles(t)).toEqual([{ c: "c", role: "member" }]);
    t.close();
  });

  test("a nested create with the property is refused too", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs
        .replace(
          'properties: "Membership")',
          'properties: "Membership", nestedOperations: [CONNECT, CREATE, UPDATE])',
        )
        .replace("type Channel @node {", "type Channel @node @mutation {"),
      seed,
    });
    const r = await t.run(
      `mutation { updatePerson(key: "a", update: { channels: { create: [{ node: { key: "n" }, edge: { role: "owner" } }] } }) { person { key } } }`,
      {},
      member,
    );
    expect(codes(r)).toEqual(["FORBIDDEN"]);
    expect(await cypher(t, "MATCH (c:Channel {key: 'n'}) RETURN c")).toEqual(
      [],
    );
    t.close();
  });

  test("an admin can create and update it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const created = await t.data(connect(`{ role: "owner" }`), {}, admin);
    expect(created).toEqual({
      updatePerson: {
        person: {
          channelsConnection: {
            edges: [{ properties: { role: "owner", note: null } }],
          },
        },
      },
    });
    await t.data(connect(`{ role: "member" }`), {}, admin);
    await t.data(
      `mutation { updatePerson(key: "a", update: { channels: { update: [{ key: "c", edge: { role: "editor" } }] } }) { person { key } } }`,
      {},
      admin,
    );
    expect(await roles(t)).toEqual([{ c: "c", role: "editor" }]);
    t.close();
  });

  test("READ rules guard reading, filtering and sorting by the property", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed: [
        ...seed,
        "MATCH (p:Person {key:'a'}), (c:Channel {key:'c'}) CREATE (p)-[:MEMBER_OF {secret: 's', note: 'n'}]->(c)",
      ],
    });
    const read = `{ person(key: "a") { channelsConnection { edges { properties { note secret } } } } }`;
    const denied = await t.run(read, {}, member);
    expect(codes(denied)).toEqual(["FORBIDDEN"]);
    expect(denied.errors![0]!.message).toMatch(/Membership\.secret/);
    expect(await t.data(read, {}, admin)).toEqual({
      person: {
        channelsConnection: {
          edges: [{ properties: { note: "n", secret: "s" } }],
        },
      },
    });
    // Properties without rules stay readable.
    expect(
      await t.data(
        `{ person(key: "a") { channelsConnection { edges { properties { note } } } } }`,
        {},
        member,
      ),
    ).toBeTruthy();
    const filter = `{ person(key: "a") { channelsConnection(where: { edge: { secret: { eq: "s" } } }) { totalCount } } }`;
    expect(codes(await t.run(filter, {}, member))).toEqual(["FORBIDDEN"]);
    expect((await t.run(filter, {}, admin)).errors).toBeUndefined();
    const sort = `{ person(key: "a") { channelsConnection(sort: [{ edge: { secret: ASC } }]) { totalCount } } }`;
    expect(codes(await t.run(sort, {}, member))).toEqual(["FORBIDDEN"]);
    expect((await t.run(sort, {}, admin)).errors).toBeUndefined();
    t.close();
  });

  test("rules on relationship properties test claims only", () => {
    const build = (rule: string) =>
      new LoraGraphQL({
        typeDefs:
          J +
          `type P @node { key: String! @key
            cs: [C!]! @relationship(type: "M", direction: OUT, properties: "M") }
          type C @node { key: String! @key }
          type M @relationshipProperties { role: String @authorization(${rule}) }`,
        driver: undefined as never,
      });
    expect(() =>
      build(
        `validate: [{ operations: [CREATE], where: { node: { key: { eq: "$jwt.sub" } } } }]`,
      ),
    ).toThrow(
      /M\.role: @authorization: node: rules on relationship properties test claims \(jwt\) only/,
    );
    expect(() =>
      build(`filter: [{ where: { jwt: { roles: { includes: "a" } } } }]`),
    ).toThrow(ModelError);
    expect(() =>
      build(
        `validate: [{ operations: [DELETE], where: { jwt: { roles: { includes: "a" } } } }]`,
      ),
    ).toThrow(/apply to READ, CREATE and UPDATE, not DELETE/);
    expect(() =>
      build(
        `validate: [{ operations: [CREATE], where: { jwt: { nope: { eq: "a" } } } }]`,
      ),
    ).toThrow(/not a claim of the @jwt type/);
  });
});

describe("G-10: @settable and @readonly on relationship fields", () => {
  const typeDefs = `
    type Request @node @mutation {
      key: String! @key
      note: String
      from: Person! @relationship(type: "SENT", direction: IN) @settable(onCreate: true, onUpdate: false)
      to: Person @relationship(type: "TO", direction: OUT)
    }
    type Person @node { key: String! @key }`;
  const seed = [
    "CREATE (a:Person {key:'a'}), (:Person {key:'b'}), (a)-[:SENT]->(:Request {key:'r'})",
  ];
  const sender = (t: Test) =>
    cypher(
      t,
      "MATCH (p:Person)-[:SENT]->(:Request {key:'r'}) RETURN p.key AS k",
    );

  test("an update cannot re-point a relationship settable on create only", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      `mutation { updateRequest(key: "r", update: { from: { connect: { key: "b" } } }) { request { from { key } } } }`,
    );
    // Absent from RequestUpdateInput, like a scalar settable on create only.
    expect(r.errors?.[0]?.message).toMatch(/from/);
    expect(t.schema.getType("RequestUpdateInput")).toBeDefined();
    expect(
      Object.keys(
        (
          t.schema.getType("RequestUpdateInput") as never as {
            getFields(): Record<string, unknown>;
          }
        ).getFields(),
      ).sort(),
    ).toEqual(["note", "to"]);
    // An upsert of the existing node keeps the relationship it was created with.
    const up = await t.run(
      `mutation { upsertRequests(input: [{ key: "r", note: "x", from: { connect: { key: "b" } } }]) { requests { key note from { key } } } }`,
    );
    expect(up.errors).toBeUndefined();
    expect(up.data).toEqual({
      upsertRequests: {
        requests: [{ key: "r", note: "x", from: { key: "a" } }],
      },
    });
    expect(await sender(t)).toEqual([{ k: "a" }]);
    t.close();
  });

  test("create sets it, and other relationships stay updatable", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const created = await t.data(
      `mutation { createRequests(input: [{ key: "r2", from: { connect: { key: "b" } } }]) { requests { from { key } } } }`,
    );
    expect(created).toEqual({
      createRequests: { requests: [{ from: { key: "b" } }] },
    });
    const updated = await t.data(
      `mutation { updateRequest(key: "r", update: { to: { connect: { key: "b" } } }) { request { to { key } from { key } } } }`,
    );
    expect(updated).toEqual({
      updateRequest: { request: { to: { key: "b" }, from: { key: "a" } } },
    });
    t.close();
  });

  test("@readonly leaves it out of both inputs; value directives are refused", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs.replace(
        'to: Person @relationship(type: "TO", direction: OUT)',
        'to: Person @relationship(type: "TO", direction: OUT) @readonly',
      ),
      seed,
    });
    for (const input of ["RequestCreateInput", "RequestUpdateInput"]) {
      const fields = (
        t.schema.getType(input) as never as {
          getFields(): Record<string, unknown>;
        }
      ).getFields();
      expect(fields["to"]).toBeUndefined();
    }
    t.close();
    expect(
      () =>
        new LoraGraphQL({
          typeDefs: typeDefs.replace(
            "@settable(onCreate: true, onUpdate: false)",
            "@settable(onCreate: false)",
          ),
          driver: undefined as never,
        }),
    ).toThrow(
      /Request\.from: a required relationship must be settable on create/,
    );
    expect(
      () =>
        new LoraGraphQL({
          typeDefs: typeDefs.replace(
            "direction: OUT)",
            'direction: OUT) @default(value: "b")',
          ),
          driver: undefined as never,
        }),
    ).toThrow(/@default is not allowed on a relationship field/);
  });
});
