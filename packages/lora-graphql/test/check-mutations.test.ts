// check() plans a mutation's statements too: it runs the mutation in a
// transaction it rolls back, then holds each statement to starting from
// keys.

import { createDatabase } from "@loradb/lora-node";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type WriteChange } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Venue
    @node
    @mutation
    @trigger(
      on: [CREATE]
      statement: "MATCH (s:Stage) WHERE toLower(s.name) = toLower(this.name) MATCH (s)-[:AT]->(v:Venue) SET v.twin = true RETURN v AS updated"
    ) {
    key: String! @key
    name: String!
    twin: Boolean
    stages: [Stage!]! @relationship(type: "AT", direction: IN)
  }
  type Stage @node @mutation {
    key: String! @key
    name: String
    venue: Venue @relationship(type: "AT", direction: OUT)
  }
`;

async function setup() {
  const db = await createDatabase();
  const lora = new LoraGraphQL({ typeDefs, driver: loraDriver(db) });
  await lora.assertSchema({ create: true });
  await db.execute("CREATE (:Venue {key: 'v1', name: 'One'})");
  const changes: WriteChange[] = [];
  lora.onWrite((c) => changes.push(c));
  return { db, lora, changes };
}

test("a mutation is planned statement by statement, and nothing is written", async () => {
  const { db, lora, changes } = await setup();
  const report = await lora.check({
    operations: [
      {
        name: "addStage",
        document: `mutation ($key: String!) {
          createStages(input: [{ key: $key, name: "Main", venue: { connect: { key: "v1" } } }]) { stages { key venue { key } } }
        }`,
        variables: { key: "s1" },
      },
      { name: "read", document: `{ venues { key } }` },
    ],
  });
  const plans = report.plans.filter((p) => p.operation === "addStage");
  expect(plans).toHaveLength(1);
  expect(plans[0]!.field).toBe("createStages");
  // The create, the link and the payload read.
  expect(plans[0]!.reports.length).toBeGreaterThanOrEqual(3);
  expect(plans[0]!.reports.flatMap((r) => r.findings)).toEqual([]);
  expect(report.plans.some((p) => p.operation === "read")).toBe(true);
  expect(report.errors).toEqual([]);
  // Rolled back: no node, no change event.
  const { rows } = await db.execute("MATCH (s:Stage) RETURN count(s) AS c");
  expect(rows[0]!["c"]).toBe(0);
  expect(changes).toEqual([]);
});

test("a statement that expands from a full scan fails the check, a trigger's included", async () => {
  const { lora } = await setup();
  const report = await lora.check({
    operations: [
      {
        name: "addVenue",
        document: `mutation { createVenues(input: [{ key: "v2", name: "Two" }]) { venues { key } } }`,
      },
    ],
  });
  const findings = report.plans.flatMap((p) =>
    p.reports.flatMap((r) => r.findings),
  );
  expect(findings.map((f) => f.rule)).toEqual(["scan-expand"]);
  expect(findings[0]!.statement).toContain("toLower(s.name)");
  expect(report.ok).toBe(false);
});

test("a mutation that fails is reported as an error of the operation", async () => {
  const { lora } = await setup();
  const report = await lora.check({
    operations: [
      {
        name: "duplicate",
        document: `mutation { createVenues(input: [{ key: "v1", name: "Again" }]) { venues { key } } }`,
      },
    ],
  });
  expect(report.errors).toHaveLength(1);
  expect(report.errors[0]!.operation).toBe("duplicate");
  expect(report.ok).toBe(false);
});
