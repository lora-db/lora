// `@key(scope: VIEWER)`: created keys live in the caller's key space
// (`lou:x`), checked before any statement runs, so the answer never tells
// whether someone else's key exists.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "subject")
  roles: [String!]
}
`;
const types = `type Person @node { key: String! @key  subject: String! @unique }
type Trip @node @mutation
  @authorization(filter: [{ where: { node: { key: { startsWith: "\${jwt.sub}:" } } } }]) {
  key: String! @key(scope: VIEWER, separator: ":")
  name: String
}`;
const seed = [
  "CREATE (:Person {key: 'lou', subject: 'lou'}), (:Person {key: 'bob', subject: 'bob'}), (:Trip {key: 'bob:x', name: 'Bob'})",
];
const as = (sub: string, roles: string[] = []) => ({ jwt: { sub, roles } });
const create = (key: string) =>
  `mutation { createTrips(input: [{ key: "${key}", name: "T" }]) { trips { key } } }`;

describe("@key(scope: VIEWER)", () => {
  test("a create outside the caller's key space is FORBIDDEN, whether the key exists or not", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + types, seed });
    const taken = await t.run(create("bob:x"), {}, as("lou"));
    const free = await t.run(create("bob:y"), {}, as("lou"));
    expect(codes(taken)).toEqual(["FORBIDDEN"]);
    expect(codes(free)).toEqual(["FORBIDDEN"]);
    expect(taken.errors![0]!.message).toBe(free.errors![0]!.message);
    expect(codes(await t.run(create("lou")))).toEqual(["UNAUTHENTICATED"]);
  });

  test("a create in the caller's key space succeeds", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + types, seed });
    expect(codes(await t.run(create("lou:x"), {}, as("lou")))).toBeUndefined();
    // The prefix alone is not a key.
    expect(codes(await t.run(create("lou:"), {}, as("lou")))).toEqual([
      "FORBIDDEN",
    ]);
  });

  test("a claim containing the separator cannot reach into another key space", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: claims + types, seed });
    expect(codes(await t.run(create("bob:x:1"), {}, as("bob:x")))).toEqual([
      "FORBIDDEN",
    ]);
  });

  test("the schema's bypass skips it", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        claims +
        `extend schema @authorizationDefaults(bypass: { jwt: { roles: { includes: "admin" } } })\n` +
        types,
      seed,
    });
    expect(
      codes(await t.run(create("bob:z"), {}, as("root", ["admin"]))),
    ).toBeUndefined();
  });

  test("needs a @viewer claim, and excludes generate", () => {
    const problems = (sdl: string) => {
      try {
        buildModel(sdl);
        return [];
      } catch (err) {
        if (err instanceof ModelError)
          return err.problems.map((p) => p.message);
        throw err;
      }
    };
    expect(
      problems(
        claims.replace(' @viewer(type: "Person", field: "subject")', "") +
          types,
      ),
    ).toContain(
      "@key(scope: VIEWER) needs a @viewer claim on the @jwt type: it names whose key space a create writes in",
    );
    expect(
      problems(
        claims +
          types.replace(
            '@key(scope: VIEWER, separator: ":")',
            "@key(scope: VIEWER, generate: true)",
          ),
      ),
    ).toContain("@key(scope:) and @key(generate: true) are exclusive");
  });
});

describe("@key(scope: VIEWER) with an opaque subject (G-33)", () => {
  const typeDefs = `type Claims @jwt { sub: String! @viewer(type: "Person", field: "subject") }
    type Person @node { key: String! @key  subject: String! @unique }
    type Trip @node @mutation(operations: [CREATE]) @authorization(public: [CREATE]) {
      key: String! @key(scope: VIEWER, separator: ":")
      owner: Person! @relationship(type: "OWNS", direction: IN) }`;
  const seed = "CREATE (:Person {key: 'lou', subject: 'auth0|abc'})";
  const start = (
    t: Awaited<ReturnType<typeof createTestLoraGraphQL>>,
    key: string,
    sub = "auth0|abc",
  ) =>
    t.run(
      `mutation { createTrips(input: [{ key: "${key}", owner: { connect: { key: "lou" } } }]) { trips { key } } }`,
      {},
      { jwt: { sub } },
    );

  test("prefixes with the caller's key, not the subject", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(codes(await start(t, "lou:x"))).toBeUndefined();
    expect(codes(await start(t, "auth0|abc:x"))).toEqual(["FORBIDDEN"]);
  });

  test("a token naming no person creates nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(codes(await start(t, "lou:y", "auth0|nobody"))).toEqual([
      "FORBIDDEN",
    ]);
  });
});
