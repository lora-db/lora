// `<field>Exists: Boolean` on a single relationship: whether it is set.
// `trip: null` can't say "no trip" (absent and null filters are left out),
// and an empty filter is a model error in a rule.

import { describe, expect, test } from "vitest";
import { buildModel } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

const typeDefs = `type Claims @jwt { sub: String!  roles: [String!] }
type Trip @node
  @authorization(filter: [{ requireAuthentication: false, where: { node: { public: { eq: true } } } }]) {
  key: String! @key
  public: Boolean!
}
type Conversation @node @mutation
  @authorization(validate: [{
    operations: [CREATE]
    # A 1:1 has no trip; a crew room has one.
    where: { OR: [
      { AND: [{ node: { kind: { eq: "DIRECT" } } }, { node: { tripExists: false } }] }
      { AND: [{ node: { kind: { eq: "CREW" } } }, { node: { tripExists: true } }] }
    ] }
  }]) {
  key: String! @key
  kind: String! @filterable
  trip: Trip @relationship(type: "CHAT_OF", direction: OUT) @filterable
}`;
const seed = [
  "CREATE (:Trip {key: 'open', public: true}), (:Trip {key: 'hidden', public: false})",
  "MATCH (o:Trip {key: 'open'}), (h:Trip {key: 'hidden'}) CREATE (:Conversation {key: 'c-open', kind: 'CREW'})-[:CHAT_OF]->(o), (:Conversation {key: 'c-hidden', kind: 'CREW'})-[:CHAT_OF]->(h), (:Conversation {key: 'dm', kind: 'DIRECT'})",
];
const someone = { jwt: { sub: "someone", roles: [] } };

describe("<field>Exists", () => {
  test("in a rule: a 1:1 can't carry a trip, a crew room must", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const create = (key: string, kind: string, trip?: string) =>
      `mutation { createConversations(input: [{ key: "${key}", kind: "${kind}"${trip ? `, trip: { connect: { key: "${trip}" } }` : ""} }]) { conversations { key } } }`;
    expect(
      codes(await t.run(create("a", "DIRECT", "open"), {}, someone)),
    ).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(create("b", "DIRECT"), {}, someone))).toBe(
      undefined,
    );
    expect(codes(await t.run(create("c", "CREW"), {}, someone))).toEqual([
      "FORBIDDEN",
    ]);
    expect(
      codes(await t.run(create("d", "CREW", "open"), {}, someone)),
    ).toBeUndefined();
  });

  test("in a public filter; a related node the reader can't see counts as none", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const keys = async (exists: boolean | null) =>
      (
        await t.data<{ conversations: Array<{ key: string }> }>(
          "query ($e: Boolean) { conversations(where: { tripExists: $e }, sort: [{ key: ASC }]) { key } }",
          { e: exists },
        )
      ).conversations.map((c) => c.key);
    expect(await keys(true)).toEqual(["c-open"]);
    expect(await keys(false)).toEqual(["c-hidden", "dm"]);
    // Absent or null, like any filter: left out.
    expect(await keys(null)).toEqual(["c-hidden", "c-open", "dm"]);
  });

  test("model checks", () => {
    const build =
      (rule: string, extra = "") =>
      () =>
        buildModel(
          `type Trip @node { key: String! @key }
        type C @node @mutation @authorization(validate: [{ operations: [CREATE], where: ${rule} }]) {
          key: String! @key
          trip: Trip @relationship(type: "OF", direction: OUT)
          trips: [Trip!]! @relationship(type: "ALL", direction: OUT)
          ${extra}
        }`,
        );
    expect(build(`{ node: { tripExists: false } }`)).not.toThrow();
    expect(build(`{ node: { tripExists: "no" } }`)).toThrow(
      /tripExists must be true or false/,
    );
    // A list relationship asks `count`.
    expect(build(`{ node: { tripsExists: true } }`)).toThrow(
      /C has no field tripsExists/,
    );
    expect(
      build(`{ node: { key: { eq: "x" } } }`, "tripExists: Boolean"),
    ).toThrow(/collides with the tripExists filter/);
  });
});
