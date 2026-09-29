import { expect, test } from "vitest";
import { createTestLoraGraphQL, expectSeeks } from "../src/testing.js";
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
