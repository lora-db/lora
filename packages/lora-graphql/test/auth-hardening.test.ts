import { parse, subscribe, type ExecutionResult } from "graphql";
import { buildModel } from "../src/index.js";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Claims @jwt {
    sub: String!
    roles: [String!]
  }

  type Team @node @mutation {
    key: String! @key
    staff: [Member!]! @relationship(type: "STAFF", direction: OUT) @filterable
  }

  type Member
    @node
    @mutation
    @subscription
    @authorization(
      filter: [
        {
          operations: [SUBSCRIBE]
          where: { node: { owner: { eq: "$jwt.sub" } } }
        }
      ]
      validate: [
        {
          operations: [CREATE_RELATIONSHIP, DELETE_RELATIONSHIP]
          where: { node: { locked: { eq: false } } }
        }
      ]
    ) {
    key: String! @key
    owner: String!
    locked: Boolean!
    badge: String @settable(onUpdate: false)
    salary: Int
      @sortable
      @filterable(byValue: [EQ, IN])
      @authorization(
        validate: [
          { operations: [READ], where: { node: { owner: { eq: "$jwt.sub" } } } }
        ]
      )
    secret: String
      @authentication(operations: [READ], jwt: { roles: { includes: "admin" } })
  }

  type Query {
    adminCount: Int!
      @cypher(
        statement: "MATCH (p:Member) RETURN count(p) AS n"
        columnName: "n"
      )
      @authentication(jwt: { roles: { includes: "admin" } })
  }
`;

const seed = [
  `CREATE (p1:Member {key: 'p1', owner: 'alice', locked: false, badge: 'b1', salary: 100, secret: 's1'}),
          (p2:Member {key: 'p2', owner: 'bob', locked: true, salary: 200}),
          (t:Team {key: 't1'})
   CREATE (t)-[:STAFF]->(p1), (t)-[:STAFF]->(p2)`,
];

let h: Harness;
beforeEach(async () => {
  h = await festivalHarness({ typeDefs, seed });
});

const alice = { jwt: { sub: "alice" } };
const admin = { jwt: { sub: "root", roles: ["admin"] } };
const code = (r: ExecutionResult) => r.errors?.[0]?.extensions?.["code"];

test("an `in` filter on a ruled field does not bypass the rule", async () => {
  const d = await h.data<{ members: unknown[] }>(
    `{ members(where: { salary: { in: [100, 200] } }) { key } }`,
    {},
    alice,
  );
  expect(d.members).toEqual([{ key: "p1" }]);
});

test("aggregate filters over a ruled field are refused", async () => {
  const r = await h.run(
    `{ teams(where: { staff: { aggregate: { node: { salary: { max: { gt: 150 } } } } } }) { key } }`,
    {},
    alice,
  );
  expect(r.errors?.[0]?.message).toBe(
    "cannot aggregate Member.salary: it has row-level read rules",
  );
});

test("field @authentication(jwt:) holds on read and on @cypher fields", async () => {
  const r = await h.run(`{ member(key: "p1") { secret } }`, {}, alice);
  expect(code(r)).toBe("UNAUTHENTICATED");
  const ok = await h.data<{ member: unknown }>(
    `{ member(key: "p1") { secret } }`,
    {},
    admin,
  );
  expect(ok.member).toEqual({ secret: "s1" });
  expect(code(await h.run(`{ adminCount }`, {}, alice))).toBe(
    "UNAUTHENTICATED",
  );
  expect(await h.data(`{ adminCount }`, {}, admin)).toEqual({ adminCount: 2 });
});

test("field-level @authorization on relationship and @cypher fields takes READ rules only", () => {
  // Without operations a rule covers every operation, writes included.
  const rule = `@authorization(validate: [{ where: { jwt: { sub: { eq: "x" } } } }])`;
  const read = `@authorization(validate: [{ operations: [READ], where: { jwt: { sub: { eq: "x" } } } }])`;
  const rel = (r: string) => `
      type A @node { key: ID! @key bs: [B!]! @relationship(type: "R", direction: OUT) ${r} }
      type B @node { key: ID! @key }
    `;
  const cypher = (r: string) => `
      type A @node { key: ID! @key n: Int @cypher(statement: "RETURN 1 AS n", columnName: "n") ${r} }
    `;
  expect(() => buildModel(rel(rule))).toThrow(/takes READ rules, or CONNECT/);
  expect(() => buildModel(cypher(rule))).toThrow(/takes READ rules only/);
  expect(() => buildModel(rel(read))).not.toThrow();
  expect(() => buildModel(cypher(read))).not.toThrow();
});

test("CREATE_RELATIONSHIP and DELETE_RELATIONSHIP validate rules", async () => {
  await h.db.execute("CREATE (:Member {key: 'p3', owner: 'x', locked: false})");
  const ok = await h.data<{ updateTeam: unknown }>(
    `mutation { updateTeam(key: "t1", update: { staff: { connect: [{ key: "p3" }] } }) { info { relationshipsCreated } } }`,
    {},
    alice,
  );
  expect(ok.updateTeam).toEqual({ info: { relationshipsCreated: 1 } });
  await h.db.execute("CREATE (:Member {key: 'p4', owner: 'x', locked: true})");
  const connect = await h.run(
    `mutation { updateTeam(key: "t1", update: { staff: { connect: [{ key: "p4" }] } }) { info { relationshipsCreated } } }`,
    {},
    alice,
  );
  expect(code(connect)).toBe("FORBIDDEN");
  const disconnect = await h.run(
    `mutation { updateTeam(key: "t1", update: { staff: { disconnect: ["p2"] } }) { info { relationshipsDeleted } } }`,
    {},
    alice,
  );
  expect(code(disconnect)).toBe("FORBIDDEN");
  const { rows } = await h.db.execute(
    "MATCH (:Team {key: 't1'})-[r:STAFF]->(:Member {key: 'p2'}) RETURN count(r) AS n",
  );
  expect(rows).toEqual([{ n: 1 }]);
});

test("SUBSCRIBE filter rules decide which events reach a subscriber", async () => {
  const controller = new AbortController();
  const result = await subscribe({
    schema: h.lora.getSchema(),
    document: parse(`subscription { memberChanged { operation key } }`),
    contextValue: { ...alice, signal: controller.signal },
  });
  if (!(Symbol.asyncIterator in result)) throw result.errors?.[0];
  const events: unknown[] = [];
  const done = (async () => {
    for await (const r of result as AsyncIterable<ExecutionResult>) {
      events.push(r.data);
    }
  })();
  await h.data(
    `mutation { updateMembers(where: { key: { in: ["p1", "p2"] } }, update: { locked: true }) { info { nodesUpdated } } }`,
    {},
    admin,
  );
  await new Promise((r) => setTimeout(r, 20));
  controller.abort();
  await (result as AsyncGenerator).return?.(undefined);
  await done;
  expect(events).toEqual([
    { memberChanged: { operation: "UPDATE", key: "p1" } },
  ]);
});

test("bulk limit is capped at maxBatch and must be non-negative", async () => {
  const over = await h.run(
    `mutation { updateMembers(where: { key: { eq: "p1" } }, update: { locked: true }, limit: 5000) { info { nodesUpdated } } }`,
    {},
    admin,
  );
  expect(code(over)).toBe("LIMIT_EXCEEDED");
  const negative = await h.run(
    `mutation { deleteMembers(where: { key: { eq: "p1" } }, limit: -1) { nodesDeleted } }`,
    {},
    admin,
  );
  expect(code(negative)).toBe("BAD_USER_INPUT");
});

test("upsert keeps create-only fields of existing nodes", async () => {
  const d = await h.data<{ upsertMembers: { members: unknown[] } }>(
    `mutation {
      upsertMembers(input: [
        { key: "p1", owner: "alice", locked: false, badge: "new" }
        { key: "p9", owner: "alice", locked: false, badge: "b9" }
      ]) { members { key badge } }
    }`,
    {},
    alice,
  );
  expect(d.upsertMembers.members).toEqual([
    { key: "p1", badge: "b1" },
    { key: "p9", badge: "b9" },
  ]);
});
