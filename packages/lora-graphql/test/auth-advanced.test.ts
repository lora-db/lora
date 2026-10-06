import { buildModel } from "../src/index.js";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)
  type Claims @jwt {
    sub: String!
    roles: [String!] @jwtClaim(path: "app.roles")
    tenant: String
  }

  type Account
    @node
    @mutation
    @authorization(
      filter: [{ where: { node: { tenant: { eq: "$context.tenant" } } } }]
    ) {
    key: String! @key
    tenant: String!
    owner: String!
    salary: Int
      @sortable
      @filterable(byValue: [EQ, GT])
      @authorization(
        validate: [
          {
            operations: [READ, UPDATE]
            where: {
              OR: [
                { node: { owner: { eq: "$jwt.sub" } } }
                { jwt: { roles: { includes: "hr" } } }
              ]
            }
          }
        ]
      )
    tags: [Tag!]! @relationship(type: "TAGGED", direction: OUT)
  }

  type Tag
    @node
    @mutation
    @authentication(operations: [CREATE], jwt: { roles: { includes: "admin" } })
    @authorization(
      filter: [
        { operations: [READ], where: { node: { key: { startsWith: "" } } } }
        {
          operations: [CREATE_RELATIONSHIP, DELETE_RELATIONSHIP]
          where: { NOT: { node: { locked: { eq: true } } } }
        }
      ]
    ) {
    key: String! @key
    locked: Boolean
  }
`;

const seed = [
  `CREATE (:Account {key: 'a1', tenant: 't1', owner: 'alice', salary: 100}),
          (:Account {key: 'a2', tenant: 't1', owner: 'bob', salary: 200}),
          (:Account {key: 'a3', tenant: 't2', owner: 'alice', salary: 300}),
          (:Tag {key: 'open', locked: false}), (:Tag {key: 'frozen', locked: true})`,
];

let h: Harness;
beforeEach(async () => {
  h = await festivalHarness({ typeDefs, seed });
});

const alice = { jwt: { sub: "alice" }, tenant: "t1" };
const hr = { jwt: { sub: "hana", app: { roles: ["hr"] } }, tenant: "t1" };
const admin = { jwt: { sub: "root", app: { roles: ["admin"] } }, tenant: "t1" };

test("$context values in rules: tenant isolation", async () => {
  const d = await h.data<{ accounts: Array<{ key: string }> }>(
    `{ accounts(sort: [{ key: ASC }]) { key } }`,
    {},
    alice,
  );
  expect(d.accounts.map((a) => a.key)).toEqual(["a1", "a2"]);
  const none = await h.data<{ accounts: unknown[] }>(
    `{ accounts { key } }`,
    {},
    { jwt: { sub: "x" } },
  );
  expect(none.accounts).toEqual([]); // no tenant in the context: the rule denies
});

test("field-level rules: reading, filtering, sorting", async () => {
  const own = await h.data<{ account: unknown }>(
    `{ account(key: "a1") { salary } }`,
    {},
    alice,
  );
  expect(own.account).toEqual({ salary: 100 });
  const other = await h.run(`{ account(key: "a2") { salary } }`, {}, alice);
  expect(other.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
  const all = await h.data<{ accounts: unknown[] }>(
    `{ accounts(sort: [{ key: ASC }]) { salary } }`,
    {},
    hr,
  );
  expect(all.accounts).toEqual([{ salary: 100 }, { salary: 200 }]);
  // Filtering cannot probe salaries of accounts the caller may not read.
  const probe = await h.data<{ accounts: unknown[] }>(
    `{ accounts(where: { salary: { gt: 150 } }) { key } }`,
    {},
    alice,
  );
  expect(probe.accounts).toEqual([]);
  const sort = await h.run(
    `{ accounts(sort: [{ salary: DESC }]) { key } }`,
    {},
    alice,
  );
  expect(sort.errors?.[0]?.message).toBe(
    "cannot sort by Account.salary: it has row-level read rules",
  );
});

test("field-level rules on writes", async () => {
  const r = await h.run(
    `mutation { updateAccount(key: "a2", update: { salary: 1 }) { info { nodesUpdated } } }`,
    {},
    alice,
  );
  expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
  const ok = await h.data<{ updateAccount: unknown }>(
    `mutation { updateAccount(key: "a2", update: { salary: 250 }) { info { nodesUpdated } } }`,
    {},
    hr,
  );
  expect(ok.updateAccount).toEqual({ info: { nodesUpdated: 1 } });
});

test("@authentication(jwt:) and @jwtClaim paths", async () => {
  const denied = await h.run(
    `mutation { createTags(input: [{ key: "t" }]) { info { nodesCreated } } }`,
    {},
    alice,
  );
  expect(denied.errors?.[0]?.extensions?.["code"]).toBe("UNAUTHENTICATED");
  const ok = await h.data<{ createTags: unknown }>(
    `mutation { createTags(input: [{ key: "t" }]) { info { nodesCreated } } }`,
    {},
    admin,
  );
  expect(ok.createTags).toEqual({ info: { nodesCreated: 1 } });
});

test("CREATE_RELATIONSHIP and DELETE_RELATIONSHIP rules guard connects", async () => {
  const ok = await h.data<{ updateAccount: unknown }>(
    `mutation { updateAccount(key: "a1", update: { tags: { connect: [{ key: "open" }] } }) { info { relationshipsCreated } } }`,
    {},
    alice,
  );
  expect(ok.updateAccount).toEqual({ info: { relationshipsCreated: 1 } });
  const locked = await h.run(
    `mutation { updateAccount(key: "a1", update: { tags: { connect: [{ key: "frozen" }] } }) { info { relationshipsCreated } } }`,
    {},
    alice,
  );
  expect(locked.errors?.[0]?.extensions?.["code"]).toBe("NOT_FOUND");
  await h.db.execute(
    "MATCH (a:Account {key: 'a1'}), (t:Tag {key: 'frozen'}) CREATE (a)-[:TAGGED]->(t)",
  );
  const keep = await h.data<{ updateAccount: unknown }>(
    `mutation { updateAccount(key: "a1", update: { tags: { disconnect: ["frozen"] } }) { info { relationshipsDeleted } } }`,
    {},
    alice,
  );
  expect(keep.updateAccount).toEqual({ info: { relationshipsDeleted: 0 } });
});

test("a missing claim is unknown: false where it stands, never true under NOT", () => {
  const model = buildModel(`
    type D @node @authorization(filter: [
      { where: { OR: [{ node: { owner: { eq: "$jwt.sub" } } }, { jwt: { roles: { includes: "admin" } } }] } }
      { where: { NOT: { jwt: { banned: { eq: true } } } } }
    ]) { key: ID! @key owner: String }
  `);
  expect(model.nodes.get("D")!.authorization!.filter).toHaveLength(2);
});

test("@jwt: rules may only use declared claims", () => {
  expect(() =>
    buildModel(`
      type C @jwt { sub: String }
      type D @node @authorization(filter: [{ where: { jwt: { role: { eq: "x" } }, node: { k: { eq: "$jwt.team" } } } }]) {
        key: ID! @key k: String
      }
    `),
  ).toThrow(
    /jwt\.role: not a claim of the @jwt type[\s\S]*\$jwt\.team is not a claim of the @jwt type/,
  );
});

test("upsert does not reveal hidden keys: they read as a denied create", async () => {
  // "Taken" would tell alice that a3 exists in another tenant (G-20).
  const r = await h.run(
    `mutation { upsertAccounts(input: [{ key: "a3", tenant: "t1", owner: "alice" }]) { info { nodesCreated nodesUpdated } } }`,
    {},
    alice,
  );
  expect(r.errors?.map((e) => [e.extensions?.["code"], e.message])).toEqual([
    ["FORBIDDEN", "not allowed to create this Account"],
  ]);
  const a3 = await h.db.execute(
    "MATCH (a:Account {key: 'a3'}) RETURN a.tenant AS tenant, a.owner AS owner",
  );
  expect(a3.rows).toEqual([{ tenant: "t2", owner: "alice" }]);
});
