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

describe("G-6: re-connecting keeps a relationship's properties", () => {
  const typeDefs = `
    type User @node @mutation(operations: [CREATE, UPDATE]) {
      key: String! @key
      plan: [Slot!]! @relationship(type: "PLANS", direction: OUT, properties: "Planned")
      home: Slot @relationship(type: "HOME", direction: OUT, properties: "Planned")
    }
    type Slot @node { key: String! @key }
    type Planned @relationshipProperties {
      reminder: Boolean! @default(value: false)
      note: String
    }`;
  const seed = [
    "CREATE (:User {key:'u'}), (:Slot {key:'s'}), (:Slot {key:'t'})",
  ];

  test("a list relationship keeps what a re-connect does not set", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(
      `mutation { updateUser(key: "u", update: { plan: { connect: [{ key: "s", edge: { reminder: true, note: "n" } }] } }) { user { key } } }`,
    );
    const again = await t.data(
      `mutation { updateUser(key: "u", update: { plan: { connect: [{ key: "s" }, { key: "t" }] } })
        { info { relationshipsCreated } user { planConnection(sort: [{ key: ASC }]) { edges { node { key } properties { reminder note } } } } } }`,
    );
    expect(again).toEqual({
      updateUser: {
        info: { relationshipsCreated: 1 },
        user: {
          planConnection: {
            edges: [
              { node: { key: "s" }, properties: { reminder: true, note: "n" } },
              // A new relationship still gets the default.
              {
                node: { key: "t" },
                properties: { reminder: false, note: null },
              },
            ],
          },
        },
      },
    });
    // What a re-connect does set, it sets.
    await t.data(
      `mutation { updateUser(key: "u", update: { plan: { connect: [{ key: "s", edge: { note: "m" } }] } }) { user { key } } }`,
    );
    expect(
      await cypher(
        t,
        "MATCH (:User)-[r:PLANS]->(:Slot {key:'s'}) RETURN r.reminder AS r, r.note AS n",
      ),
    ).toEqual([{ r: true, n: "m" }]);
    t.close();
  });

  test("a single relationship re-connected to its target keeps it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const home = (edge: string, key = "s") =>
      t.data(
        `mutation { updateUser(key: "u", update: { home: { connect: { key: "${key}"${edge} } } })
          { info { relationshipsCreated relationshipsDeleted } } }`,
      );
    const stored = () =>
      cypher(
        t,
        "MATCH (:User {key:'u'})-[r:HOME]->(s:Slot) RETURN s.key AS s, r.reminder AS reminder",
      );
    await home(", edge: { reminder: true }");
    expect(await home("")).toEqual({
      updateUser: {
        info: { relationshipsCreated: 0, relationshipsDeleted: 0 },
      },
    });
    expect(await stored()).toEqual([{ s: "s", reminder: true }]);
    // Replacing it with another target is a new relationship.
    expect(await home("", "t")).toEqual({
      updateUser: {
        info: { relationshipsCreated: 1, relationshipsDeleted: 1 },
      },
    });
    expect(await stored()).toEqual([{ s: "t", reminder: false }]);
    t.close();
  });
});

describe("G-19: claims inside rule strings", () => {
  const typeDefs = (where: string) =>
    J +
    `type PlannedSet @node @mutation
      @authorization(validate: [{ operations: [CREATE, UPDATE], where: ${where} }]) {
      key: String! @key
      slot: String
    }`;
  const prefix = `{ node: { key: { startsWith: "\${jwt.sub}:" } } }`;
  const create = (key: string) =>
    `mutation { createPlannedSets(input: [{ key: "${key}" }]) { plannedSets { key } } }`;
  const as = (sub: string, roles: string[] = []) => ({ jwt: { sub, roles } });
  const stored = (t: Test) =>
    cypher(t, "MATCH (p:PlannedSet) RETURN p.key AS k ORDER BY k");

  test("a user cannot take another user's key space", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: typeDefs(prefix) });
    // The workaround `startsWith: "$jwt.sub"` let p1 take p10's keys.
    expect(codes(await t.run(create("p10:slot"), {}, as("p1")))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(create("p1slot"), {}, as("p1")))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(create("p1:slot")))).toEqual(["UNAUTHENTICATED"]);
    expect(await stored(t)).toEqual([]);
    t.close();
  });

  test("a user can create and update keys in their own space", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: typeDefs(prefix) });
    await t.data(create("p1:slot"), {}, as("p1"));
    await t.data(create("p10:slot"), {}, as("p10"));
    await t.data(
      `mutation { updatePlannedSet(key: "p1:slot", update: { slot: "x" }) { plannedSet { slot } } }`,
      {},
      as("p1"),
    );
    const other = await t.run(
      `mutation { updatePlannedSet(key: "p10:slot", update: { slot: "x" }) { plannedSet { slot } } }`,
      {},
      as("p1"),
    );
    expect(codes(other)).toEqual(["FORBIDDEN"]);
    expect(await stored(t)).toEqual([{ k: "p10:slot" }, { k: "p1:slot" }]);
    t.close();
  });

  test("a missing or non-scalar claim denies, under NOT too", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(
        `{ OR: [{ node: { key: { startsWith: "\${jwt.roles}" } } }, { NOT: { node: { key: { startsWith: "\${jwt.roles}" } } } }] }`,
      ),
    });
    expect(
      codes(await t.run(create("admin:x"), {}, as("p1", ["admin"]))),
    ).toEqual(["FORBIDDEN"]);
    expect(await stored(t)).toEqual([]);
    t.close();
  });

  test("context values interpolate into filter rules", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Doc @node @authorization(filter: [{ requireAuthentication: false,
        where: { node: { key: { startsWith: "\${context.tenant}/" } } } }]) { key: String! @key }`,
      seed: [
        "CREATE (:Doc {key: 'acme/1'}), (:Doc {key: 'acme2/1'}), (:Doc {key: 'other/1'})",
      ],
    });
    const docs = (tenant: string | undefined) =>
      t.data("{ docs { key } }", {}, tenant === undefined ? {} : { tenant });
    expect(await docs("acme")).toEqual({ docs: [{ key: "acme/1" }] });
    expect(await docs("other")).toEqual({ docs: [{ key: "other/1" }] });
    expect(await docs(undefined)).toEqual({ docs: [] });
    t.close();
  });

  test("malformed references are model errors", () => {
    const build = (where: string) => () =>
      new LoraGraphQL({
        typeDefs: typeDefs(where),
        driver: undefined as never,
      });
    expect(build(`{ node: { key: { startsWith: "$jwt.sub-" } } }`)).toThrow(
      /"\$jwt\.sub-" is not a jwt reference; to put one inside a string write "\$\{jwt\.sub\}-"/,
    );
    expect(build(`{ node: { key: { startsWith: "\${jwt.nope}:" } } }`)).toThrow(
      /\$jwt\.nope is not a claim of the @jwt type/,
    );
    expect(build(`{ node: { key: { startsWith: "\${sub}:" } } }`)).toThrow(
      /a placeholder is \$\{jwt\.<claim>\} or \$\{context\.<path>\}/,
    );
    expect(build(`{ jwt: { sub: { startsWith: "\${context.x}" } } }`)).toThrow(
      /placeholders belong in node parts/,
    );
  });
});

describe("G-11: the required-relationship check seeks and sees other edges", () => {
  const typeDefs = `
    type Person @node @mutation(operations: [UPDATE]) {
      key: String! @key
      sent: [Message!]! @relationship(type: "SENT", direction: OUT)
    }
    type Message @node @mutation(operations: [UPDATE]) {
      key: String! @key
      from: Person! @relationship(type: "SENT", direction: IN)
    }`;
  const seed = [
    "CREATE (a:Person {key: 'a'}), (b:Person {key: 'b'}), (a)-[:SENT]->(:Message {key: 'm1'}), (b)-[:SENT]->(:Message {key: 'm2'})",
  ];

  test("disconnecting leaves no message without a sender", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      `mutation { updatePerson(key: "a", update: { sent: { disconnect: ["m1"] } }) { person { key } } }`,
    );
    // m2 still has its SENT: the check must look at m1's own.
    expect(codes(r)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(r.errors![0]!.message).toBe(
      'Message "m1" requires a Person (Message.from)',
    );
    expect(
      await cypher(
        t,
        "MATCH (:Person)-[:SENT]->(m:Message) RETURN m.key AS k ORDER BY k",
      ),
    ).toEqual([{ k: "m1" }, { k: "m2" }]);
    t.close();
  });

  test("the check binds the count in the WITH, so it keeps the key seek", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.run(
      `mutation { updatePerson(key: "a", update: { sent: { disconnect: ["m1"] } }) { person { key } } }`,
    );
    const check = t.statements.find((s) => s.statement.text.includes("= 0"));
    expect(check?.statement.text).toMatch(
      /WITH a, size\(\[\(a\)<-\[:`?SENT`?\]-\(x\) WHERE .* \| 1\]\) AS related WHERE related = 0/,
    );
    t.close();
  });
});

describe("G-7: field READ rules compile per row; check() takes a context", () => {
  const typeDefs =
    J +
    `type Person @node {
      key: String! @key
      name: String
      saved: [Post!]! @relationship(type: "SAVED", direction: OUT)
        @authorization(validate: [{ operations: [READ], where: { node: { key: { eq: "$jwt.sub" } } } }])
      email: String @authentication(operations: [READ])
    }
    type Post @node { key: String! @key }`;
  const seed = [
    "CREATE (a:Person {key: 'a', email: 'a@x'})-[:SAVED]->(:Post {key: 'p'}), (:Person {key: 'b'})",
  ];
  const query = `{ person(key: "a") { key saved { key } } }`;

  test("an anonymous compile of a private field plans like a signed-in one", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const [anonymous] = t.lora.compile(query);
    const [signedIn] = t.lora.compile(
      query,
      {},
      {
        context: { jwt: { sub: "a" } },
      },
    );
    expect(anonymous!.compiled.statements).toHaveLength(1);
    expect(anonymous!.compiled.statements[0]!.text).toMatch(/__forbidden/);
    expect(signedIn!.compiled.statements[0]!.text).toMatch(/__forbidden/);
    await expectSeeks(t.lora, query);
    t.close();
  });

  test("each row enforces the rule", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(await t.data(query, {}, member)).toEqual({
      person: { key: "a", saved: [{ key: "p" }] },
    });
    const other = await t.run(query, {}, { jwt: { sub: "b", roles: [] } });
    expect(codes(other)).toEqual(["FORBIDDEN"]);
    expect(other.errors![0]!.message).toBe("not allowed to read Person.saved");
    expect(other.data).toEqual({ person: null });
    const connection = await t.run(
      `{ person(key: "a") { savedConnection { totalCount } } }`,
      {},
      { jwt: { sub: "b", roles: [] } },
    );
    expect(codes(connection)).toEqual(["FORBIDDEN"]);
    const anonymous = await t.run(query);
    expect(codes(anonymous)).toEqual(["UNAUTHENTICATED"]);
    expect(anonymous.errors![0]!.message).toBe(
      "Person.saved needs an authenticated request",
    );
    // @authentication on a field reads the same way.
    expect(codes(await t.run(`{ person(key: "a") { email } }`))).toEqual([
      "UNAUTHENTICATED",
    ]);
    expect(await t.data(`{ person(key: "a") { email } }`, {}, member)).toEqual({
      person: { email: "a@x" },
    });
    // Nothing selected, nothing refused: other fields stay readable.
    expect(await t.data(`{ person(key: "b") { key name } }`)).toEqual({
      person: { key: "b", name: null },
    });
    t.close();
  });

  test("check() compiles each operation under its own context", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const report = await t.lora.check({
      operations: [
        { name: "anonymous", document: query },
        { name: "owner", document: query, context: { jwt: { sub: "s-1" } } },
      ],
    });
    expect(report.errors).toEqual([]);
    expect(report.ok).toBe(true);
    const params = (name: string) =>
      Object.values(
        report.plans.find((p) => p.operation === name)!.reports[0]!.statement
          .params,
      );
    // The owner's statement carries the claim the rule compares with.
    expect(params("owner")).toContain("s-1");
    expect(params("anonymous")).not.toContain("s-1");
    t.close();
  });
});

describe("G-14: a nested create is visible to a filter on its new edge", () => {
  const typeDefs =
    J +
    `type User @node @mutation {
      key: String! @key
      sent: [Request!]! @relationship(type: "SENT", direction: OUT, nestedOperations: [CREATE, CONNECT])
    }
    type Request @node @mutation
      @authorization(filter: [{ where: { node: { from: { key: { eq: "$jwt.sub" } } } } }]) {
      key: String! @key
      from: User! @relationship(type: "SENT", direction: IN)
    }`;
  const seed = ["CREATE (:User {key: 'a'}), (:User {key: 'b'})"];

  test("the created node and its edge are there, and readable", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.data(
      `mutation { updateUser(key: "a", update: { sent: { create: [{ node: { key: "r1" } }] } })
        { user { sent { key from { key } } } } }`,
      {},
      member,
    );
    expect(r).toEqual({
      updateUser: { user: { sent: [{ key: "r1", from: { key: "a" } }] } },
    });
    t.close();
  });

  test("an existing node the caller cannot see still cannot be connected", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(
      `mutation { updateUser(key: "a", update: { sent: { create: [{ node: { key: "r1" } }] } }) { user { key } } }`,
      {},
      member,
    );
    const r = await t.run(
      `mutation { updateUser(key: "b", update: { sent: { connect: [{ key: "r1" }] } }) { user { key } } }`,
      {},
      { jwt: { sub: "b", roles: [] } },
    );
    expect(codes(r)).toEqual(["NOT_FOUND"]);
    expect(
      await cypher(t, "MATCH (u:User)-[:SENT]->(:Request) RETURN u.key AS u"),
    ).toEqual([{ u: "a" }]);
    t.close();
  });
});

describe("G-4 / G-12: an input with nothing in it is left out", () => {
  const fieldsOf = (t: Test, name: string) =>
    Object.keys(
      (
        t.schema.getType(name) as never as {
          getFields(): Record<string, unknown>;
        }
      ).getFields(),
    );

  test("nothing updatable: no update input, no update mutations, a warning", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `
        type Hashtag @node @mutation(operations: [CREATE, UPDATE]) {
          key: String! @key
          name: String! @settable(onCreate: true, onUpdate: false)
          posts: [Post!]! @relationship(type: "TAGGED", direction: IN, nestedOperations: [])
        }
        type Post @node { key: String! @key }`,
    });
    expect(await t.data("{ hashtags { key } }")).toEqual({ hashtags: [] });
    expect(t.schema.getType("HashtagUpdateInput")).toBeUndefined();
    const mutations = Object.keys(t.schema.getMutationType()!.getFields());
    expect(mutations).toContain("createHashtags");
    expect(mutations).not.toContain("updateHashtag");
    expect(t.lora.model.warnings).toContainEqual({
      type: "Hashtag",
      message: expect.stringMatching(/nothing is updatable/),
    });
    await t.data(
      `mutation { createHashtags(input: [{ key: "h", name: "H" }]) { hashtags { key } } }`,
    );
    t.close();
  });

  test("a nested update of a type with nothing updatable is left out too", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `
        type Post @node @mutation(operations: [UPDATE]) {
          key: String! @key
          title: String
          tag: Tag @relationship(type: "TAGGED", direction: OUT, nestedOperations: [CONNECT, UPDATE])
        }
        type Tag @node @mutation(operations: [UPDATE]) {
          key: String! @key
          name: String @readonly
        }`,
    });
    expect(t.schema.getType("TagUpdateInput")).toBeUndefined();
    expect(fieldsOf(t, "PostTagUpdateRelationInput")).toEqual(["connect"]);
    expect(await t.data("{ posts { key } }")).toEqual({ posts: [] });
    t.close();
  });

  test("no settable relationship property: no edge input", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `
        type Person @node @mutation {
          key: String! @key
          channels: [Channel!]! @relationship(type: "MEMBER_OF", direction: OUT, properties: "Membership")
        }
        type Channel @node { key: String! @key }
        type Membership @relationshipProperties {
          role: String! @default(value: "member") @readonly
        }`,
      seed: ["CREATE (:Channel {key: 'c'})"],
    });
    expect(await t.data("{ persons { key } }")).toEqual({ persons: [] });
    expect(t.schema.getType("MembershipCreateInput")).toBeUndefined();
    expect(fieldsOf(t, "PersonChannelsConnectInput")).toEqual(["key"]);
    await t.data(
      `mutation { createPersons(input: [{ key: "a", channels: { connect: [{ key: "c" }] } }]) { persons { key } } }`,
    );
    expect(
      await cypher(
        t,
        "MATCH (:Person)-[r:MEMBER_OF]->(:Channel) RETURN r.role AS role",
      ),
    ).toEqual([{ role: "member" }]);
    t.close();
  });
});

describe("G-13: @timestamp on relationship properties", () => {
  const typeDefs = `
    type Person @node @mutation {
      key: String! @key
      channels: [Channel!]! @relationship(type: "MEMBER_OF", direction: OUT, properties: "Membership")
    }
    type Channel @node @mutation { key: String! @key }
    type Membership @relationshipProperties {
      joinedAt: DateTime @timestamp(operations: [CREATE])
      changedAt: DateTime @timestamp(operations: [UPDATE])
      note: String
    }`;
  const seed = ["CREATE (:Person {key: 'a'}), (:Channel {key: 'c'})"];
  const stamps = async (t: Test) =>
    (
      await cypher(
        t,
        "MATCH (:Person)-[r:MEMBER_OF]->(:Channel) RETURN toString(r.joinedAt) AS joined, toString(r.changedAt) AS changed, r.note AS note",
      )
    )[0];
  const connect = (edge: string) =>
    `mutation { updatePerson(key: "a", update: { channels: { connect: [{ key: "c"${edge} }] } })
      { person { channelsConnection { edges { properties { joinedAt note } } } } } }`;

  test("CREATE stamps are set when the relationship is created, and kept", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.data<{
      updatePerson: {
        person: {
          channelsConnection: {
            edges: Array<{ properties: { joinedAt: string; note: string } }>;
          };
        };
      };
    }>(connect(`, edge: { note: "x" }`));
    const edge = r.updatePerson.person.channelsConnection.edges[0]!.properties;
    expect(edge.note).toBe("x");
    expect(Date.parse(edge.joinedAt)).toBeGreaterThan(Date.now() - 60_000);
    const first = await stamps(t);
    expect(first!["changed"]).toBeNull();
    // A re-connect without properties changes nothing.
    await new Promise((r) => setTimeout(r, 5));
    await t.data(connect(""));
    expect(await stamps(t)).toEqual(first);
    t.close();
  });

  test("UPDATE stamps follow edge updates and re-connects that set properties", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(connect(`, edge: { note: "x" }`));
    const created = await stamps(t);
    await t.data(connect(`, edge: { note: "y" }`));
    const reconnected = await stamps(t);
    expect(reconnected!["changed"]).not.toBeNull();
    expect(reconnected!["joined"]).toBe(created!["joined"]);
    await new Promise((r) => setTimeout(r, 5));
    await t.data(
      `mutation { updatePerson(key: "a", update: { channels: { update: [{ key: "c", edge: { note: "z" } }] } }) { person { key } } }`,
    );
    const updated = await stamps(t);
    expect(updated!["note"]).toBe("z");
    expect(updated!["changed"]! > reconnected!["changed"]!).toBe(true);
    t.close();
  });

  test("a nested create stamps its relationship", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(
      `mutation { updatePerson(key: "a", update: { channels: { create: [{ node: { key: "n" } }] } }) { person { key } } }`,
    );
    expect(
      await cypher(
        t,
        "MATCH (:Person)-[r:MEMBER_OF]->(:Channel {key: 'n'}) RETURN r.joinedAt IS NOT NULL AS stamped",
      ),
    ).toEqual([{ stamped: true }]);
    t.close();
  });
});

describe("G-16: re-upserting single relationships is batched", () => {
  const typeDefs = `
    type Msg @node @mutation {
      key: String! @key
      text: String
      conv: Conv! @relationship(type: "IN", direction: OUT)
    }
    type Conv @node @mutation { key: String! @key }`;
  const seed = ["CREATE (:Conv {key: 'c'}), (:Conv {key: 'd'})"];
  const upsert = (conv: string) =>
    `mutation { upsertMsgs(input: [${Array.from(
      { length: 200 },
      (_, i) => `{ key: "m${i}", conv: { connect: { key: "${conv}" } } }`,
    ).join(" ")}]) { info { relationshipsCreated relationshipsDeleted } } }`;

  test("an identical re-upsert runs a constant number of statements and writes nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(upsert("c"));
    t.statements.length = 0;
    expect(await t.data(upsert("c"))).toEqual({
      upsertMsgs: {
        info: { relationshipsCreated: 0, relationshipsDeleted: 0 },
      },
    });
    expect(t.statements.length).toBeLessThanOrEqual(8);
    t.close();
  });

  test("re-pointing every row is batched too, and stays single", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(upsert("c"));
    t.statements.length = 0;
    expect(await t.data(upsert("d"))).toEqual({
      upsertMsgs: {
        info: { relationshipsCreated: 200, relationshipsDeleted: 200 },
      },
    });
    expect(t.statements.length).toBeLessThanOrEqual(8);
    expect(
      await cypher(
        t,
        "MATCH (m:Msg)-[:IN]->(c:Conv) RETURN c.key AS c, count(m) AS n",
      ),
    ).toEqual([{ c: "d", n: 200 }]);
    t.close();
  });
});

describe("G-2: generateTypes merges a field selected twice", () => {
  test("one field with the sub-selections of every mention", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `
        type Genre @node { key: String! @key  name: String!  family: String }
        type Festival @node {
          key: String! @key
          genre: Genre @relationship(type: "IN_GENRE", direction: OUT)
        }`,
    });
    const types = t.lora.generateTypes(
      t.lora.buildManifest({
        q: "query Q { festivals { genre { key name } ...F } } fragment F on Festival { genre { key family } }",
      }),
    );
    const result = types.slice(types.indexOf("export interface QResult"));
    expect(result).toContain("genre: {");
    expect(result).not.toMatch(/\} \| null \| \{/);
    const genre = result.slice(result.indexOf("genre: {"));
    expect(genre.slice(0, genre.indexOf("}"))).toMatch(
      /key: string;\s+name: string;\s+family: string \| null;/,
    );
    t.close();
  });
});
