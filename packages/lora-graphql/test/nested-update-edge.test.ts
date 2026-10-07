// `nestedOperations: [UPDATE_EDGE]`: a nested `update` that writes the
// relationship's properties and never the connected node, so an input can
// keep "set my RSVP" without advertising "edit the festival".

import { describe, expect, test } from "vitest";
import { getNamedType, type GraphQLInputObjectType } from "graphql";
import { buildModel } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = (ops: string) => `
type Festival @node @mutation {
  key: String! @key
  name: String!
}
type Attendance @relationshipProperties { status: String }
type Person @node @mutation {
  key: String! @key
  festivals: [Festival!]!
    @relationship(type: "ATTENDS", direction: OUT, properties: "Attendance", nestedOperations: [${ops}])
}`;
const seed = [
  "CREATE (p:Person {key: 'lou'})-[:ATTENDS {status: 'MAYBE'}]->(:Festival {key: 'f', name: 'F'})",
];

describe("UPDATE_EDGE", () => {
  test("offers the edge, not the node", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs("CONNECT, DISCONNECT, UPDATE_EDGE"),
      seed,
    });
    const nested = t.lora
      .getSchema()
      .getType("PersonFestivalsUpdateConnectedInput") as GraphQLInputObjectType;
    expect(Object.keys(nested.getFields()).sort()).toEqual(["edge", "key"]);
    const r = await t.data<{
      updatePerson: {
        person: {
          festivalsConnection: {
            edges: Array<{ properties: { status: string } }>;
          };
        };
      };
    }>(
      `mutation { updatePerson(key: "lou", update: { festivals: { update: [{ key: "f", edge: { status: "GOING" } }] } }) {
        person { festivalsConnection { edges { properties { status } } } } } }`,
    );
    expect(
      r.updatePerson.person.festivalsConnection.edges[0]!.properties.status,
    ).toBe("GOING");
    const node = await t.run(
      `mutation { updatePerson(key: "lou", update: { festivals: { update: [{ key: "f", node: { name: "X" } }] } }) { person { key } } }`,
    );
    expect(node.errors?.[0]?.message).toMatch(/node/);
  });

  test("UPDATE still offers both", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs("CONNECT, UPDATE"),
      seed,
    });
    const nested = t.lora
      .getSchema()
      .getType("PersonFestivalsUpdateConnectedInput") as GraphQLInputObjectType;
    expect(Object.keys(nested.getFields()).sort()).toEqual([
      "edge",
      "key",
      "node",
    ]);
    expect(getNamedType(nested.getFields()["node"]!.type).name).toBe(
      "FestivalUpdateInput",
    );
  });

  test("alone, it is enough for the type to keep its update mutation", async () => {
    // Person has nothing else to update: the edge is the whole update.
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs("UPDATE_EDGE"),
      seed,
    });
    expect(t.lora.model.warnings).toEqual([]);
    const d = await t.data<{
      updatePerson: { info: { nodesUpdated: number } };
    }>(
      `mutation { updatePerson(key: "lou", update: { festivals: { update: [{ key: "f", edge: { status: "GOING" } }] } }) { info { nodesUpdated } } }`,
    );
    expect(d.updatePerson.info.nodesUpdated).toBe(1);
    const stored = await t.db.execute(
      "MATCH (:Person {key: 'lou'})-[r:ATTENDS]->(:Festival {key: 'f'}) RETURN r.status AS status",
    );
    expect(stored.rows).toEqual([{ status: "GOING" }]);
  });

  test("needs relationship properties", () => {
    expect(() =>
      buildModel(`type A @node @mutation { key: String! @key  b: [B!]! @relationship(type: "R", direction: OUT, nestedOperations: [UPDATE_EDGE]) }
        type B @node { key: String! @key }`),
    ).toThrow(/UPDATE_EDGE updates relationship properties/);
  });
});
