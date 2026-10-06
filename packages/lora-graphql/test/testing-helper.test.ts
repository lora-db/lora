import { describe, expect, test } from "vitest";
import {
  createTestLoraGraphQL,
  expectAccess,
  expectSeeks,
} from "../src/testing.js";
import { festivalTypeDefs, seedStatements } from "./fixtures.js";

test("createTestLoraGraphQL builds, asserts, seeds and records statements", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: festivalTypeDefs,
    seed: seedStatements,
  });
  const d = await t.data<{ festival: { name: string } }>(
    `{ festival(key: "f1") { name } }`,
  );
  expect(d.festival.name).toBe("Moonfest 1");
  expect(t.statements).toHaveLength(1);
  t.close();
});

test("expectSeeks passes for a keyed read and names the scan otherwise", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: festivalTypeDefs,
    seed: seedStatements,
  });
  await expectSeeks(t.lora, `{ festival(key: "f1") { name } }`);
  await expect(
    expectSeeks(
      t.lora,
      `{ festivals(where: { capacity: { gt: 1 } }) { key } }`,
      {},
      { rowBudget: 0 },
    ),
  ).rejects.toThrow(
    /festivals: the engine estimates \d+ rows scanned; the budget is 0/,
  );
});

describe("expectAccess", () => {
  const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node @mutation
  @authorization(validate: [{ operations: [UPDATE, DELETE], where: { node: { isViewer: true } } }]) {
  key: String! @key
  name: String
  trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "Invite")
}
type Invite @relationshipProperties { rsvp: String }
type Trip @node @mutation
  @authorization(filter: [{ where: { OR: [{ node: { owner: { isViewer: true } } }, { node: { members: { some: { isViewer: true } } } }] } }]) {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN, properties: "Invite")
    @authorization(validate: [
      { operations: [CONNECT], where: { source: { owner: { isViewer: true } } } }
      { operations: [UPDATE_EDGE], where: { target: { isViewer: true } } }
    ])
}`;
  const seed = [
    "CREATE (lou:Person {key: 'lou'}), (bo:Person {key: 'bo'}), (:Person {key: 'cy'}), (t:Trip {key: 'lou:tml'}), (lou)-[:OWNS]->(t), (bo)-[:MEMBER {rsvp: 'INVITED'}]->(t)",
  ];

  test("passes when every entry matches, and leaves no trace", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await expectAccess(t, {
      as: { sub: "lou", roles: [] },
      allowed: [
        "read Trip lou:tml",
        'update Person lou {"name": "Lou"}',
        "connect Trip.members lou:tml → lou",
      ],
      denied: [
        'update Person bo {"name": "B"}',
        'update-edge Trip.members lou:tml → bo {"rsvp": "GOING"}',
      ],
    });
    await expectAccess(t, {
      as: { sub: "bo", roles: [] },
      allowed: [
        "read Trip lou:tml",
        'update-edge Trip.members lou:tml → bo {"rsvp": "GOING"}',
      ],
      denied: ["connect Trip.members lou:tml → cy"],
    });
    await expectAccess(t, { as: undefined, denied: ["read Trip lou:tml"] });
    // Every probe rolled back.
    const rows = (await t.db.execute(
      "MATCH (p:Person)-[m:MEMBER]->(:Trip) RETURN p.key AS k, m.rsvp AS rsvp, p.name AS name",
    )) as { rows: Array<Record<string, unknown>> };
    expect(rows.rows).toEqual([{ k: "bo", rsvp: "INVITED", name: null }]);
  });

  test("update-edge on a single relationship", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node { key: String! @key }
type Lease @relationshipProperties { note: String }
type Flat @node @mutation {
  key: String! @key
  tenant: Person @relationship(type: "RENTS", direction: IN, properties: "Lease")
    @authorization(validate: [{ operations: [UPDATE_EDGE], where: { target: { isViewer: true } } }])
}`,
      seed: [
        "CREATE (:Person {key: 'lou'})-[:RENTS {note: 'old'}]->(:Flat {key: 'f'}), (:Person {key: 'bo'})",
      ],
    });
    const entry = 'update-edge Flat.tenant f → lou {"note": "new"}';
    await expectAccess(t, { as: { sub: "lou", roles: [] }, allowed: [entry] });
    await expectAccess(t, { as: { sub: "bo", roles: [] }, denied: [entry] });
    await expectAccess(t, { as: undefined, denied: [entry] });
    const rows = (await t.db.execute(
      "MATCH (:Person)-[r:RENTS]->(:Flat) RETURN r.note AS note",
    )) as { rows: Array<Record<string, unknown>> };
    expect(rows.rows).toEqual([{ note: "old" }]);
  });

  test("reports every mismatch at once", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const failure = await expectAccess(t, {
      as: { sub: "bo", roles: [] },
      allowed: ["connect Trip.members lou:tml → cy", "read Trip lou:tml"],
      denied: ["read Trip lou:tml", "read Trip nope:x"],
    }).then(
      () => undefined,
      (err: Error) => err.message,
    );
    expect(failure).toMatch(/differs in 2 entries/);
    expect(failure).toMatch(
      /expected allowed, was denied: connect Trip\.members lou:tml → cy \(FORBIDDEN/,
    );
    expect(failure).toMatch(/expected denied, was allowed: read Trip lou:tml/);
    expect(failure).not.toMatch(/nope:x/);
  });

  test("refuses entries it cannot read", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await expect(
      expectAccess(t, { as: undefined, allowed: ["fly Trip x"] }),
    ).rejects.toThrow(/expected `<read\|create/);
  });
});

test("expectAccess: a write to a hidden node is denied, not allowed (G-39)", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: `type Claims @jwt { sub: String! @viewer(type: "Person", field: "key") }
    type Person @node { key: String! @key }
    type Trip @node @mutation(operations: [UPDATE, DELETE]) @authorization(public: [UPDATE, DELETE], filter: [{ where: { node: { owner: { isViewer: true } } } }]) {
      key: String! @key
      owner: Person! @relationship(type: "OWNS", direction: IN, nestedOperations: [])
      members: [Person!]! @relationship(type: "MEMBER", direction: IN, nestedOperations: [CONNECT, DISCONNECT]) }`,
    seed: "CREATE (:Person {key: 'lou'})-[:OWNS]->(:Trip {key: 't'}), (:Person {key: 'eve'})",
  });
  await expectAccess(t, {
    as: { sub: "eve" },
    denied: [
      "connect Trip.members t → eve",
      "disconnect Trip.members t → eve",
      "delete Trip t",
    ],
  });
  await expectAccess(t, {
    as: { sub: "lou" },
    allowed: ["connect Trip.members t → eve", "delete Trip t"],
  });
});
