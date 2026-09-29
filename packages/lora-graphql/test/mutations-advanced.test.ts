import { createDatabase } from "@loradb/lora-node";
import { graphql } from "graphql";
import { LoraGraphQL, loraDriver, type WriteChange } from "../src/index.js";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Venue @node @mutation {
    key: String! @key
    name: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
    capacity: Int @filterable(byValue: [EQ, GT])
    rating: Float
    tags: [String!]
    createdAt: DateTime! @timestamp(operations: [CREATE]) @sortable
    slug: String @populatedBy(callback: "slug", operations: [CREATE, UPDATE])
    code: String @settable(onCreate: true, onUpdate: false)
    secret: String @selectable(onRead: false)
    stages: [Stage!]!
      @relationship(type: "HAS_STAGE", direction: OUT, onDelete: CASCADE)
    bookings: [Booking!]!
      @relationship(type: "BOOKED_AT", direction: IN, onDelete: RESTRICT)
    fans: [Fan!]!
      @relationship(type: "LIKES", direction: IN, properties: "Likes")
  }
  type Stage @node @mutation {
    key: String! @key
    name: String
    venue: Venue! @relationship(type: "HAS_STAGE", direction: IN)
  }
  type Booking @node @mutation {
    key: String! @key
    venue: Venue @relationship(type: "BOOKED_AT", direction: OUT)
  }
  type Fan @node @mutation {
    key: String! @key
    name: String
  }
  type Likes @relationshipProperties {
    stars: Int
    note: String
  }
`;

let h: Harness;
const changes: WriteChange[] = [];

async function harness() {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    callbacks: {
      slug: ({ input, key }) =>
        `${String(input["name"] ?? key)
          .toLowerCase()
          .replace(/\s+/g, "-")}`,
    },
  });
  await lora.assertSchema({ create: true });
  const schema = lora.getSchema();
  const run = (
    source: string,
    variables: Record<string, unknown> = {},
    contextValue: Record<string, unknown> = {},
  ) => graphql({ schema, source, variableValues: variables, contextValue });
  const data = async <T>(
    source: string,
    variables?: Record<string, unknown>,
    context?: Record<string, unknown>,
  ) => {
    const r = await run(source, variables, context);
    if (r.errors) throw r.errors[0];
    return r.data as T;
  };
  return { db, lora, statements: [], run, data } as unknown as Harness;
}

beforeEach(async () => {
  h = await harness();
  changes.length = 0;
  h.lora.onWrite((c) => changes.push(c));
  await h.data(`mutation {
    createVenues(input: [
      { key: "v1", name: "Big Hall", capacity: 100, rating: 4.0, tags: ["a", "b"], code: "X1", secret: "s",
        stages: { create: [{ node: { key: "s1", name: "Main" } }] },
        fans: { create: [{ node: { key: "f1" }, edge: { stars: 3, note: "ok" } }] } }
      { key: "v2", name: "Small Room", capacity: 20 }
    ]) { info { nodesCreated } }
  }`);
  changes.length = 0;
});

const one = async (cypher: string) => (await h.db.execute(cypher)).rows[0];

test("a non-null @sortable @timestamp is set inside the CREATE", async () => {
  const d = await h.data<{ venue: { createdAt: string } }>(
    `{ venue(key: "v1") { createdAt } }`,
  );
  expect(d.venue.createdAt).toMatch(/^\d{4}-/);
});

test("@populatedBy computes on create and update; @settable and @selectable", async () => {
  expect(
    await one(
      "MATCH (v:Venue {key: 'v1'}) RETURN v.slug AS s, v.secret AS secret",
    ),
  ).toEqual({
    s: "big-hall",
    secret: "s",
  });
  await h.data(
    `mutation { updateVenue(key: "v1", update: { name: "Huge Hall" }) { info { nodesUpdated } } }`,
  );
  expect(
    (await one("MATCH (v:Venue {key: 'v1'}) RETURN v.slug AS s"))!["s"],
  ).toBe("huge-hall");
  const code = await h.run(
    `mutation { updateVenue(key: "v1", update: { code: "X2" }) { info { nodesUpdated } } }`,
  );
  expect(code.errors?.[0]?.message).toMatch(
    /"code" is not defined by type "VenueUpdateInput"/,
  );
  const secret = await h.run(`{ venue(key: "v1") { secret } }`);
  expect(secret.errors?.[0]?.message).toMatch(/Cannot query field "secret"/);
});

test("adjust: math and list operators, atomically", async () => {
  const d = await h.data<{ updateVenue: { venue: unknown } }>(`mutation {
    updateVenue(key: "v1", adjust: { capacity: { add: 5 }, rating: { multiply: 1.5 }, tags: { push: ["c"] } }) {
      venue { capacity rating tags }
    }
  }`);
  expect(d.updateVenue.venue).toEqual({
    capacity: 105,
    rating: 6,
    tags: ["a", "b", "c"],
  });
  await h.data(
    `mutation { updateVenue(key: "v1", adjust: { capacity: { divide: 2 }, tags: { pop: 2 } }) { info { nodesUpdated } } }`,
  );
  expect(
    await one(
      "MATCH (v:Venue {key: 'v1'}) RETURN v.capacity AS c, v.tags AS t",
    ),
  ).toEqual({ c: 52, t: ["a"] });
  await h.data(
    `mutation { updateVenue(key: "v2", adjust: { rating: { add: 1 }, tags: { push: ["x", "y", "x"] } }) { info { nodesUpdated } } }`,
  );
  await h.data(
    `mutation { updateVenue(key: "v2", adjust: { tags: { remove: ["x"] } }) { info { nodesUpdated } } }`,
  );
  expect(
    await one("MATCH (v:Venue {key: 'v2'}) RETURN v.rating AS r, v.tags AS t"),
  ).toEqual({ r: 1, t: ["y"] });
  const both = await h.run(
    `mutation { updateVenue(key: "v1", update: { capacity: 1 }, adjust: { capacity: { add: 1 } }) { info { nodesUpdated } } }`,
  );
  expect(both.errors?.[0]?.message).toBe(
    "Venue.capacity is both set and adjusted",
  );
  const two = await h.run(
    `mutation { updateVenue(key: "v1", adjust: { capacity: { add: 1, subtract: 1 } }) { info { nodesUpdated } } }`,
  );
  expect(two.errors?.[0]?.message).toBe(
    "adjust.capacity takes exactly one operation",
  );
});

test("relationship properties update in place; reconnecting keeps them", async () => {
  const d = await h.data<{ updateVenue: { info: unknown } }>(`mutation {
    updateVenue(key: "v1", update: { fans: { update: [{ key: "f1", edge: { stars: 5 } }] } }) {
      info { relationshipsCreated relationshipsDeleted }
    }
  }`);
  expect(d.updateVenue.info).toEqual({
    relationshipsCreated: 0,
    relationshipsDeleted: 0,
  });
  expect(
    await one(
      "MATCH (:Fan)-[l:LIKES]->(:Venue {key: 'v1'}) RETURN l.stars AS s, l.note AS n",
    ),
  ).toEqual({ s: 5, n: "ok" });
  const re = await h.data<{ updateVenue: { info: unknown } }>(`mutation {
    updateVenue(key: "v1", update: { fans: { connect: [{ key: "f1" }] } }) { info { relationshipsCreated relationshipsDeleted } }
  }`);
  expect(re.updateVenue.info).toEqual({
    relationshipsCreated: 0,
    relationshipsDeleted: 0,
  });
  expect(
    await one(
      "MATCH (:Fan)-[l:LIKES]->(:Venue {key: 'v1'}) RETURN l.stars AS s, count(l) AS c",
    ),
  ).toEqual({ s: 5, c: 1 });
  const missing = await h.run(
    `mutation { updateVenue(key: "v2", update: { fans: { update: [{ key: "f1", edge: { stars: 1 } }] } }) { info { nodesUpdated } } }`,
  );
  expect(missing.errors?.[0]?.extensions?.["code"]).toBe("NOT_FOUND");
});

test("nested update of connected nodes", async () => {
  await h.data(`mutation {
    updateVenue(key: "v1", update: { stages: { update: [{ key: "s1", node: { name: "Mainstage" } }] } }) { info { nodesUpdated } }
  }`);
  expect(
    (await one("MATCH (s:Stage {key: 's1'}) RETURN s.name AS n"))!["n"],
  ).toBe("Mainstage");
  expect(
    changes[0]!.updated.map((e) => `${e.type}:${String(e.key)}`).sort(),
  ).toEqual(["Stage:s1", "Venue:v1"]);
});

test("bulk update and delete by where, bounded", async () => {
  const d = await h.data<{
    updateVenues: { venues: Array<{ key: string; capacity: number }> };
  }>(`mutation {
    updateVenues(where: { capacity: { gt: 10 } }, adjust: { capacity: { add: 1 } }) { venues { key capacity } info { nodesUpdated } }
  }`);
  expect(d.updateVenues.venues).toEqual([
    { key: "v1", capacity: 101 },
    { key: "v2", capacity: 21 },
  ]);
  const over = await h.run(
    `mutation { updateVenues(where: { capacity: { gt: 10 } }, update: { rating: 1 }, limit: 1) { info { nodesUpdated } } }`,
  );
  expect(over.errors?.[0]?.extensions?.["code"]).toBe("LIMIT_EXCEEDED");
  const empty = await h.run(
    `mutation { updateVenues(where: {}, update: { rating: 1 }) { info { nodesUpdated } } }`,
  );
  expect(empty.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  const del = await h.data<{ deleteVenues: unknown }>(
    `mutation { deleteVenues(where: { name: { eq: "Small Room" } }) { nodesDeleted } }`,
  );
  expect(del.deleteVenues).toEqual({ nodesDeleted: 1 });
});

test("onDelete: CASCADE deletes what the field reaches; RESTRICT refuses", async () => {
  const d = await h.data<{ deleteVenue: unknown }>(
    `mutation { deleteVenue(key: "v1") { nodesDeleted relationshipsDeleted } }`,
  );
  expect(d.deleteVenue).toEqual({ nodesDeleted: 2, relationshipsDeleted: 2 });
  expect((await one("MATCH (s:Stage) RETURN count(s) AS c"))!["c"]).toBe(0);
  expect(changes[0]!.deleted.map((e) => e.type).sort()).toEqual([
    "Stage",
    "Venue",
  ]);

  await h.data(
    `mutation { createBookings(input: [{ key: "b1", venue: { connect: { key: "v2" } } }]) { info { nodesCreated } } }`,
  );
  const r = await h.run(`mutation { deleteVenue(key: "v2") { nodesDeleted } }`);
  expect(r.errors?.[0]?.message).toBe(
    'Venue "v2" still has bookings (onDelete: RESTRICT); remove them first',
  );
});

test("required relationships: on create, and when their target goes away", async () => {
  const empty = await h.run(
    `mutation { createStages(input: [{ key: "s9", venue: {} }]) { info { nodesCreated } } }`,
  );
  expect(empty.errors?.[0]?.message).toBe(
    "Stage.venue is required: connect or create one",
  );
  const moved = await h.run(
    `mutation { updateVenue(key: "v1", update: { stages: { disconnect: ["s1"] } }) { info { nodesUpdated } } }`,
  );
  expect(moved.errors?.[0]?.message).toBe(
    'Stage "s1" requires a Venue (Stage.venue)',
  );
});

test("a caller-owned transaction commits GraphQL and Cypher together", async () => {
  const tx = await h.lora.begin();
  await h.data(
    `mutation { createFans(input: [{ key: "f9" }]) { info { nodesCreated } } }`,
    {},
    { transaction: tx },
  );
  await tx.execute("MATCH (f:Fan {key: 'f9'}) SET f.name = 'Nine'");
  const inside = await h.data<{ fan: unknown }>(
    `{ fan(key: "f9") { name } }`,
    {},
    { transaction: tx },
  );
  expect(inside.fan).toEqual({ name: "Nine" });
  expect(changes).toEqual([]);
  await tx.commit();
  expect(changes.map((c) => c.created)).toEqual([[{ type: "Fan", key: "f9" }]]);

  const tx2 = await h.lora.begin();
  await h.data(
    `mutation { createFans(input: [{ key: "f10" }]) { info { nodesCreated } } }`,
    {},
    { transaction: tx2 },
  );
  await tx2.rollback();
  expect(
    (await one("MATCH (f:Fan {key: 'f10'}) RETURN count(f) AS c"))!["c"],
  ).toBe(0);
  expect(changes).toHaveLength(1);
});

test("the cost limit holds per operation, not per root field", async () => {
  const tight = await festivalHarness({ maxCost: 60 });
  const one = await tight.run(`{ festivals(limit: 40) { key } }`);
  expect(one.errors).toBeUndefined();
  const aliased = await tight.run(
    `{ a: festivals(limit: 40) { key } b: festivals(limit: 40) { key } }`,
    {},
    {},
  );
  expect(aliased.errors?.[0]?.extensions?.["code"]).toBe("COST_EXCEEDED");
});
