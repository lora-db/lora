// `@authorizationDefaults`: a claims-only bypass and a default write rule
// for @mutation types, plus check() failing on unguarded writes.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";
import { expectSameStatements } from "./equivalence.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const claims = "type Claims @jwt { sub: String!  roles: [String!] }\n";
const defaults = `extend schema @authorizationDefaults(
  bypass: { jwt: { roles: { includes: "admin" } } }
  mutations: { jwt: { roles: { includes: "editor" } } }
)
`;
const types = `type Genre @node @mutation { key: String! @key  name: String }
type Doc @node @mutation
  @authorization(
    filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
    validate: [{ operations: [CREATE, UPDATE, DELETE], where: { node: { owner: { eq: "$jwt.sub" } } } }]
  ) {
  key: String! @key
  owner: String!
}
type Vault @node
  @authorization(bypass: false, filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
  key: String! @key
  owner: String!
}`;
const seed = [
  "CREATE (:Doc {key: 'a', owner: 'u'}), (:Doc {key: 'b', owner: 'v'}), (:Vault {key: 'va', owner: 'u'}), (:Vault {key: 'vb', owner: 'v'})",
];
const admin = { jwt: { sub: "root", roles: ["admin"] } };
const editor = { jwt: { sub: "ed", roles: ["editor"] } };
const user = { jwt: { sub: "u", roles: [] } };

describe("bypass", () => {
  test("a request passing it skips the rules", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + defaults + types,
      seed,
    });
    const docs = async (ctx: Record<string, unknown>) =>
      (
        await t.data<{ docs: Array<{ key: string }> }>(
          "{ docs(sort: [{ key: ASC }]) { key } }",
          {},
          ctx,
        )
      ).docs.map((d) => d.key);
    expect(await docs(admin)).toEqual(["a", "b"]);
    expect(await docs(user)).toEqual(["a"]);
    const [compiled] = t.lora.compile(
      "{ docs { key } }",
      {},
      {
        context: admin,
      },
    );
    expect(compiled!.compiled.statements[0]!.text).not.toMatch(/owner/);
  });

  test("a type opts out with bypass: false", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + defaults + types,
      seed,
    });
    const r = await t.data<{ vaults: Array<{ key: string }> }>(
      "{ vaults { key } }",
      {},
      { jwt: { sub: "u", roles: ["admin"] } },
    );
    expect(r.vaults.map((v) => v.key)).toEqual(["va"]);
  });

  test("compiles exactly as the hand-written admin branch", async () => {
    const plain =
      claims +
      types.replaceAll(
        'where: { node: { owner: { eq: "$jwt.sub" } } }',
        'where: { OR: [{ node: { owner: { eq: "$jwt.sub" } } }, { jwt: { roles: { includes: "admin" } } }] }',
      );
    // Vault opts out, so its hand-written form keeps only the owner test.
    const vaultPlain = plain.replace(
      /(type Vault[\s\S]*?)where: \{ OR: \[\{ node: \{ owner: \{ eq: "\$jwt\.sub" \} \} \}, \{ jwt: \{ roles: \{ includes: "admin" \} \} \}\] \}/,
      '$1where: { node: { owner: { eq: "$jwt.sub" } } }',
    );
    await expectSameStatements(
      claims + defaults.replace(/\n {2}mutations:[^\n]*/, "") + types,
      vaultPlain,
      [
        { query: "{ docs { key } }", context: admin },
        { query: "{ docs { key } }", context: user },
        { query: '{ doc(key: "a") { key } }', context: admin },
        { query: "{ vaults { key } }", context: admin },
      ],
    );
  });
});

describe("mutations default", () => {
  test("guards @mutation types without a write rule of their own", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + defaults + types,
      seed,
    });
    const create = (key: string) =>
      `mutation { createGenres(input: [{ key: "${key}" }]) { genres { key } } }`;
    expect(codes(await t.run(create("g1"), {}, editor))).toBeUndefined();
    expect(codes(await t.run(create("g2"), {}, user))).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(create("g3")))).toEqual(["UNAUTHENTICATED"]);
    // Admins pass through the bypass.
    expect(codes(await t.run(create("g4"), {}, admin))).toBeUndefined();
  });

  test("a type's own write rules replace it", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + defaults + types,
      seed,
    });
    // The editor role means nothing to Doc: its own rule decides.
    const r = await t.run(
      `mutation { createDocs(input: [{ key: "c", owner: "ed" }]) { docs { key } } }`,
      {},
      editor,
    );
    expect(codes(r)).toBeUndefined();
    const r2 = await t.run(
      `mutation { createDocs(input: [{ key: "d", owner: "someone" }]) { docs { key } } }`,
      {},
      editor,
    );
    expect(codes(r2)).toEqual(["FORBIDDEN"]);
  });
});

describe("check() fails on unguarded writes", () => {
  test("a @mutation type with no rule is an error unless declared public", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + types });
    const report = await t.lora.check();
    expect(report.ok).toBe(false);
    expect(report.security.map((s) => s.type)).toEqual(["Genre"]);
    expect(report.security[0]!.message).toMatch(
      /CREATE, UPDATE, DELETE have no @authentication or @authorization rule/,
    );

    const open = await createTestLoraGraphQL({
      typeDefs:
        claims +
        types.replace(
          "type Genre @node @mutation {",
          "type Genre @node @mutation @authorization(public: [CREATE, UPDATE, DELETE]) {",
        ),
    });
    expect((await open.lora.check()).security).toEqual([]);

    const guarded = await createTestLoraGraphQL({
      typeDefs: claims + defaults + types,
    });
    expect((await guarded.lora.check()).security).toEqual([]);
  });
});

describe("model checks", () => {
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test("the bypass tests claims only", () => {
    expect(
      problems(
        claims +
          `extend schema @authorizationDefaults(bypass: { node: { key: { eq: "x" } } })\n` +
          types,
      ),
    ).toContain(
      "@authorizationDefaults(bypass:) tests claims only (jwt, AND, OR, NOT), so it stays a compile-time decision; node is not allowed",
    );
    expect(
      problems(
        claims +
          `extend schema @authorizationDefaults(bypass: { jwt: { nope: { eq: "x" } } })\n` +
          types,
      ),
    ).toContain("@authorization: jwt.nope: not a claim of the @jwt type");
  });

  test("bypass and public belong on the type", () => {
    expect(
      problems(
        claims +
          types.replace(
            "name: String }",
            "name: String @authorization(bypass: false) }",
          ),
      ),
    ).toContain("@authorization(bypass:, public:) belong on the type");
  });
});

test("a write the claims refuse runs no statement (G-37)", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs:
      claims +
      `extend schema @authorizationDefaults(mutations: { jwt: { roles: { includes: "admin" } } })
type G @node @mutation { key: String! @key }`,
  });
  t.statements.length = 0;
  const r = await t.run(
    'mutation { createGs(input: [{ key: "x" }]) { gs { key } } }',
    {},
    { jwt: { sub: "u", roles: [] } },
  );
  expect(codes(r)).toEqual(["FORBIDDEN"]);
  expect(t.statements).toHaveLength(0);
  const anonymous = await t.run(
    'mutation { deleteG(key: "x") { nodesDeleted } }',
  );
  expect(codes(anonymous)).toEqual(["UNAUTHENTICATED"]);
  expect(t.statements).toHaveLength(0);
});

describe("requireAuthentication", () => {
  const post = (rules: string) => `type Post @node @mutation
  @authorization(${rules}) {
  key: String! @key
  published: Boolean!
  tenant: String
  owner: String!
}`;
  const rules = `
    filter: [{ operations: [READ], where: { OR: [
      { node: { published: { eq: true } } }
      { node: { owner: { eq: "$jwt.sub" } } }
      { AND: [{ jwt: { sub: { exists: true } } }, { node: { tenant: { eq: "acme" } } }] }
    ] } }]
    validate: [{ operations: [CREATE, UPDATE, DELETE], where: { node: { owner: { eq: "$jwt.sub" } } } }]`;
  const posts = [
    "CREATE (:Post {key: 'pub', published: true, owner: 'v'}), (:Post {key: 'mine', published: false, owner: 'u'}), (:Post {key: 'acme', published: false, owner: 'v', tenant: 'acme'}), (:Post {key: 'other', published: false, owner: 'v'})",
  ];
  const open =
    "extend schema @authorizationDefaults(requireAuthentication: false)\n";
  const keys = async (
    t: Awaited<ReturnType<typeof createTestLoraGraphQL>>,
    context?: Record<string, unknown>,
  ) =>
    (
      await t.data<{ posts: Array<{ key: string }> }>(
        "{ posts(sort: [{ key: ASC }]) { key } }",
        {},
        context,
      )
    ).posts.map((p) => p.key);
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test("by default, without a token every rule denies", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + post(rules),
      seed: posts,
    });
    expect(await keys(t)).toEqual([]);
    expect(await keys(t, user)).toEqual(["acme", "mine", "pub"]);
  });

  test("false: a branch that reads no claims decides for anonymous callers", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + open + post(rules),
      seed: posts,
    });
    // The tenant branch tests a claim beside the node, so it stays closed.
    expect(await keys(t)).toEqual(["pub"]);
    expect(await keys(t, user)).toEqual(["acme", "mine", "pub"]);
  });

  test("false: a rule that needs claims still asks for a token", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: claims + open + post(rules),
      seed: posts,
    });
    t.statements.length = 0;
    const anonymous = await t.run(
      'mutation { deletePost(key: "pub") { nodesDeleted } }',
    );
    expect(codes(anonymous)).toEqual(["UNAUTHENTICATED"]);
    expect(t.statements).toHaveLength(0);
    expect(
      codes(
        await t.run(
          'mutation { deletePost(key: "pub") { nodesDeleted } }',
          {},
          user,
        ),
      ),
    ).toEqual(["FORBIDDEN"]);
  });

  test("false never lets NOT turn a missing claim into a grant", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        open +
        post(
          'filter: [{ where: { NOT: { node: { owner: { eq: "$jwt.sub" } } } } }]',
        ),
      seed: posts,
    });
    expect(await keys(t)).toEqual([]);
    expect(await keys(t, user)).toEqual(["acme", "other", "pub"]);
  });

  test("it is the schema's setting, not a rule's", () => {
    expect(
      problems(
        claims +
          post(
            "filter: [{ requireAuthentication: false, where: { node: { published: { eq: true } } } }]",
          ),
      ),
    ).toEqual([
      "@authorization: filter[0] has no field requireAuthentication (expected operations, where)",
    ]);
  });

  test("a field a directive's input does not define is a model error", () => {
    expect(
      problems(
        claims +
          post(
            "validate: [{ operation: [CREATE], where: { node: { published: { eq: true } } } }]",
          ),
      ),
    ).toEqual([
      "@authorization: validate[0] has no field operation (expected operations, when, where)",
    ]);
  });
});
