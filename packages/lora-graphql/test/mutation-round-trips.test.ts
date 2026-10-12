// A mutation in a transaction holds the writer lock from begin() to
// commit(), so each call into it is a round trip under the lock:
// statements that do not need an earlier result go in one executeMany()
// call. A create without relationship writes needs no transaction at
// all: it is one write batch that commits as it returns.

import { createDatabase } from "@loradb/lora-node";
import { graphql, type GraphQLSchema } from "graphql";
import { beforeEach, expect, test } from "vitest";
import {
  LoraGraphQL,
  loraDriver,
  type LoraDriver,
  type Statement,
} from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Venue @node @mutation {
    key: String! @key
    name: String! @unique
    stages: [Stage!]!
      @relationship(type: "HAS_STAGE", direction: OUT, onDelete: CASCADE)
  }
  type Stage @node @mutation {
    key: String! @key
    name: String
    venue: Venue! @relationship(type: "HAS_STAGE", direction: IN)
  }
`;

let schema: GraphQLSchema;
/** Statements per call into the transaction, in order. */
let calls: number[] = [];

/** `driver`, counting the calls each transaction receives. */
function counting(driver: LoraDriver, many: boolean): LoraDriver {
  return {
    ...driver,
    // A single-call mutation is one write batch, with no begin().
    run: (statements, options) => {
      if (options.mode === "write") calls.push(statements.length);
      return driver.run(statements, options);
    },
    begin: async (options) => {
      const tx = await driver.begin!(options);
      return {
        execute: (s: Statement) => (calls.push(1), tx.execute(s)),
        ...(many
          ? {
              executeMany: (s: Statement[]) => (
                calls.push(s.length),
                tx.executeMany!(s)
              ),
            }
          : {}),
        commit: () => tx.commit(),
        rollback: () => tx.rollback(),
        get isOpen() {
          return tx.isOpen;
        },
      };
    },
  };
}

async function setup(many: boolean) {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: counting(loraDriver(db), many),
  });
  await lora.assertSchema({ create: true });
  schema = lora.getSchema();
}

async function run(source: string): Promise<unknown> {
  calls = [];
  const r = await graphql({ schema, source, contextValue: {} });
  if (r.errors) throw r.errors[0];
  return r.data;
}

const CREATE = `mutation { createVenues(input: [{ key: "v1", name: "One" }]) { venues { key name } } }`;
const NESTED = `mutation { createVenues(input: [{ key: "v2", name: "Two", stages: { create: [{ node: { key: "s1" } }] } }]) { venues { key stages { key } } } }`;
const UPDATE = `mutation { updateVenue(key: "v1", update: { name: "Uno" }) { venue { key name } } }`;
const CONNECT = `mutation { createStages(input: [{ key: "s9", venue: { connect: { key: "v1" } } }]) { stages { key venue { key } } } }`;

beforeEach(() => setup(true));

test("a create and its payload read are one call", async () => {
  expect(await run(CREATE)).toEqual({
    createVenues: { venues: [{ key: "v1", name: "One" }] },
  });
  expect(calls).toEqual([2]);
});

test("nested creates and their link are one call, the payload another", async () => {
  expect(await run(NESTED)).toEqual({
    createVenues: { venues: [{ key: "v2", stages: [{ key: "s1" }] }] },
  });
  expect(calls).toEqual([3, 1]);
});

test("an update's write shares the call of its payload read", async () => {
  await run(CREATE);
  expect(await run(UPDATE)).toEqual({
    updateVenue: { venue: { key: "v1", name: "Uno" } },
  });
  expect(calls).toEqual([1, 2]);
});

test("a create with a connect is two calls", async () => {
  await run(CREATE);
  expect(await run(CONNECT)).toEqual({
    createStages: { stages: [{ key: "s9", venue: { key: "v1" } }] },
  });
  expect(calls).toEqual([2, 1]);
});

test("a failing create in a batch reports its own error and writes nothing", async () => {
  await run(CREATE);
  await expect(
    run(
      `mutation { createVenues(input: [{ key: "v3", name: "One" }]) { venues { key } } }`,
    ),
  ).rejects.toMatchObject({ extensions: { code: "CONSTRAINT_VIOLATION" } });
  expect(await run(`{ venues { key } }`)).toEqual({ venues: [{ key: "v1" }] });
});

test("a driver without executeMany runs a transaction's statements one by one", async () => {
  await setup(false);
  // A create is a single write batch, which needs no executeMany.
  expect(await run(CREATE)).toEqual({
    createVenues: { venues: [{ key: "v1", name: "One" }] },
  });
  expect(calls).toEqual([2]);
  expect(await run(UPDATE)).toEqual({
    updateVenue: { venue: { key: "v1", name: "Uno" } },
  });
  expect(calls).toEqual([1, 1, 1]);
});
