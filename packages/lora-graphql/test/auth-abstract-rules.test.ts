// Rules reaching a node through a union member or an interface
// implementation: sugar expands there too, the startup checks cover those
// branches, and nothing a rule says is ever compiled away.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const typeDefs = (
  test: string,
) => `type Claims @jwt { sub: String! @viewer(type: "Person", field: "key") }
type Person @node { key: String! @key }
type Artist @node { key: String! @key }
union Author = Person | Artist
type Post @node @mutation(operations: [DELETE])
  @authorization(
    filter: [{ operations: [READ], where: { node: ${test} } }]
    validate: [{ operations: [DELETE], where: { node: ${test} } }]) {
  key: String! @key
  author: Author! @relationship(type: "WROTE", direction: IN)
}`;
const seed =
  "CREATE (:Person {key: 'lou'})-[:WROTE]->(:Post {key: 'p1'}), (:Person {key: 'eve'})";
const eve = { jwt: { sub: "eve" } };
const lou = { jwt: { sub: "lou" } };

describe("isViewer under a union member", () => {
  const sugared = typeDefs("{ author: { Person: { isViewer: true } } }");

  test("tests the viewer, so another person neither reads nor deletes", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: sugared, seed });
    expect(await t.data("{ posts { key } }", {}, eve)).toEqual({ posts: [] });
    expect(
      codes(
        await t.run(
          'mutation { deletePost(key: "p1") { nodesDeleted } }',
          {},
          eve,
        ),
      ),
    ).toEqual(["FORBIDDEN"]);
    expect(await t.data("{ posts { key } }", {}, lou)).toEqual({
      posts: [{ key: "p1" }],
    });
    const [compiled] = t.lora.compile(
      "{ posts { key } }",
      {},
      { context: eve },
    );
    expect(compiled!.compiled.statements[0]!.text).toMatch(
      /this_author\.key = \$p\d+/,
    );
  });

  test("check() does not lint the branch as claim-free", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: sugared, seed });
    const lint = (await t.lora.check()).lint.map((w) => w.message);
    expect(lint.filter((m) => m.includes("needs no claims"))).toEqual([]);
  });

  test("holds under NOT", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs("{ NOT: { author: { Person: { isViewer: true } } } }"),
      seed,
    });
    expect(await t.data("{ posts { key } }", {}, lou)).toEqual({ posts: [] });
    expect(await t.data("{ posts { key } }", {}, eve)).toEqual({
      posts: [{ key: "p1" }],
    });
  });
});

test("isViewer under a union member, nested a relationship deeper", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs:
      typeDefs("{ author: { Person: { isViewer: true } } }") +
      `
type Comment @node
  @authorization(filter: [{ where: { node: { post: { author: { Person: { isViewer: true } } } } } }]) {
  key: String! @key
  post: Post! @relationship(type: "ON", direction: OUT)
}`,
    seed: [
      seed,
      "MATCH (p:Post {key: 'p1'}) CREATE (:Comment {key: 'c1'})-[:ON]->(p)",
    ],
  });
  expect(await t.data("{ comments { key } }", {}, eve)).toEqual({
    comments: [],
  });
  expect(await t.data("{ comments { key } }", {}, lou)).toEqual({
    comments: [{ key: "c1" }],
  });
});

describe("startup checks cover union and interface branches", () => {
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test("an unknown field under a union member is refused", () => {
    expect(
      problems(typeDefs('{ author: { Person: { nope: { eq: "x" } } } }')),
    ).toContain(
      "@authorization: node.author.Person.nope: Person has no field nope",
    );
  });

  test("a non-member is refused", () => {
    expect(
      problems(typeDefs('{ author: { Band: { key: { eq: "x" } } } }')),
    ).toContain(
      "@authorization: node.author.Band: Band is not a member of Author",
    );
  });
});
