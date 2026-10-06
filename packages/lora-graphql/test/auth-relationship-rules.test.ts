// Rules on a relationship field: CONNECT, DISCONNECT, UPDATE_EDGE and
// READ_EDGE, over the relationship's source, target and edge. They hold
// whichever side a write comes from.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const codes = (r: Result) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node @mutation
  @authorization(validate: [{ operations: [UPDATE, DELETE], where: { node: { isViewer: true } } }]) {
  key: String! @key
  trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "TripInvite")
}
type TripInvite @relationshipProperties {
  rsvp: String
  lastReadAt: String @filterable @sortable
}
type Trip @node @mutation {
  key: String! @key
  owner: Person! @relationship(type: "OWNS", direction: IN)
  members: [Person!]! @relationship(type: "MEMBER", direction: IN, properties: "TripInvite") @filterable
    @authorization(validate: [
      # the owner invites
      { operations: [CONNECT], where: { source: { owner: { isViewer: true } } } }
      # the owner removes anyone; a member removes only themselves
      { operations: [DISCONNECT], where: { OR: [{ source: { owner: { isViewer: true } } }, { target: { isViewer: true } }] } }
      # only the member answers their own invitation
      { operations: [UPDATE_EDGE], where: { target: { isViewer: true } } }
      # a member's read marker is theirs alone
      { operations: [READ_EDGE], where: { target: { isViewer: true } } }
    ])
}`;
const seed = [
  "CREATE (lou:Person {key: 'lou'}), (bo:Person {key: 'bo'}), (cy:Person {key: 'cy'}), (t:Trip {key: 't'}), (lou)-[:OWNS]->(t), (lou)-[:MEMBER {rsvp: 'GOING', lastReadAt: 'L'}]->(t)",
];
const as = (sub: string) => ({ jwt: { sub, roles: [] } });

const members = async (t: Awaited<ReturnType<typeof createTestLoraGraphQL>>) =>
  (
    (await t.db.execute(
      "MATCH (p:Person)-[m:MEMBER]->(:Trip {key: 't'}) RETURN p.key AS k, m.rsvp AS rsvp ORDER BY k",
    )) as { rows: Array<Record<string, unknown>> }
  ).rows;

const invite = (who: string, edge = "") =>
  `mutation { updateTrip(key: "t", update: { members: { connect: [{ key: "${who}"${edge} }] } }) { trip { key } } }`;
const joinFromPerson = (who: string) =>
  `mutation { updatePerson(key: "${who}", update: { trips: { connect: [{ key: "t" }] } }) { person { key } } }`;
const removeFromTrip = (who: string) =>
  `mutation { updateTrip(key: "t", update: { members: { disconnect: ["${who}"] } }) { trip { key } } }`;
const leave = (who: string) =>
  `mutation { updatePerson(key: "${who}", update: { trips: { disconnect: ["t"] } }) { person { key } } }`;
const rsvp = (who: string, value: string) =>
  `mutation { updateTrip(key: "t", update: { members: { update: [{ key: "${who}", edge: { rsvp: "${value}" } }] } }) { trip { key } } }`;

describe("CONNECT", () => {
  test("the owner invites", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      invite("bo", ', edge: { rsvp: "INVITED" }'),
      {},
      as("lou"),
    );
    expect(codes(r)).toBeUndefined();
    expect(await members(t)).toEqual([
      { k: "bo", rsvp: "INVITED" },
      { k: "lou", rsvp: "GOING" },
    ]);
  });

  test("a stranger cannot add themselves, from either side", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(codes(await t.run(invite("cy"), {}, as("cy")))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(joinFromPerson("cy"), {}, as("cy")))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(invite("cy")))).toEqual(["UNAUTHENTICATED"]);
    expect((await members(t)).map((m) => m["k"])).toEqual(["lou"]);
  });
});

describe("DISCONNECT", () => {
  test("a member leaves; nobody else removes them but the owner", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.run(invite("bo"), {}, as("lou"));
    await t.run(invite("cy"), {}, as("lou"));
    expect(codes(await t.run(removeFromTrip("bo"), {}, as("cy")))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(leave("bo"), {}, as("bo")))).toBeUndefined();
    expect(
      codes(await t.run(removeFromTrip("cy"), {}, as("lou"))),
    ).toBeUndefined();
    expect((await members(t)).map((m) => m["k"])).toEqual(["lou"]);
  });
});

describe("UPDATE_EDGE", () => {
  test("only the member sets their RSVP", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.run(invite("bo", ', edge: { rsvp: "INVITED" }'), {}, as("lou"));
    expect(codes(await t.run(rsvp("bo", "GOING"), {}, as("lou")))).toEqual([
      "FORBIDDEN",
    ]);
    // A re-connect that sets properties is an edge update too.
    expect(
      codes(
        await t.run(
          invite("bo", ', edge: { rsvp: "DECLINED" }'),
          {},
          as("lou"),
        ),
      ),
    ).toEqual(["FORBIDDEN"]);
    expect(
      codes(await t.run(rsvp("bo", "GOING"), {}, as("bo"))),
    ).toBeUndefined();
    expect(await members(t)).toEqual([
      { k: "bo", rsvp: "GOING" },
      { k: "lou", rsvp: "GOING" },
    ]);
  });
});

describe("READ_EDGE", () => {
  const read = `{ trip(key: "t") { membersConnection(sort: [{ key: ASC }]) { edges { node { key } properties { lastReadAt } } } } }`;

  test("nobody else reads a member's marker", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.run(invite("bo"), {}, as("lou"));
    const own = await t.run(
      `{ trip(key: "t") { membersConnection(where: { node: { key: { eq: "lou" } } }) { edges { properties { lastReadAt } } } } }`,
      {},
      as("lou"),
    );
    expect(own.errors).toBeUndefined();
    expect(JSON.stringify(own.data)).toContain('"lastReadAt":"L"');
    const all = await t.run(read, {}, as("bo"));
    expect(codes(all)).toEqual(["FORBIDDEN"]);
    expect(JSON.stringify(all.data ?? null)).not.toContain('"L"');
  });

  test("nothing filters, sorts or aggregates by the guarded properties", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const probes = [
      `{ trip(key: "t") { membersConnection(where: { edge: { lastReadAt: { eq: "L" } } }) { edges { node { key } } } } }`,
      `{ trip(key: "t") { membersConnection(sort: [{ edge: { lastReadAt: ASC } }]) { edges { node { key } } } } }`,
      `{ trips(where: { membersConnection: { some: { edge: { lastReadAt: { eq: "L" } } } } }) { key } }`,
    ];
    for (const q of probes) {
      const r = await t.run(q, {}, as("lou"));
      expect(codes(r), q).toEqual(["FORBIDDEN"]);
      expect(r.errors?.[0]?.message, q).toMatch(
        /READ_EDGE rules decide per relationship/,
      );
    }
  });
});

describe("model checks", () => {
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test("relationship rules test source, target and edge", () => {
    expect(
      problems(
        typeDefs.replace(
          "{ operations: [CONNECT], where: { source: { owner: { isViewer: true } } } }",
          '{ operations: [CONNECT], where: { node: { key: { eq: "x" } } } }',
        ),
      ),
    ).toContain(
      "@authorization: node: a relationship rule tests source, target and edge",
    );
    expect(
      problems(
        typeDefs.replace(
          "{ target: { isViewer: true } } }\n      # a member's",
          '{ edge: { nope: { eq: "x" } } } }\n      # a member\'s',
        ),
      ),
    ).toContain("@authorization: edge.nope: TripInvite has no field nope");
  });

  test("relationship operations belong on relationship fields, alone", () => {
    expect(
      problems(
        typeDefs.replace(
          "type Trip @node @mutation {",
          'type Trip @node @mutation @authorization(validate: [{ operations: [CONNECT], where: { jwt: { sub: { eq: "x" } } } }]) {',
        ),
      ),
    ).toContain("@authorization: CONNECT rules belong on a relationship field");
    expect(
      problems(
        typeDefs.replace(
          "{ operations: [CONNECT], where",
          "{ operations: [CONNECT, READ], where",
        ),
      ),
    ).toContain(
      "a rule on CONNECT tests source, target and edge; give READ its own rule",
    );
  });
});

describe("rules on both sides of one relationship", () => {
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };
  const bothSides = (operations: string) =>
    typeDefs.replace(
      'trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "TripInvite")',
      `trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "TripInvite")
    @authorization(validate: [{ operations: [${operations}], where: { source: { isViewer: true } } }])`,
    );

  test("the same operation is a model error: one side declares it", () => {
    expect(problems(bothSides("CONNECT, READ_EDGE"))).toEqual([
      "@authorization: CONNECT, READ_EDGE rules are each declared on both Person.trips and Trip.members, the two sides of MEMBER; a relationship's rules hold from either side, so declare each operation on one (combine with AND where both must pass)",
    ]);
  });

  test("different operations may sit on different sides", async () => {
    // Trip.members rules everything but a hypothetical second READ_EDGE.
    const sdl = typeDefs
      .replace(
        `      # a member's read marker is theirs alone
      { operations: [READ_EDGE], where: { target: { isViewer: true } } }
`,
        "",
      )
      .replace(
        'trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "TripInvite")',
        `trips: [Trip!]! @relationship(type: "MEMBER", direction: OUT, properties: "TripInvite")
    @authorization(validate: [{ operations: [READ_EDGE], where: { source: { isViewer: true } } }])`,
      );
    expect(problems(sdl)).toEqual([]);
    const t = await createTestLoraGraphQL({ typeDefs: sdl, seed });
    // Declared on Person.trips, it guards the edge read from Trip.members.
    const q =
      '{ trip(key: "t") { membersConnection { edges { properties { lastReadAt } } } } }';
    expect(codes(await t.run(q, {}, as("lou")))).toBeUndefined();
    expect(codes(await t.run(q, {}, as("bo")))).toEqual(["FORBIDDEN"]);
  });
});

describe("READ_EDGE from either side", () => {
  const sides = (
    rule: "people" | "rooms",
  ) => `type Claims @jwt { sub: String! @viewer(type: "Person", field: "key") }
type Seen @relationshipProperties { at: String }
type Person @node { key: String! @key
  rooms: [Room!]! @relationship(type: "IN", direction: OUT, properties: "Seen")${
    rule === "rooms"
      ? `
    @authorization(validate: [{ operations: [READ_EDGE], where: { source: { isViewer: true } } }])`
      : ""
  } }
type Room @node { key: String! @key
  people: [Person!]! @relationship(type: "IN", direction: IN, properties: "Seen")${
    rule === "people"
      ? `
    @authorization(validate: [{ operations: [READ_EDGE], where: { target: { isViewer: true } } }])`
      : ""
  } }`;
  const seed =
    "CREATE (u:Person {key: 'u'})-[:IN {at: '1'}]->(r:Room {key: 'r'}), (:Person {key: 'v'})-[:IN {at: '2'}]->(r)";
  const u = { jwt: { sub: "u" } };

  for (const rule of ["people", "rooms"] as const) {
    test(`with the rule on ${rule === "people" ? "Room.people" : "Person.rooms"}`, async () => {
      const t = await createTestLoraGraphQL({ typeDefs: sides(rule), seed });
      // The viewer reads their own edge from either end.
      const own = await t.run(
        '{ person(key: "u") { roomsConnection { edges { properties { at } } } } }',
        {},
        u,
      );
      expect(own.errors).toBeUndefined();
      expect(JSON.stringify(own.data)).toContain('"at":"1"');
      const fromRoom = await t.run(
        '{ rooms { peopleConnection(where: { node: { key: { eq: "u" } } }) { edges { properties { at } } } } }',
        {},
        u,
      );
      expect(fromRoom.errors).toBeUndefined();
      // Another person's edge reads FORBIDDEN, from both ends.
      expect(
        codes(
          await t.run(
            '{ person(key: "v") { roomsConnection { edges { properties { at } } } } }',
            {},
            u,
          ),
        ),
      ).toEqual(["FORBIDDEN"]);
      expect(
        codes(
          await t.run(
            '{ rooms { peopleConnection(where: { node: { key: { eq: "v" } } }) { edges { properties { at } } } } }',
            {},
            u,
          ),
        ),
      ).toEqual(["FORBIDDEN"]);
    });
  }
});
