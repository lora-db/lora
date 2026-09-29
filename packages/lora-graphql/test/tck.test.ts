// TCK: GraphQL in → Cypher + params out (snapshotted), and every
// statement's plan checked against the access path it was written for.

import type { PlanReport } from "../src/index.js";
import { festivalHarness, type Harness } from "./harness.js";

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness();
});

const cases: Record<
  string,
  { query: string; variables?: Record<string, unknown> }
> = {
  "list, default order by key": { query: `{ festivals { key name } }` },
  "text filter + sort": {
    query: `{ festivals(where: { name: { contains: "land" } }, sort: [{ name: ASC }], limit: 5) { key } }`,
  },
  "range filter": {
    query: `{ festivals(where: { capacity: { gte: 5000, lt: 9000 } }) { key capacity } }`,
  },
  equality: {
    query: `{ festivals(where: { status: { eq: ON_SALE } }) { key } }`,
  },
  "in becomes unwind + seek": {
    query: `{ festivals(where: { key: { in: ["f1", "f2"] }, capacity: { gt: 0 } }) { key } }`,
  },
  "lookup by key": { query: `{ festival(key: "f1") { key title } }` },
  "connection, plan example": {
    query: `{ festivalsConnection(first: 2, where: { name: { contains: "land" } }, sort: [{ name: ASC }]) {
      edges { node { key name genre { name } } } pageInfo { hasNextPage endCursor } } }`,
  },
  "connection after a cursor": {
    query: `query ($after: String) { festivalsConnection(first: 2, after: $after, sort: [{ name: ASC }]) { edges { node { key } } } }`,
    variables: {
      after:
        "eyJzIjoibmFtZTpBU0Msa2V5OkFTQyIsInYiOlsiU3RhcmxhbmQgMTEiLCJmMTEiXX0",
    },
  },
  "relationship filters": {
    query: `{ festivals(where: { followers: { some: { key: { eq: "u1" } }, count: { gt: 1 } }, genre: { name: { startsWith: "H" } } }) { key } }`,
  },
  "nested lists, sorted and unsorted": {
    query: `{ genres { key festivals(limit: 3) { key headliners(sort: [{ name: ASC }]) { name } } } }`,
  },
  "relationship connection with properties": {
    query: `{ festival(key: "f1") { followersConnection(first: 5, where: { edge: { since: { gte: 2000 } } }) { totalCount edges { properties { since } node { key } } } } }`,
  },
  aggregate: {
    query: `{ festivalsAggregate(where: { capacity: { gt: 0 } }) { count capacity { max } } }`,
  },
};

async function firstReport(
  harness: Harness,
  query: string,
  variables?: Record<string, unknown>,
): Promise<PlanReport> {
  const [first] = await harness.lora.explain(query, variables);
  return first!.reports[0]!;
}

describe("TCK", () => {
  test.each(Object.entries(cases))(
    "%s",
    async (_name, { query, variables }) => {
      const compiled = h.lora.compile(query, variables);
      for (const { compiled: c } of compiled) {
        for (const s of c.statements) {
          // Translation rules: nothing that is slow or silently wrong on LoraDB.
          expect(s.text).not.toMatch(
            /OPTIONAL MATCH|COUNT \{|EXISTS \{|\bfirst\(|\] [<>]=? \$/,
          );
        }
      }
      expect(
        compiled.map(({ field, compiled: c }) => ({
          field,
          statements: c.statements,
          reads: c.reads,
        })),
      ).toMatchSnapshot();

      // The same operation executes without errors.
      const r = await h.run(query, variables);
      expect(r.errors).toBeUndefined();

      // S2: every plan uses the access path it was compiled for.
      for (const { reports } of await h.lora.explain(query, variables)) {
        for (const report of reports) expect(report.findings).toEqual([]);
      }
    },
  );

  test("ordering by key streams from the index: no Sort", async () => {
    const report = await firstReport(h, `{ festivals(limit: 3) { key } }`);
    expect(report.operators).toContain("NodeByPropertyRangeScan");
    expect(report.operators).not.toContain("Sort");
  });

  test("a keyset page seeks from the cursor on a non-null sort key", async () => {
    const { query, variables } = cases["connection after a cursor"]!;
    const report = await firstReport(h, query, variables);
    expect(report.operators).toContain("NodeByPropertyRangeScan");
  });

  test("S2 flags a full scan when the inferred index is missing", async () => {
    const bare = await festivalHarness({ assert: false });
    const report = await firstReport(
      bare,
      `{ festivals(where: { name: { contains: "land" } }) { key } }`,
    );
    expect(report.findings).toEqual([
      {
        rule: "full-scan",
        message:
          "expected an index text seek on :Festival (Festival.name contains), plan uses NodeByLabelScan",
        statement: expect.any(String),
      },
    ]);
  });
});

test("public schema", () => {
  expect(h.lora.printPublicSchema()).toMatchSnapshot();
});
