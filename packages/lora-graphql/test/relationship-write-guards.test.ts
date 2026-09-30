// Directives on a relationship field or its properties hold on every
// write path, not only on reads.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const J = "type Claims @jwt { sub: String!  roles: [String!] }\n";
const user = { jwt: { sub: "u", roles: [] } };

describe("@authentication on a relationship field covers writing it", () => {
  const typeDefs =
    J +
    `type P @node @mutation { key: String! @key }
    type C @node @mutation { key: String! @key
      open: [P!]! @relationship(type: "OPEN", direction: OUT)
      guarded: [P!]! @relationship(type: "IN", direction: OUT)
        @authentication(operations: [CREATE_RELATIONSHIP, DELETE_RELATIONSHIP]) }`;
  const seed = ["CREATE (:P {key: 'p'}), (:C {key: 'c'})"];

  test("connecting and disconnecting need a token; other fields do not", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const connect = (field: string) =>
      `mutation { updateC(key: "c", update: { ${field}: { connect: [{ key: "p" }] } }) { c { key } } }`;
    expect(codes(await t.run(connect("open")))).toBeUndefined();
    expect(codes(await t.run(connect("guarded")))).toEqual(["UNAUTHENTICATED"]);
    expect(codes(await t.run(connect("guarded"), {}, user))).toBeUndefined();
    const disconnect = `mutation { updateC(key: "c", update: { guarded: { disconnect: ["p"] } }) { c { key } } }`;
    expect(codes(await t.run(disconnect))).toEqual(["UNAUTHENTICATED"]);
    expect(codes(await t.run(disconnect, {}, user))).toBeUndefined();
    // A create that connects through the field is a relationship write too.
    expect(
      codes(
        await t.run(
          `mutation { createCs(input: [{ key: "c2", guarded: { connect: [{ key: "p" }] } }]) { cs { key } } }`,
        ),
      ),
    ).toEqual(["UNAUTHENTICATED"]);
  });
});

describe("@settable(onUpdate: false) on a relationship property", () => {
  const typeDefs = `type P @node { key: String! @key }
    type Seen @relationshipProperties { since: Int @settable(onUpdate: false)  note: String }
    type C @node @mutation { key: String! @key
      people: [P!]! @relationship(type: "IN", direction: OUT, properties: "Seen") }`;
  const seed = ["CREATE (:P {key: 'p'}), (:C {key: 'c'})"];

  test("is set on create and refused on a re-connect", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const connect = (edge: string) =>
      `mutation { updateC(key: "c", update: { people: { connect: [{ key: "p", edge: { ${edge} } }] } }) { c { key } } }`;
    expect(codes(await t.run(connect("since: 2020")))).toBeUndefined();
    expect(codes(await t.run(connect("since: 2024")))).toEqual([
      "BAD_USER_INPUT",
    ]);
    // Other properties still update on a re-connect.
    expect(codes(await t.run(connect(`note: "hi"`)))).toBeUndefined();
    const rows = (await t.db.execute(
      "MATCH (:C)-[r:IN]->(:P) RETURN r.since AS since, r.note AS note",
    )) as { rows: Array<Record<string, unknown>> };
    expect(rows.rows).toEqual([{ since: 2020, note: "hi" }]);
  });
});
