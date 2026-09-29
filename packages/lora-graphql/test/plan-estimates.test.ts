import { expect, test } from "vitest";
import { festivalHarness } from "./harness.js";

test("plan reports carry engine row estimates; a row budget flags them", async () => {
  const h = await festivalHarness();
  const query = `{ festivals(where: { capacity: { gt: 10 } }, limit: 5) { key } }`;
  const [plain] = await h.lora.explain(query);
  const estimate = plain!.reports[0]!.estimatedRows;
  expect(estimate).toBeGreaterThan(0);
  expect(plain!.reports[0]!.findings).toEqual([]);

  const report = await h.lora.check({
    operations: [{ name: "big", document: query }],
    rowBudget: estimate! - 1,
  });
  expect(report.ok).toBe(false);
  expect(report.plans[0]!.reports[0]!.findings.map((f) => f.rule)).toEqual([
    "row-budget",
  ]);
});
