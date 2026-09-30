import { authSeed, authTypeDefs } from "./fixtures.js";
import { festivalHarness, type Harness } from "./harness.js";
import { unknownField } from "./graphql-messages.js";

let h: Harness;
beforeEach(async () => {
  h = await festivalHarness({ typeDefs: authTypeDefs, seed: authSeed });
});

const alice = { jwt: { sub: "alice" } };
const bob = { jwt: { sub: "bob" } };
const admin = { jwt: { sub: "root", roles: ["admin"] } };

const postKeys = async (context: Record<string, unknown>) =>
  (
    await h.data<{ posts: Array<{ key: string }> }>(
      `{ posts(sort: [{ key: ASC }]) { key } }`,
      {},
      context,
    )
  ).posts.map((p) => p.key);

describe("filter rules", () => {
  test("each reader sees what the rules grant", async () => {
    expect(await postKeys({})).toEqual(["a-pub"]);
    expect(await postKeys(alice)).toEqual(["a-draft", "a-pub"]);
    expect(await postKeys(bob)).toEqual(["a-pub", "b-draft"]);
    expect(await postKeys(admin)).toEqual(["a-draft", "a-pub", "b-draft"]);
  });

  test("apply to nested reads, lookups, counts and relationship filters", async () => {
    const d = await h.data<Record<string, unknown>>(
      `{
        user(key: "alice") { posts(sort: [{ key: ASC }]) { key } }
        post(key: "b-draft") { key }
        postsConnection { totalCount }
        users(where: { key: { eq: "bob" } }) { key }
      }`,
      {},
      alice,
    );
    expect(d).toEqual({
      user: { posts: [{ key: "a-draft" }, { key: "a-pub" }] },
      post: null,
      postsConnection: { totalCount: 2 },
      users: [{ key: "bob" }],
    });
  });

  test("claims fold into constants; missing claims deny", () => {
    const [anon] = h.lora.compile(`{ posts { key } }`);
    const [root] = h.lora.compile(`{ posts { key } }`, {}, { context: admin });
    // The admin rule is decided in JavaScript: no filter reaches Cypher.
    expect(root!.compiled.statements[0]!.text).not.toMatch(/published|WROTE/);
    // Anonymous: only the rule without requireAuthentication remains.
    expect(anon!.compiled.statements[0]!.text).toMatch(
      /this\.published = \$p\d/,
    );
    expect(anon!.compiled.statements[0]!.text).not.toMatch(/WROTE/);
  });

  test("fields with @authentication need a jwt", async () => {
    const r = await h.run(`{ post(key: "a-pub") { secretNotes } }`);
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("UNAUTHENTICATED");
    const d = await h.data<{ post: unknown }>(
      `{ post(key: "a-pub") { secretNotes } }`,
      {},
      alice,
    );
    expect(d.post).toEqual({ secretNotes: "s1" });
  });
});

describe("validate rules on mutations", () => {
  const create = (author: string) => `mutation {
    createPosts(input: [{ title: "New", author: { connect: { key: "${author}" } } }]) {
      posts { title published author { key } }
    }
  }`;

  test("anonymous writes need authentication", async () => {
    const r = await h.run(create("alice"));
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("UNAUTHENTICATED");
  });

  test("AFTER: a user may create only their own posts", async () => {
    const ok = await h.data<{ createPosts: unknown }>(
      create("alice"),
      {},
      alice,
    );
    expect(ok.createPosts).toEqual({
      posts: [{ title: "New", published: false, author: { key: "alice" } }],
    });
    const r = await h.run(create("bob"), {}, alice);
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    const count = await h.db.execute(
      "MATCH (p:Post {title: 'New'}) RETURN count(p) AS c",
    );
    expect(count.rows[0]!["c"]).toBe(1); // the forbidden create rolled back
  });

  test("BEFORE: published posts are visible to all, editable by their author", async () => {
    const r = await h.run(
      `mutation { updatePost(key: "a-pub", update: { title: "Hijacked" }) { post { title } } }`,
      {},
      bob,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    const d = await h.data<{ updatePost: unknown }>(
      `mutation { updatePost(key: "a-pub", update: { title: "Edited" }) { post { title } } }`,
      {},
      alice,
    );
    expect(d.updatePost).toEqual({ post: { title: "Edited" } });
  });

  test("filter rules hide nodes from updates and deletes", async () => {
    const d = await h.data<{ deletePost: unknown }>(
      `mutation { deletePost(key: "b-draft") { nodesDeleted } }`,
      {},
      alice,
    );
    expect(d.deletePost).toEqual({ nodesDeleted: 0 });
    const byAdmin = await h.data<{ deletePost: unknown }>(
      `mutation { deletePost(key: "b-draft") { nodesDeleted relationshipsDeleted } }`,
      {},
      admin,
    );
    expect(byAdmin.deletePost).toEqual({
      nodesDeleted: 1,
      relationshipsDeleted: 1,
    });
  });
});

describe("rules are checked at startup", () => {
  test("unknown fields and operators in a rule", async () => {
    const { buildModel } = await import("../src/index.js");
    expect(() =>
      buildModel(`
        type A @node @authorization(filter: [{ where: { node: { nope: { eq: 1 } }, jwt: { sub: { like: "x" } } } }]) {
          key: ID! @key
        }
      `),
    ).toThrow(
      /A: @authorization: node\.nope: A has no field nope[\s\S]*A: @authorization: jwt\.sub: unknown operator like/,
    );
  });
});

describe("more rules", () => {
  const typeDefs = /* GraphQL */ `
    type Doc
      @node
      @mutation
      @authorization(
        validate: [
          {
            operations: [READ]
            where: { node: { level: { lte: "$jwt.clearance" } } }
          }
        ]
      ) {
      key: String! @key
      level: Int!
      audit: String @readonly
    }
    type Folder @node @mutation {
      key: String! @key
      docs: [Doc!]! @relationship(type: "HOLDS", direction: OUT)
    }
  `;
  let t: Harness;
  beforeEach(async () => {
    t = await festivalHarness({
      typeDefs,
      seed: [
        `CREATE (:Doc {key: 'low', level: 1, audit: 'ok'}), (:Doc {key: 'high', level: 9})`,
      ],
    });
  });

  test("READ validate rules fail the request instead of hiding rows", async () => {
    const ok = await t.data<{ doc: unknown }>(
      `{ doc(key: "low") { key audit } }`,
      {},
      { jwt: { clearance: 5 } },
    );
    expect(ok.doc).toEqual({ key: "low", audit: "ok" });
    const r = await t.run(
      `{ docs(sort: [{ key: ASC }]) { key } }`,
      {},
      { jwt: { clearance: 5 } },
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
  });

  test("@readonly fields are readable but not settable", async () => {
    const r = await t.run(
      `mutation { createDocs(input: [{ key: "x", level: 1, audit: "forged" }]) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.message).toMatch(
      unknownField("audit", "DocCreateInput"),
    );
  });

  test("connect targets must exist", async () => {
    const r = await t.run(
      `mutation { createFolders(input: [{ key: "f", docs: { connect: [{ key: "low" }, { key: "gone" }] } }]) { info { nodesCreated } } }`,
    );
    expect(r.errors?.[0]?.message).toBe('Folder.docs: no Doc with key "gone"');
    const folders = await t.db.execute("MATCH (f:Folder) RETURN count(f) AS c");
    expect(folders.rows[0]!["c"]).toBe(0);
  });
});
