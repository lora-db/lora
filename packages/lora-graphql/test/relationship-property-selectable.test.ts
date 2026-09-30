// `@selectable` on a `@relationshipProperties` field hides it as on a node
// field; directives relationship properties cannot apply are refused.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const codes = (r: Result) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

describe("@selectable on a relationship property", () => {
  const J = "type Claims @jwt { sub: String!  roles: [String!] }\n";
  const typeDefs =
    J +
    `type Person @node @mutation { key: String! @key }
    type Seen @relationshipProperties { lastReadAt: String @selectable(onRead: false)  role: String  score: Int @selectable(onAggregate: false) }
    type Room @node @mutation @query(aggregate: true) { key: String! @key
      people: [Person!]! @relationship(type: "IN", direction: IN, properties: "Seen") }`;
  const seed = [
    "CREATE (:Person {key:'u'})-[:IN {lastReadAt:'2026-09-30T10:00:00Z', role:'member', score: 3}]->(:Room {key:'r'})",
  ];

  test("onRead: false leaves the property out of the edge type", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const seen = t.schema.getType("Seen") as unknown as {
      getFields(): Record<string, unknown>;
    };
    expect(Object.keys(seen.getFields()).sort()).toEqual(["role", "score"]);
    const r = await t.run(
      "{ rooms { peopleConnection { edges { properties { lastReadAt } } } } }",
    );
    expect(r.errors?.[0]?.message).toMatch(/lastReadAt/);
    const ok = await t.data<{
      rooms: Array<{
        peopleConnection: { edges: Array<{ properties: { role: string } }> };
      }>;
    }>("{ rooms { peopleConnection { edges { properties { role } } } } }");
    expect(ok.rooms[0]!.peopleConnection.edges[0]!.properties.role).toBe(
      "member",
    );
  });

  test("the hidden property can still be written", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      `mutation { createRooms(input: [{ key: "r2", people: { connect: [{ key: "u", edge: { lastReadAt: "2026-10-01T00:00:00Z" } }] } }]) { rooms { key } } }`,
    );
    expect(codes(r)).toBeUndefined();
    const rows = (await t.db.execute(
      "MATCH (:Person)-[s:IN]->(:Room {key: 'r2'}) RETURN s.lastReadAt AS at",
    )) as { rows: Array<Record<string, unknown>> };
    expect(rows.rows[0]!["at"]).toBe("2026-10-01T00:00:00Z");
  });

  test("onAggregate: false leaves it out of edge aggregates", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const edge = t.schema.getType("SeenAggregateEdge") as unknown as
      | { getFields(): Record<string, unknown> }
      | undefined;
    expect(Object.keys(edge?.getFields() ?? {})).not.toContain("score");
    expect(Object.keys(edge?.getFields() ?? {})).not.toContain("lastReadAt");
  });

  test("directives relationship properties cannot apply are refused", async () => {
    await expect(
      createTestLoraGraphQL({
        typeDefs: `type Person @node { key: String! @key }
          type Seen @relationshipProperties { at: String @populatedBy(callback: "now", operations: [CREATE]) }
          type Room @node { key: String! @key
            people: [Person!]! @relationship(type: "IN", direction: IN, properties: "Seen") }`,
      }),
    ).rejects.toThrow(
      /@populatedBy is not supported on a relationship property/,
    );
  });
});
