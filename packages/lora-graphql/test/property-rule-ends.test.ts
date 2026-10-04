// G-42: rules on a relationship property may test the relationship's
// `source`, `target` and `edge` when every relationship field using the
// properties type declares the same ends, so one edge can carry an `rsvp`
// every member reads and a marker only its member reads.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
`;
const marker = `@authorization(validate: [{ operations: [READ], where: { target: { isViewer: true } } }])`;
const typeDefs =
  claims +
  `type Person @node { key: String! @key }
type E @relationshipProperties {
  rsvp: String
  lastReadAt: String @filterable @sortable ${marker}
}
type Trip @node {
  key: String! @key
  members: [Person!]! @relationship(type: "MEMBER", direction: IN, properties: "E") @filterable
}`;
const seed = [
  "CREATE (lou:Person {key: 'lou'}), (bo:Person {key: 'bo'}), (t:Trip {key: 't'}), (lou)-[:MEMBER {rsvp: 'GOING', lastReadAt: 'L'}]->(t), (bo)-[:MEMBER {rsvp: 'MAYBE', lastReadAt: 'B'}]->(t)",
];
const as = (sub: string, roles: string[] = []) => ({ jwt: { sub, roles } });
const read = `{ trip(key: "t") { membersConnection(sort: [{ key: ASC }]) { edges { node { key } properties { rsvp lastReadAt } } } } }`;

describe("relationship property rules over the ends", () => {
  test("members read everyone's rsvp; only the member reads their lastReadAt", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(read, {}, as("bo"));
    expect(codes(r)).toEqual(["FORBIDDEN"]);
    expect(r.errors![0]!.path).toEqual([
      "trip",
      "membersConnection",
      "edges",
      1,
      "properties",
      "lastReadAt",
    ]);
    expect(r.data).toEqual({
      trip: {
        membersConnection: {
          edges: [
            {
              node: { key: "bo" },
              properties: { rsvp: "MAYBE", lastReadAt: "B" },
            },
            {
              node: { key: "lou" },
              properties: { rsvp: "GOING", lastReadAt: null },
            },
          ],
        },
      },
    });
    // rsvp alone answers for every edge.
    const rsvp = await t.run(
      `{ trip(key: "t") { membersConnection { edges { properties { rsvp } } } } }`,
      {},
      as("bo"),
    );
    expect(rsvp.errors).toBeUndefined();
  });

  test("nothing filters, sorts or aggregates by it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    for (const q of [
      `{ trip(key: "t") { membersConnection(where: { edge: { lastReadAt: { eq: "L" } } }) { edges { node { key } } } } }`,
      `{ trip(key: "t") { membersConnection(sort: [{ edge: { lastReadAt: ASC } }]) { edges { node { key } } } } }`,
      `{ trips(where: { membersConnection: { some: { edge: { lastReadAt: { eq: "L" } } } } }) { key } }`,
    ]) {
      const r = await t.run(q, {}, as("lou"));
      expect(codes(r), q).toEqual(["FORBIDDEN"]);
      expect(r.errors![0]!.message).toMatch(/decide per relationship/);
    }
  });

  test("a claims rule beside it still grants on every edge", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs.replace(
        "where: { target: { isViewer: true } } }",
        'where: { target: { isViewer: true } } }, { operations: [READ], where: { jwt: { roles: { includes: "staff" } } } }',
      ),
      seed,
    });
    const r = await t.run(read, {}, as("cy", ["staff"]));
    expect(r.errors).toBeUndefined();
    expect(JSON.stringify(r.data)).toContain('"lastReadAt":"L"');
    const anonymous = await t.run(read);
    expect(JSON.stringify(anonymous.data)).not.toContain('"L"');
  });

  test("the access matrix says validated", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const m = t.lora
      .accessMatrix()
      .filter((e) => e.type === "E" && e.field === "lastReadAt");
    expect(m.map((e) => `${e.principal}: ${e.verdict}`)).toEqual([
      "anonymous: unauthenticated",
      "authenticated: validated",
    ]);
  });

  test("model checks", () => {
    const problems = (sdl: string) => {
      try {
        buildModel(sdl);
        return [];
      } catch (err) {
        if (!(err instanceof ModelError)) throw err;
        return err.problems.map((p) => p.message);
      }
    };
    // Two fields with different ends: `target` would be ambiguous.
    expect(
      problems(
        typeDefs.replace(
          "type Person @node { key: String! @key }",
          `type Person @node { key: String! @key  trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "E") }`,
        ),
      ).some((m) => m.includes("declare different ends")),
    ).toBe(true);
    // Writes decide on claims.
    expect(
      problems(
        typeDefs.replace("operations: [READ]", "operations: [READ, CREATE]"),
      ),
    ).toContain(
      "@authorization: source, target and edge decide per relationship, so they take READ rules only; CREATE and UPDATE rules on a relationship property test claims",
    );
    // The ends are checked like a relationship rule's.
    expect(
      problems(
        typeDefs.replace(
          "target: { isViewer: true }",
          "target: { nope: { eq: 1 } }",
        ),
      ).some((m) => m.includes("target.nope")),
    ).toBe(true);
  });
});
