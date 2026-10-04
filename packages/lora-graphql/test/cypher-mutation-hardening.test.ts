// @cypher fields and mutations, hardened: the caller as `$viewer`,
// claims-and-viewer rules on root fields, and capped list arguments.

import { execute, parse } from "graphql";
import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const problems = (sdl: string): string[] => {
  try {
    buildModel(sdl);
    return [];
  } catch (err) {
    if (err instanceof ModelError) return err.problems.map((p) => p.message);
    throw err;
  }
};

// The key is a slug; the subject is the identity provider's opaque id.
const bySubject = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "subject")
  roles: [String!]
}
`;
const byKey = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
`;
const types = `type Person @node {
  key: String! @key
  subject: String! @unique
  name: String
  verified: Boolean
}
type Post @node {
  key: String! @key
  caption: String!
  author: Person! @relationship(type: "POSTED", direction: IN)
}
type Query {
  me: Person @cypher(statement: "MATCH (p:Person) WHERE p.key = $viewer RETURN p")
  myKey: String @cypher(statement: "RETURN $viewer AS k")
}
type Mutation {
  createPost(key: String!, caption: String!): Post
    @authentication
    @cypher(statement: """
      MATCH (a:Person) WHERE a.key = $viewer
      CREATE (a)-[:POSTED]->(p:Post {key: $key, caption: $caption})
      RETURN p
    """)
}
`;
const seed =
  "CREATE (:Person {key: 'lou', subject: 'auth0|8f2e', name: 'Lou', verified: true}), (:Person {key: 'bo', subject: 'auth0|77aa', name: 'Bo', verified: false})";
const lou = { jwt: { sub: "auth0|8f2e", roles: [] } };
const louByKey = { jwt: { sub: "lou", roles: [] } };
const ghost = { jwt: { sub: "auth0|nobody", roles: [] } };

describe("$viewer in @cypher statements", () => {
  for (const [label, claims, caller] of [
    ["@viewer on an opaque subject", bySubject, lou],
    ["@viewer on the key", byKey, louByKey],
  ] as const) {
    test(`is the caller's key: ${label}`, async () => {
      const t = await createTestLoraGraphQL({
        typeDefs: claims + types,
        seed,
      });
      expect(await t.data("{ myKey me { name } }", {}, caller)).toEqual({
        myKey: "lou",
        me: { name: "Lou" },
      });
      const post = await t.data<{ createPost: unknown }>(
        'mutation { createPost(key: "p1", caption: "hi") { key author { key } } }',
        {},
        caller,
      );
      expect(post.createPost).toEqual({ key: "p1", author: { key: "lou" } });
    });
  }

  test("is null signed out, and for a token naming no node", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: bySubject + types,
      seed,
    });
    expect(await t.data("{ myKey me { name } }")).toEqual({
      myKey: null,
      me: null,
    });
    expect(await t.data("{ myKey me { name } }", {}, ghost)).toEqual({
      myKey: null,
      me: null,
    });
  });

  test("a cached compile serves each caller their own", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: bySubject + types,
      seed,
    });
    const document = parse("{ myKey me { name } }");
    const as = async (contextValue: Record<string, unknown>) =>
      (await execute({ schema: t.schema, document, contextValue })).data;
    const bo = { jwt: { sub: "auth0|77aa", roles: [] } };
    expect(await as(lou)).toEqual({ myKey: "lou", me: { name: "Lou" } });
    expect(await as(bo)).toEqual({ myKey: "bo", me: { name: "Bo" } });
    expect(await as({})).toEqual({ myKey: null, me: null });
    expect(await as(lou)).toEqual({ myKey: "lou", me: { name: "Lou" } });
  });

  test("check() plans statements that use it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: bySubject + types });
    expect((await t.lora.check()).cypher).toEqual([]);
  });

  test("an object field sees it too", async () => {
    const sdl =
      bySubject +
      types.replace(
        "verified: Boolean\n}",
        'verified: Boolean\n  isMe: Boolean! @cypher(statement: "RETURN this.key = $viewer AS me")\n}',
      );
    const t = await createTestLoraGraphQL({ typeDefs: sdl, seed });
    const people = await t.data<{ persons: unknown }>(
      "{ persons(sort: [{ key: ASC }]) { key isMe } }",
      {},
      lou,
    );
    expect(people.persons).toEqual([
      { key: "bo", isMe: false },
      { key: "lou", isMe: true },
    ]);
  });

  test("needs a @viewer claim, and no argument may be called viewer", () => {
    expect(
      problems(
        `type Person @node { key: String! @key }
type Query { me: Person @cypher(statement: "MATCH (p:Person) WHERE p.key = $viewer RETURN p") }`,
      ),
    ).toContain(
      "the statement uses $viewer, which needs a @viewer claim on the @jwt type",
    );
    expect(
      problems(
        bySubject +
          `type Person @node { key: String! @key  subject: String! @unique }
type Query { who(viewer: String!): Person @cypher(statement: "MATCH (p:Person) WHERE p.key = $viewer RETURN p") }`,
      ),
    ).toContain("argument `viewer` is reserved for the caller's @viewer key");
  });
});

describe("@authorization on root @cypher fields", () => {
  const guarded = (rule: string) =>
    bySubject +
    types.replace(
      "@authentication\n",
      `@authentication\n    @authorization(validate: [{ where: ${rule} }])\n`,
    );
  const create = (key: string) =>
    `mutation { createPost(key: "${key}", caption: "hi") { key } }`;

  test("a viewer rule runs before the statement", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: guarded("{ viewer: { verified: { eq: true } } }"),
      seed,
    });
    expect(codes(await t.run(create("a"), {}, lou))).toBeUndefined();
    const bo = { jwt: { sub: "auth0|77aa", roles: [] } };
    expect(codes(await t.run(create("b"), {}, bo))).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(create("c"), {}, ghost))).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(create("d")))).toEqual(["UNAUTHENTICATED"]);
    const { posts } = await t.data<{ posts: Array<{ key: string }> }>(
      "{ posts { key } }",
    );
    expect(posts.map((p) => p.key)).toEqual(["a"]);
  });

  test("a claims rule is decided without the database", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: guarded('{ jwt: { roles: { includes: "poster" } } }'),
      seed,
    });
    t.statements.length = 0;
    expect(codes(await t.run(create("a"), {}, lou))).toEqual(["FORBIDDEN"]);
    expect(t.statements).toHaveLength(0);
    const poster = { jwt: { ...lou.jwt, roles: ["poster"] } };
    expect(codes(await t.run(create("a"), {}, poster))).toBeUndefined();
  });

  test("on a Query field", async () => {
    const sdl =
      bySubject +
      types.replace(
        "myKey: String @cypher",
        "myKey: String @authorization(validate: [{ where: { viewer: { verified: { eq: true } } } }]) @cypher",
      );
    const t = await createTestLoraGraphQL({ typeDefs: sdl, seed });
    expect(await t.data("{ myKey }", {}, lou)).toEqual({ myKey: "lou" });
    const bo = { jwt: { sub: "auth0|77aa", roles: [] } };
    expect(codes(await t.run("{ myKey }", {}, bo))).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run("{ myKey }"))).toEqual(["UNAUTHENTICATED"]);
  });

  test("rules test jwt and viewer only", () => {
    expect(
      problems(guarded('{ node: { key: { eq: "x" } } }')).some((m) =>
        m.includes("a root @cypher field has no node"),
      ),
    ).toBe(true);
    expect(
      problems(
        bySubject +
          types.replace(
            "@authentication\n",
            '@authentication\n    @authorization(filter: [{ where: { jwt: { sub: { eq: "x" } } } }])\n',
          ),
      ),
    ).toContain(
      "a root @cypher field takes validate rules: there are no rows to filter",
    );
  });
});

describe("list arguments of @cypher fields", () => {
  const sdl = (arg: string) =>
    byKey +
    `type Person @node { key: String! @key  subject: String! @unique }
type Tag @node { key: String! @key }
type Mutation {
  tag(${arg}): Int @cypher(statement: "UNWIND $tags AS t MERGE (:Tag {key: t}) RETURN count(*) AS n")
}`;
  const tags = (n: number) => Array.from({ length: n }, (_, i) => `t${i}`);
  const tag = "mutation ($tags: [String!]!) { tag(tags: $tags) }";

  test("@size(max:) caps one", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: sdl("tags: [String!]! @size(max: 3)"),
    });
    expect(await t.data(tag, { tags: tags(3) })).toEqual({ tag: 3 });
    t.statements.length = 0;
    const r = await t.run(tag, { tags: tags(4) });
    expect(codes(r)).toEqual(["BAD_USER_INPUT"]);
    expect(r.errors?.[0]?.message).toContain("tags takes at most 3 items");
    expect(t.statements).toHaveLength(0);
  });

  test("maxListArgument caps every other one", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: sdl("tags: [String!]!"),
      maxListArgument: 5,
    });
    expect(await t.data(tag, { tags: tags(5) })).toEqual({ tag: 5 });
    expect(codes(await t.run(tag, { tags: tags(6) }))).toEqual([
      "BAD_USER_INPUT",
    ]);
  });

  test("the default cap is 1000", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: sdl("tags: [String!]!"),
    });
    expect(codes(await t.run(tag, { tags: tags(1001) }))).toEqual([
      "BAD_USER_INPUT",
    ]);
  });

  test("@size takes list arguments of @cypher fields", () => {
    expect(problems(sdl("tags: [String!]!, n: Int @size(max: 2)"))).toContain(
      "argument n: @size applies to list arguments",
    );
    expect(problems(sdl("tags: [String!]! @size(max: 0)"))).toContain(
      "argument tags: @size(max:) must be at least 1",
    );
    expect(
      problems(
        `type Person @node {
  key: String! @key
  label(tags: [String!] @size(max: 2)): String @customResolver
}`,
      ),
    ).toContain("argument tags: @size applies to arguments of @cypher fields");
  });
});

describe("@range on scalar arguments of @cypher fields", () => {
  const sdl = (arg: string) =>
    byKey +
    `type Person @node { key: String! @key  subject: String! @unique }
type Query {
  echo(${arg}): Float @cypher(statement: "RETURN $n AS n")
}`;
  const echo = "query ($n: Int) { echo(n: $n) }";

  test("a value outside the bounds is BAD_USER_INPUT before any statement", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: sdl("n: Int @range(min: 1, max: 100)"),
    });
    expect(await t.data(echo, { n: 1 })).toEqual({ echo: 1 });
    expect(await t.data(echo, { n: 100 })).toEqual({ echo: 100 });
    expect(await t.data(echo, { n: null })).toEqual({ echo: null });
    t.statements.length = 0;
    const r = await t.run(echo, { n: 101 });
    expect(codes(r)).toEqual(["BAD_USER_INPUT"]);
    expect(r.errors?.[0]?.message).toContain(
      "argument n must be between 1 and 100 (got 101)",
    );
    expect(codes(await t.run(echo, { n: 0 }))).toEqual(["BAD_USER_INPUT"]);
    expect(t.statements).toHaveLength(0);
  });

  test("one bound, a Float and list items", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: sdl("n: Float @range(max: 0.5)"),
    });
    const r = await t.run("{ echo(n: 0.75) }");
    expect(r.errors?.[0]?.message).toContain("must be at most 0.5");
    const list = await createTestLoraGraphQL({
      typeDefs:
        byKey.concat(`type Person @node { key: String! @key  subject: String! @unique }
type Query {
  total(ns: [Int!]! @range(min: 0) @size(max: 3)): Int @cypher(statement: "RETURN reduce(s = 0, x IN $ns | s + x) AS n")
}`),
    });
    expect(await list.data("{ total(ns: [1, 2]) }")).toEqual({ total: 3 });
    const bad = await list.run("{ total(ns: [1, -2]) }");
    expect(bad.errors?.[0]?.message).toContain("must be at least 0 (got -2)");
  });

  test("model checks", () => {
    expect(problems(sdl("n: String @range(min: 1)"))).toContain(
      "argument n: @range applies to Int and Float arguments",
    );
    expect(problems(sdl("n: Int @range"))).toContain(
      "argument n: @range needs min, max or both",
    );
    expect(problems(sdl("n: Int @range(min: 5, max: 1)"))).toContain(
      "argument n: @range(min:) is greater than max",
    );
    expect(problems(sdl("n: Int = 500 @range(max: 100)"))).toContain(
      "argument n: its default is outside @range",
    );
    expect(
      problems(
        `type Person @node {
  key: String! @key
  label(n: Int @range(max: 2)): String @customResolver
}`,
      ),
    ).toContain("argument n: @range applies to arguments of @cypher fields");
  });
});
