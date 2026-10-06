// One test per defect found by the adversarial review.

import { buildModel, type WriteChange } from "../src/index.js";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)
  type User @node @mutation @query(aggregate: true) {
    key: String! @key
    email: String @unique @sortable
    secret: Int
      @authentication(operations: [READ])
      @filterable(byValue: [EQ, GT])
      @sortable
    posts: [Post!]! @relationship(type: "WROTE", direction: OUT) @filterable
    follows: [User!]! @relationship(type: "FOLLOWS", direction: OUT)
  }
  type Post
    @node
    @mutation
    @authorization(filter: [{ where: { node: { published: { eq: true } } } }]) {
    key: String! @key
    published: Boolean!
  }
  type Doc
    @node
    @query(aggregate: true)
    @authorization(
      validate: [
        { operations: [READ], where: { node: { owner: { eq: "$jwt.sub" } } } }
      ]
    ) {
    key: String! @key
    owner: String!
    salary: Int @sortable
  }
  type Note
    @node
    @authorization(
      filter: [{ where: { NOT: { node: { team: { eq: "$jwt.team" } } } } }]
    ) {
    key: String! @key
    team: String
  }
  type Secret
    @node
    @mutation(operations: [CREATE])
    @authentication(operations: [READ]) {
    key: String! @key
    v: Int
  }
  type Mutation {
    touchDoc(key: String!): Doc
      @cypher(
        statement: "MATCH (d:Doc) WHERE d.key = $key SET d.touched = true RETURN d"
      )
  }
`;

const seed = [
  `CREATE (a:User {key: 'alice', email: 'a@x', secret: 42})
   CREATE (:User {key: 'bob', secret: 7}), (:User {key: 'u3'}), (:User {key: 'u4'}),
          (:User {key: 'u5'}), (:User {key: 'u6'})
   CREATE (a)-[:WROTE]->(:Post {key: 'p-pub', published: true}),
          (a)-[:WROTE]->(:Post {key: 'p-draft', published: false})
   CREATE (:Doc {key: 'd1', owner: 'alice', salary: 99999})
   CREATE (:Note {key: 'n1', team: 'red'})`,
];

let h: Harness;
const changes: WriteChange[] = [];
beforeEach(async () => {
  h = await festivalHarness({ typeDefs, seed });
  changes.length = 0;
  h.lora.onWrite((c) => changes.push(c));
});

const code = async (source: string, context: Record<string, unknown> = {}) =>
  (await h.run(source, {}, context)).errors?.[0]?.extensions?.["code"];
const bob = { jwt: { sub: "bob" } };
const alice = { jwt: { sub: "alice" } };
const count = async (cypher: string) =>
  Number((await h.db.execute(cypher)).rows[0]!["c"]);

test("1. field @authentication covers aggregates, filters and sorts", async () => {
  expect(await code(`{ usersAggregate { secret { max } } }`)).toBe(
    "UNAUTHENTICATED",
  );
  expect(await code(`{ users(where: { secret: { gt: 40 } }) { key } }`)).toBe(
    "UNAUTHENTICATED",
  );
  expect(
    await code(
      `{ usersConnection(sort: [{ secret: DESC }]) { pageInfo { endCursor } } }`,
    ),
  ).toBe("UNAUTHENTICATED");
  expect(
    await code(`{ usersAggregate { secret { max } } }`, alice),
  ).toBeUndefined();
});

test("2. READ validate rules cover aggregates, totalCount and cursors", async () => {
  expect(await code(`{ docsAggregate { count salary { max } } }`, bob)).toBe(
    "FORBIDDEN",
  );
  expect(await code(`{ docsConnection { totalCount } }`, bob)).toBe(
    "FORBIDDEN",
  );
  expect(
    await code(
      `{ docsConnection(sort: [{ salary: DESC }]) { pageInfo { endCursor } } }`,
      bob,
    ),
  ).toBe("FORBIDDEN");
  expect(
    await code(
      `{ docsAggregate { count } docsConnection { totalCount } }`,
      alice,
    ),
  ).toBeUndefined();
});

test("3. @cypher mutation payloads are validated, and a failure rolls back", async () => {
  expect(
    await code(`mutation { touchDoc(key: "d1") { key salary } }`, bob),
  ).toBe("FORBIDDEN");
  expect(
    await count("MATCH (d:Doc) WHERE d.touched = true RETURN count(d) AS c"),
  ).toBe(0);
});

test("4. mutation payloads respect type @authentication(READ)", async () => {
  expect(
    await code(
      `mutation { createSecrets(input: [{ key: "s1", v: 1 }]) { secrets { key v } } }`,
    ),
  ).toBe("UNAUTHENTICATED");
  expect(await count("MATCH (s:Secret) RETURN count(s) AS c")).toBe(0);
});

test("5. NOT around a rule needing a missing claim still denies", async () => {
  const d = await h.data<{ notes: unknown[] }>(
    `{ notes { key } }`,
    {},
    { jwt: { sub: "x" } },
  );
  expect(d.notes).toEqual([]);
  const withTeam = await h.data<{ notes: unknown[] }>(
    `{ notes { key } }`,
    {},
    { jwt: { sub: "x", team: "blue" } },
  );
  expect(withTeam.notes).toEqual([{ key: "n1" }]);
});

test("6. disconnect leaves hidden nodes alone", async () => {
  const d = await h.data<{ updateUser: unknown }>(
    `mutation { updateUser(key: "alice", update: { posts: { disconnect: ["p-draft", "p-pub"] } }) { info { relationshipsDeleted } } }`,
  );
  expect(d.updateUser).toEqual({ info: { relationshipsDeleted: 1 } });
  expect(
    await count(
      "MATCH (:User {key: 'alice'})-[r:WROTE]->() RETURN count(r) AS c",
    ),
  ).toBe(1);
});

test("7. connecting a pair twice creates one relationship", async () => {
  const d = await h.data<{ updateUser: unknown }>(
    `mutation { updateUser(key: "alice", update: { follows: { connect: [{ key: "bob" }, { key: "bob" }] } }) { info { relationshipsCreated } } }`,
  );
  expect(d.updateUser).toEqual({ info: { relationshipsCreated: 1 } });
  expect(
    await count(
      "MATCH (:User {key: 'alice'})-[r:FOLLOWS]->() RETURN count(r) AS c",
    ),
  ).toBe(1);
});

test("8. delete events include relationships declared on the other type", async () => {
  await h.data(`mutation { deletePost(key: "p-pub") { nodesDeleted } }`);
  const c = changes[0]!;
  expect(
    c.disconnected.map(
      (r) => `${r.field} ${String(r.from.key)}->${String(r.to.key)}`,
    ),
  ).toEqual(["User.posts alice->p-pub"]);
  expect(c.entities.map((e) => `${e.type}:${String(e.key)}`).sort()).toEqual([
    "Post:p-pub",
    "User:alice",
  ]);
  expect(c.relationshipTypes).toEqual(["WROTE"]);
});

test("9. a nullable @unique sort still pages through every row", async () => {
  const keys: string[] = [];
  let after: string | null = null;
  for (let i = 0; i < 10; i++) {
    const d: {
      usersConnection: {
        edges: Array<{ node: { key: string } }>;
        pageInfo: { hasNextPage: boolean; endCursor: string };
      };
    } = await h.data(
      `query ($after: String) { usersConnection(first: 1, after: $after, sort: [{ email: ASC }]) { edges { node { key } } pageInfo { hasNextPage endCursor } } }`,
      { after },
    );
    keys.push(...d.usersConnection.edges.map((e) => e.node.key));
    if (!d.usersConnection.pageInfo.hasNextPage) break;
    after = d.usersConnection.pageInfo.endCursor;
  }
  expect(keys).toEqual(["alice", "bob", "u3", "u4", "u5", "u6"]);
});

test("10. a relationship filter whose inner filter is all null is left out", async () => {
  const d = await h.data<{ users: unknown[] }>(
    `{ users(limit: 50, where: { posts: { some: { key: { eq: null } } } }) { key } }`,
  );
  expect(d.users).toHaveLength(6);
});

test("11. rules that test nothing are refused at startup", () => {
  expect(() =>
    buildModel(`
      type P @node @authorization(filter: [{ where: { node: { owner: {} } } }, { where: { node: { k: { eq: null } } } }]) {
        key: ID! @key
        k: String
        owner: P @relationship(type: "OWNS", direction: IN)
      }
    `),
  ).toThrow(/node\.owner is empty[\s\S]*node\.k\.eq is null/);
});
