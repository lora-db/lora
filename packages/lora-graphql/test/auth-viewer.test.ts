// `@viewer` maps a claim to the caller's node, so rules say `isViewer` and
// `viewer: { … }` instead of hard-coding the token subject into the key.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL, expectSeeks } from "../src/testing.js";
import { expectSameStatements } from "./equivalence.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "subject")
  roles: [String!]
}
`;
// The key is a URL slug; the subject is the identity provider's opaque id.
const people = `type Person @node @mutation
  @authorization(validate: [{ operations: [UPDATE, DELETE], where: { node: { isViewer: true } } }]) {
  key: String! @key
  subject: String! @unique @settable(onUpdate: false)
  name: String
  verified: Boolean
}
type Post @node @mutation
  @authorization(
    filter: [{ requireAuthentication: false, where: { OR: [{ node: { published: { eq: true } } }, { node: { author: { isViewer: true } } }] } }]
    validate: [{ operations: [CREATE], when: [AFTER], where: { AND: [{ node: { author: { isViewer: true } } }, { viewer: { verified: { eq: true } } }] } }]
  ) {
  key: String! @key
  published: Boolean!
  author: Person! @relationship(type: "WROTE", direction: IN)
}`;
const seed = [
  "CREATE (:Person {key: 'lou', subject: 'auth0|8f2e', name: 'Lou', verified: true}), (:Person {key: 'bo', subject: 'auth0|77aa', name: 'Bo', verified: false})",
  "MATCH (l:Person {key: 'lou'}), (b:Person {key: 'bo'}) CREATE (l)-[:WROTE]->(:Post {key: 'draft', published: false}), (b)-[:WROTE]->(:Post {key: 'pub', published: true})",
];
const lou = { jwt: { sub: "auth0|8f2e", roles: [] } };
const bo = { jwt: { sub: "auth0|77aa", roles: [] } };

describe("isViewer", () => {
  test("a person updates their own profile, keyed by a slug", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + people, seed });
    const rename = (key: string) =>
      `mutation { updatePerson(key: "${key}", update: { name: "X" }) { person { key } } }`;
    expect(codes(await t.run(rename("lou"), {}, lou))).toBeUndefined();
    expect(codes(await t.run(rename("bo"), {}, lou))).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(rename("lou")))).toEqual(["UNAUTHENTICATED"]);
  });

  test("through a relationship, in a filter rule", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + people, seed });
    const keys = async (context?: Record<string, unknown>) =>
      (
        await t.data<{ posts: Array<{ key: string }> }>(
          "{ posts(sort: [{ key: ASC }]) { key } }",
          {},
          context,
        )
      ).posts.map((p) => p.key);
    expect(await keys(lou)).toEqual(["draft", "pub"]);
    expect(await keys(bo)).toEqual(["pub"]);
    expect(await keys()).toEqual(["pub"]);
  });

  test("compiles to exactly the hand-written $jwt form", async () => {
    const plain = people.replaceAll(
      "isViewer: true",
      'subject: { eq: "$jwt.sub" }',
    );
    await expectSameStatements(claims + people, claims + plain, [
      { query: "{ posts { key } }", context: lou },
      { query: "{ posts { key } }" },
      { query: '{ post(key: "draft") { key author { key } } }', context: lou },
      { query: "{ persons { key } }", context: lou },
    ]);
  });

  test("keeps key lookups on the index", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + people, seed });
    await expectSeeks(
      t.lora,
      '{ post(key: "draft") { key } }',
      {},
      {
        context: lou,
      },
    );
  });

  test("signed out, NOT isViewer grants nothing", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        people.replace(
          "{ node: { author: { isViewer: true } } }",
          "{ node: { NOT: { author: { isViewer: true } } } }",
        ),
      seed,
    });
    // Anonymous: the NOT branch is unknown, so only published posts show.
    const r = await t.data<{ posts: Array<{ key: string }> }>(
      "{ posts { key } }",
    );
    expect(r.posts.map((p) => p.key)).toEqual(["pub"]);
  });
});

describe("viewer: { … }", () => {
  test("tests the caller's own node", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + people, seed });
    const create = (key: string, author: string) =>
      `mutation { createPosts(input: [{ key: "${key}", published: false, author: { connect: { key: "${author}" } } }]) { posts { key } } }`;
    // Lou is verified; Bo is not.
    expect(codes(await t.run(create("p1", "lou"), {}, lou))).toBeUndefined();
    expect(codes(await t.run(create("p2", "bo"), {}, bo))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(create("p3", "bo"), {}, lou))).toEqual([
      "FORBIDDEN",
    ]);
  });

  test("seeks the caller by the @unique field", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        people.replace(
          "{ node: { author: { isViewer: true } } }",
          "{ viewer: { verified: { eq: true } } }",
        ),
      seed,
    });
    const [compiled] = t.lora.compile(
      "{ posts { key } }",
      {},
      {
        context: lou,
      },
    );
    expect(compiled!.compiled.statements[0]!.text).toMatch(
      /\[\(viewer\w*:Person \{subject: \$p\d+\}\) WHERE/,
    );
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

  test("isViewer needs @viewer, the viewer type and `true`", () => {
    const noViewer = people.replace(
      "type Person",
      "type Claims @jwt { sub: String! }\ntype Person",
    );
    expect(problems(noViewer)).toContain(
      "@authorization: isViewer needs a @viewer claim on the @jwt type",
    );
    expect(
      problems(
        claims +
          people.replace(
            "{ node: { author: { isViewer: true } } }",
            "{ node: { isViewer: true } }",
          ),
      ),
    ).toContain(
      "@authorization: isViewer applies to Person (the @viewer type), not Post",
    );
    expect(
      problems(
        claims +
          people.replace(
            "{ node: { isViewer: true } }",
            "{ node: { isViewer: false } }",
          ),
      ),
    ).toContain(
      "@authorization: isViewer takes `true`; wrap it in NOT for the opposite",
    );
  });

  test("the viewer field names one node", () => {
    expect(
      problems(
        claims + people.replace("subject: String! @unique", "subject: String!"),
      ),
    ).toContain(
      "@viewer: Person.subject must be @key or @unique, so it names one node",
    );
    expect(
      problems(
        claims.replace(
          "roles: [String!]",
          'roles: [String!]\n  other: String @viewer(type: "Person", field: "key")',
        ) + people,
      ),
    ).toContain("only one @viewer claim is allowed (sub has one)");
  });
});
