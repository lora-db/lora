// `isNull` in an @authorization rule's where: moderation hides a removed
// post (one with `removedAt` set) from everyone but an admin.
import { beforeAll, afterAll, describe, expect, test } from "vitest";
import { createTestLoraGraphQL, type TestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  type Claims @jwt {
    sub: String!
    roles: [String!]
  }
  type Post
    @node
    @query(aggregate: true)
    @authorization(
      filter: [
        {
          operations: [READ]
          where: {
            OR: [
              { node: { removedAt: { isNull: true } } }
              { jwt: { roles: { includes: "admin" } } }
            ]
          }
        }
      ]
    ) {
    key: ID! @key
    removedAt: DateTime
    comments: [Comment!]! @relationship(type: "ON", direction: IN)
  }
  type Comment
    @node
    @authorization(
      filter: [
        {
          operations: [READ]
          where: { node: { removedAt: { isNull: false } } }
        }
      ]
    ) {
    key: ID! @key
    removedAt: DateTime
    post: Post @relationship(type: "ON", direction: OUT)
  }
`;

const reader = { jwt: { sub: "u1" } };
const admin = { jwt: { sub: "a1", roles: ["admin"] } };

describe("isNull in a rule's where", () => {
  let t: TestLoraGraphQL;
  beforeAll(async () => {
    t = await createTestLoraGraphQL({
      typeDefs,
      seed: `CREATE (p1:Post {key: 'p1'}), (p2:Post {key: 'p2', removedAt: datetime('2026-10-01T00:00:00Z')}),
                    (:Comment {key: 'c1'})-[:ON]->(p1),
                    (:Comment {key: 'c2', removedAt: datetime('2026-10-01T00:00:00Z')})-[:ON]->(p1)`,
    });
  });
  afterAll(() => t.close());

  const read = `{
    posts { key }
    postsConnection { totalCount edges { node { key } } }
    removed: post(key: "p2") { key }
    kept: post(key: "p1") { key }
    postsAggregate { count }
  }`;

  test("a removed post is absent for a reader", async () => {
    expect(await t.data(read, {}, reader)).toEqual({
      posts: [{ key: "p1" }],
      postsConnection: { totalCount: 1, edges: [{ node: { key: "p1" } }] },
      removed: null,
      kept: { key: "p1" },
      postsAggregate: { count: 1 },
    });
    expect(
      t.statements.some((s) => /removedAt IS NULL/.test(s.statement.text)),
    ).toBe(true);
  });

  test("an admin sees it", async () => {
    expect(await t.data(read, {}, admin)).toEqual({
      posts: [{ key: "p1" }, { key: "p2" }],
      postsConnection: {
        totalCount: 2,
        edges: [{ node: { key: "p1" } }, { node: { key: "p2" } }],
      },
      removed: { key: "p2" },
      kept: { key: "p1" },
      postsAggregate: { count: 2 },
    });
  });

  test("isNull: false compiles to IS NOT NULL", async () => {
    t.statements.length = 0;
    expect(
      await t.data(`{ post(key: "p1") { comments { key } } }`, {}, reader),
    ).toEqual({ post: { comments: [{ key: "c2" }] } });
    expect(
      t.statements.some((s) => /removedAt IS NOT NULL/.test(s.statement.text)),
    ).toBe(true);
  });

  test("an isNull that is not a boolean is refused at startup", async () => {
    await expect(
      createTestLoraGraphQL({
        typeDefs: typeDefs.replace(
          "removedAt: { isNull: false }",
          'removedAt: { isNull: "yes" }',
        ),
      }),
    ).rejects.toThrow(/node\.removedAt\.isNull must be true or false/);
  });
});
