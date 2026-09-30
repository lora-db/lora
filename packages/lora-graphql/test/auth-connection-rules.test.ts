// Authorization rules test relationship properties through
// `<field>Connection`, in `filter` and `validate` rules alike.

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
const keysOf = (r: Result, field: string) => {
  expect(r.errors).toBeUndefined();
  return ((r.data as Record<string, Array<{ key: string }>>)[field] ?? []).map(
    (x) => x.key,
  );
};

describe("authorization rules test relationship properties via <field>Connection", () => {
  const J = "type Claims @jwt { sub: String!  roles: [String!] }\n";
  const trips = `type Person @node { key: String! @key }
    type Membership @relationshipProperties { rsvp: String! @filterable }
    type Trip @node { key: String! @key
      members: [Person!]! @relationship(type: "MEMBER", direction: IN, properties: "Membership") @filterable }`;
  const seed = [
    "CREATE (t:Trip {key:'t1'}), (:Person {key:'u'})-[:MEMBER {rsvp:'GOING'}]->(t), (:Person {key:'v'})-[:MEMBER {rsvp:'INVITED'}]->(t)",
  ];
  const going = `membersConnection: { some: { node: { key: { eq: "$jwt.sub" } }, edge: { rsvp: { eq: "GOING" } } } }`;
  const as = (sub: string) => ({ jwt: { sub, roles: [] } });

  test("validate rules: only a GOING member may create a stop", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        J +
        trips +
        `
      type Stop @node @mutation @authorization(validate: [{ operations: [CREATE], where: { node: { trip: { ${going} } } } }]) {
        key: String! @key
        trip: Trip! @relationship(type: "STOP_OF", direction: OUT) }`,
      seed,
    });
    const create = (key: string) =>
      `mutation { createStops(input: [{ key: "${key}", trip: { connect: { key: "t1" } } }]) { stops { key } } }`;
    expect(codes(await t.run(create("s1"), {}, as("v")))).toEqual([
      "FORBIDDEN",
    ]);
    const ok = await t.run(create("s2"), {}, as("u"));
    expect(ok.errors).toBeUndefined();
    const rows = (await t.db.execute(
      "MATCH (s:Stop) RETURN s.key AS k ORDER BY k",
    )) as { rows: Array<Record<string, unknown>> };
    expect(rows.rows.map((r) => r["k"])).toEqual(["s2"]);
  });

  test("filter rules: a trip is visible to its GOING members only", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs:
        J +
        trips.replace(
          "type Trip @node {",
          `type Trip @node @authorization(filter: [{ where: { node: { ${going} } } }]) {`,
        ),
      seed,
    });
    const q = "{ trips { key } }";
    expect(keysOf(await t.run(q, {}, as("u")), "trips")).toEqual(["t1"]);
    expect(keysOf(await t.run(q, {}, as("v")), "trips")).toEqual([]);
    expect(keysOf(await t.run(q, {}, as("w")), "trips")).toEqual([]);
  });

  test("the model checks the connection test", async () => {
    const build = (rule: string) =>
      createTestLoraGraphQL({
        typeDefs:
          J +
          trips.replace(
            "type Trip @node {",
            `type Trip @node @authorization(filter: [{ where: { node: { ${rule} } } }]) {`,
          ),
      });
    await expect(
      build(`membersConnection: { some: { edge: { nope: { eq: "x" } } } }`),
    ).rejects.toThrow(/Membership has no field nope/);
    await expect(build(`membersConnection: { some: {} }`)).rejects.toThrow(
      /a rule must test something/,
    );
    await expect(
      build(`membersConnection: { any: { edge: { rsvp: { eq: "x" } } } }`),
    ).rejects.toThrow(/expected some, all, none or single/);
  });
});
