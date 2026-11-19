// `@customResolver` on Query and Mutation fields, arguments on every
// `@customResolver` field, and input object types as arguments of
// `@customResolver` and `@cypher` fields.

import type { ExecutionResult } from "graphql";
import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node { key: String! @key  name: String! }`;

const codes = (r: ExecutionResult) =>
  r.errors?.map((e) => e.extensions?.["code"]);

function problems(typeDefs: string): string[] {
  try {
    buildModel(typeDefs);
  } catch (err) {
    if (err instanceof ModelError) {
      return err.problems.map(
        (p) => `${p.type}${p.field ? `.${p.field}` : ""}: ${p.message}`,
      );
    }
    throw err;
  }
  return [];
}

describe("@customResolver on Query and Mutation", () => {
  const typeDefs = `${claims}
enum Mood { CALM LOUD }
input TasteInput {
  genres: [String!]!
  mood: Mood = CALM
  weight: Float
}
type TastePreview { label: String!  score: Float! }
type Query {
  tastePreview(taste: TasteInput!, limit: Int = 3 @range(min: 1, max: 10)): [TastePreview!]!
    @customResolver
    @authentication
  greeting(name: String! @normalize(trim: true)): String! @customResolver
}
type Mutation {
  saveTaste(taste: TasteInput!): Boolean!
    @customResolver
    @authorization(validate: [{ where: { jwt: { roles: { includes: "admin" } } } }])
}`;
  const calls: unknown[] = [];
  const resolvers = {
    Query: {
      tastePreview: (_: unknown, args: Record<string, unknown>) => {
        calls.push(args);
        const taste = args["taste"] as { genres: string[]; mood: string };
        return taste.genres
          .slice(0, args["limit"] as number)
          .map((g, i) => ({ label: `${g}/${taste.mood}`, score: 1 - i / 10 }));
      },
      greeting: (_: unknown, args: Record<string, unknown>) =>
        `hi ${String(args["name"])}`,
    },
    Mutation: { saveTaste: () => true },
  };

  test("passes the arguments, input objects with their defaults included", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers });
    calls.length = 0;
    const d = await t.data(
      'query($t: TasteInput!) { tastePreview(taste: $t, limit: 2) { label score } greeting(name: "  ada ") }',
      { t: { genres: ["jazz", "dub", "folk"] } },
      { jwt: { sub: "ada" } },
    );
    expect(d).toEqual({
      tastePreview: [
        { label: "jazz/CALM", score: 1 },
        { label: "dub/CALM", score: 0.9 },
      ],
      // @normalize ran before the resolver.
      greeting: "hi ada",
    });
    expect(calls).toEqual([
      { taste: { genres: ["jazz", "dub", "folk"], mood: "CALM" }, limit: 2 },
    ]);
  });

  test("checks argument rules before the resolver runs", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers });
    calls.length = 0;
    const r = await t.run(
      '{ tastePreview(taste: { genres: ["a"] }, limit: 50) { label } }',
      {},
      { jwt: { sub: "ada" } },
    );
    expect(codes(r)).toEqual(["BAD_USER_INPUT"]);
    expect(calls).toEqual([]);
  });

  test("applies @authentication and @authorization as on a root @cypher field", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers });
    const preview = '{ tastePreview(taste: { genres: ["a"] }) { label } }';
    expect(codes(await t.run(preview))).toEqual(["UNAUTHENTICATED"]);
    const save = 'mutation { saveTaste(taste: { genres: ["a"] }) }';
    const user = await t.run(save, {}, { jwt: { sub: "ada", roles: [] } });
    expect(codes(user)).toEqual(["FORBIDDEN"]);
    expect(user.errors![0]!.message).toBe(
      "not allowed to run Mutation.saveTaste",
    );
    expect(
      await t.data(save, {}, { jwt: { sub: "ada", roles: ["admin"] } }),
    ).toEqual({ saveTaste: true });
  });

  test("a custom mutation reports a broad change", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers });
    const seen: Array<{ field: string; broad: boolean }> = [];
    t.lora.onWrite((c) => seen.push({ field: c.field, broad: c.broad }));
    await t.data(
      'mutation { saveTaste(taste: { genres: ["a"] }) }',
      {},
      { jwt: { sub: "ada", roles: ["admin"] } },
    );
    expect(seen).toEqual([{ field: "saveTaste", broad: true }]);
  });

  test("lists the fields in the access matrix", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers });
    const rows = t.lora
      .accessMatrix()
      .filter((e) => e.field === "saveTaste" || e.field === "tastePreview");
    expect(new Set(rows.map((r) => `${r.type}.${r.field}`))).toEqual(
      new Set(["Query.tastePreview", "Mutation.saveTaste"]),
    );
    const anon = rows.find(
      (r) => r.field === "tastePreview" && r.principal === "anonymous",
    );
    expect(anon?.verdict).toBe("unauthenticated");
  });

  test("needs its resolver", async () => {
    await expect(
      createTestLoraGraphQL({ typeDefs, resolvers: { Query: {} } }),
    ).rejects.toThrow(
      "Query.tastePreview: @customResolver needs resolvers.Query.tastePreview",
    );
  });

  test("refuses what a root field cannot have", () => {
    expect(
      problems(`${claims}
type Query {
  a: Person @customResolver
  b: String @customResolver(requires: "name")
  c: String @customResolver @cypher(statement: "RETURN 1 AS c")
}`),
    ).toEqual([
      "Query.a: a root @customResolver field returns scalars, enums or object types without @node",
      "Query.b: requires: a root field has no node to read it from",
      "Query.c: @cypher cannot be combined with @customResolver",
    ]);
  });
});

describe("arguments of a @customResolver field on a node type", () => {
  test("reach the resolver", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `${claims}
type Event @node {
  key: String! @key
  title: String!
  shout(times: Int! @range(min: 1, max: 3), suffix: String = "!"): String!
    @customResolver(requires: "title")
}`,
      seed: "CREATE (:Event {key: 'e1', title: 'Dub'})",
      resolvers: {
        Event: {
          shout: (
            src: Record<string, unknown>,
            args: Record<string, unknown>,
          ) =>
            `${String(src["title"]).repeat(args["times"] as number)}${String(args["suffix"])}`,
        },
      },
    });
    expect(await t.data('{ event(key: "e1") { shout(times: 2) } }')).toEqual({
      event: { shout: "DubDub!" },
    });
    const r = await t.run('{ event(key: "e1") { shout(times: 9) } }');
    expect(codes(r)).toEqual(["BAD_USER_INPUT"]);
  });
});

describe("input object arguments of a @cypher field", () => {
  const typeDefs = `${claims}
input Window { from: Int!  to: Int! }
input Search { prefix: String!  window: Window }
type Query {
  people(search: Search!): [Person!]!
    @authentication
    @cypher(statement: """
      MATCH (p:Person)
      WHERE p.name STARTS WITH $search.prefix
      RETURN p ORDER BY p.key
      SKIP coalesce($search.window.from, 0)
      LIMIT coalesce($search.window.to, 100) - coalesce($search.window.from, 0)
    """, columnName: "p")
}`;

  test("reach the statement as a map, nested input objects included", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed: "CREATE (:Person {key: 'a', name: 'Ann'}), (:Person {key: 'b', name: 'Abe'}), (:Person {key: 'c', name: 'Cy'})",
    });
    const jwt = { jwt: { sub: "a" } };
    expect(
      await t.data('{ people(search: { prefix: "A" }) { key } }', {}, jwt),
    ).toEqual({ people: [{ key: "a" }, { key: "b" }] });
    expect(
      await t.data(
        "query($s: Search!) { people(search: $s) { key } }",
        { s: { prefix: "A", window: { from: 1, to: 2 } } },
        jwt,
      ),
    ).toEqual({ people: [{ key: "b" }] });
  });

  test("refuses input types nothing takes and output types in them", () => {
    expect(
      problems(`${claims}
input Unused { a: String }
input Bad { p: Person }
type Query {
  x(bad: Bad): Int @cypher(statement: "RETURN 1 AS x")
}`),
    ).toEqual(
      expect.arrayContaining([
        "Unused: input object types shape arguments; no @cypher or @customResolver argument takes this one",
        "Bad.p: fields of an input object type are scalars, enums or input object types",
      ]),
    );
  });
});
