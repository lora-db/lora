// One writer runs at a time. Past `maxQueuedWrites` waiting transactions a
// mutation fails at once with OVERLOADED instead of queuing without bound.

import { createDatabase } from "@loradb/lora-node";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Item @node @mutation {
    key: String! @key
  }
`;

const create = (key: string) =>
  `mutation { createItems(input: [{ key: "${key}" }]) { items { key } } }`;

test("writes past maxQueuedWrites fail fast with OVERLOADED", async () => {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    maxQueuedWrites: 2,
  });
  // Hold the writer lock, so every mutation waits in begin().
  const holder = await lora.begin();
  const results = ["a", "b", "c", "d"].map((key) =>
    lora.execute({ source: create(key) }),
  );
  // The two over the limit answer while the lock is still held.
  const [c, d] = await Promise.all([results[2]!, results[3]!]);
  for (const refused of [c, d]) {
    expect(refused.data).toBeNull();
    expect(refused.errors?.[0]?.extensions?.["code"]).toBe("OVERLOADED");
  }
  await holder.commit();
  const [a, b] = await Promise.all([results[0]!, results[1]!]);
  expect(a.errors).toBeUndefined();
  expect(b.errors).toBeUndefined();
  // The queue drained: a write is accepted again.
  const again = await lora.execute({ source: create("e") });
  expect(again.errors).toBeUndefined();
  const read = await lora.execute({ source: `{ items { key } }` });
  expect(read.data).toEqual({
    items: [{ key: "a" }, { key: "b" }, { key: "e" }],
  });
});

test("reads are not counted and Infinity disables the limit", async () => {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    maxQueuedWrites: Infinity,
  });
  const holder = await lora.begin();
  const writes = Array.from({ length: 20 }, (_, i) =>
    lora.execute({ source: create(`k${i}`) }),
  );
  const read = await lora.execute({ source: `{ items { key } }` });
  expect(read.errors).toBeUndefined();
  await holder.commit();
  for (const r of await Promise.all(writes)) expect(r.errors).toBeUndefined();
});
