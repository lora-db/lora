// Mutation statements start from keys, so none may expand from a full
// scan. A key test written in the same pattern as the expansion plans as
// scan, expand, filter, and walks every relationship of the type.

import { createDatabase } from "@loradb/lora-node";
import { graphql } from "graphql";
import { beforeEach, expect, test } from "vitest";
import {
  LoraGraphQL,
  loraDriver,
  scanExpands,
  type LoraDriver,
  type StatementEvent,
} from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Venue @node @mutation {
    key: String! @key
    name: String!
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
  }
`;

let driver: LoraDriver;
let statements: StatementEvent[];
let run: (source: string) => Promise<void>;

beforeEach(async () => {
  const db = await createDatabase();
  driver = loraDriver(db);
  statements = [];
  const lora = new LoraGraphQL({
    typeDefs,
    driver,
    onStatement: (e) => statements.push(e),
  });
  await lora.assertSchema({ create: true });
  const schema = lora.getSchema();
  run = async (source) => {
    const r = await graphql({ schema, source, contextValue: {} });
    if (r.errors) throw r.errors[0];
  };
});

async function findings() {
  const out = [];
  for (const { statement } of statements) {
    const plan = await driver.explain!(statement);
    out.push(...scanExpands(statement, plan));
  }
  return out;
}

test("no mutation statement expands from a full scan", async () => {
  await run(`mutation {
    createFans(input: [{ key: "f1" }, { key: "f2" }]) { info { nodesCreated } }
  }`);
  await run(`mutation {
    createVenues(input: [{
      key: "v1", name: "One",
      stages: { create: [{ node: { key: "s1" } }] }
      fans: { connect: [{ key: "f1", edge: { stars: 3 } }, { key: "f2" }] }
    }]) { info { nodesCreated } }
  }`);
  await run(`mutation {
    createBookings(input: [{ key: "b1", venue: { connect: { key: "v1" } } }]) { info { nodesCreated } }
  }`);
  await run(`mutation {
    updateVenue(key: "v1", update: {
      fans: {
        update: [{ key: "f1", edge: { stars: 5 } }, { key: "f2", node: { name: "Two" } }]
        disconnect: ["f2"]
        connect: [{ key: "f2", edge: { stars: 1 } }]
      }
      stages: { update: [{ key: "s1", node: { name: "Main" } }] }
    }) { info { nodesUpdated } }
  }`);
  await run(`mutation {
    updateBooking(key: "b1", update: { venue: { disconnect: true } }) { info { relationshipsDeleted } }
  }`);
  await run(
    `mutation { deleteVenue(key: "v1") { nodesDeleted relationshipsDeleted } }`,
  );

  expect(statements.length).toBeGreaterThan(10);
  expect(await findings()).toEqual([]);
});

test("the rule catches an expansion from a full scan", async () => {
  const text = (match: string) => ({
    text: `UNWIND $rows AS row ${match} DELETE r`,
    params: { rows: [] },
  });
  // A test through a function cannot seek, so the plan scans the label and
  // expands every LIKES relationship. (Since the engine pushes conditions
  // down, a plain key test in the pattern seeks too.)
  const inPattern = text(
    "MATCH (a:Venue)<-[r:LIKES]-(b:Fan) WHERE toLower(a.key) = row.from AND toLower(b.key) = row.to",
  );
  const seekFirst = text(
    "MATCH (a:Venue) WHERE a.key = row.from MATCH (a)<-[r:LIKES]-(b:Fan) WHERE b:Fan AND b.key = row.to",
  );
  expect(
    scanExpands(inPattern, await driver.explain!(inPattern)).map((f) => f.rule),
  ).toEqual(["scan-expand"]);
  expect(scanExpands(seekFirst, await driver.explain!(seekFirst))).toEqual([]);
});

test("a create does not look for relationships its new nodes cannot have", async () => {
  await run(
    `mutation { createFans(input: [{ key: "f1" }]) { info { nodesCreated } } }`,
  );
  statements.length = 0;
  await run(`mutation {
    createStages(input: [{ key: "s1", venue: { create: { node: { key: "v1", name: "One" } } } }]) {
      info { nodesCreated }
    }
  }`);
  await run(`mutation {
    createVenues(input: [{ key: "v2", name: "Two", fans: { connect: [{ key: "f1" }] } }]) {
      info { relationshipsCreated }
    }
  }`);
  const texts = statements.map((s) => s.statement.text);
  // No delete of an existing pair, and no count of Stage.venue.
  expect(texts.filter((t) => t.includes("DELETE"))).toEqual([]);
  expect(texts.filter((t) => t.includes("AS count"))).toEqual([]);
});
