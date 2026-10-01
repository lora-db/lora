// `timing`: execute() reports the request's time, the time spent in
// LoraDB, and both per root field, in extensions.timing.

import { expect, test } from "vitest";
import type { ExecutionTiming } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type F @node @mutation { key: String! @key  name: String @filterable }`;
const seed =
  "UNWIND range(1, 50) AS i CREATE (:F {key: 'f' + i, name: 'n' + i})";
const timingOf = (r: { extensions?: Record<string, unknown> }) =>
  r.extensions?.["timing"] as ExecutionTiming | undefined;

test("off by default", async () => {
  const t = await createTestLoraGraphQL({ typeDefs, seed });
  const r = await t.lora.execute({ source: "{ fs { key } }" });
  expect(timingOf(r)).toBeUndefined();
});

test("per root field, by response key, with database time inside total", async () => {
  const t = await createTestLoraGraphQL({ typeDefs, seed, timing: true });
  const r = await t.lora.execute({
    source: '{ all: fs(limit: 50) { key } one: f(key: "f1") { name } }',
  });
  expect(r.errors).toBeUndefined();
  const timing = timingOf(r)!;
  expect(Object.keys(timing.fields).sort()).toEqual(["all", "one"]);
  for (const f of Object.values(timing.fields)) {
    expect(f.totalMs).toBeGreaterThanOrEqual(0);
    expect(f.databaseMs).toBeGreaterThanOrEqual(0);
    expect(f.databaseMs).toBeLessThanOrEqual(f.totalMs + 0.01);
  }
  expect(timing.totalMs).toBeGreaterThanOrEqual(timing.databaseMs);
  expect(timing.databaseMs).toBeCloseTo(
    timing.fields["all"]!.databaseMs + timing.fields["one"]!.databaseMs,
    1,
  );
  // The cost estimate is still there.
  expect(r.extensions?.["cost"]).toBeGreaterThan(0);
});

test("mutations are timed, their statements as database time", async () => {
  const t = await createTestLoraGraphQL({ typeDefs, seed, timing: true });
  const r = await t.lora.execute({
    source:
      'mutation { a: createFs(input: [{ key: "x" }]) { fs { key } } b: updateF(key: "f2", update: { name: "y" }) { f { name } } }',
  });
  expect(r.errors).toBeUndefined();
  const timing = timingOf(r)!;
  expect(Object.keys(timing.fields).sort()).toEqual(["a", "b"]);
  expect(timing.fields["a"]!.databaseMs).toBeGreaterThan(0);
  expect(timing.fields["b"]!.databaseMs).toBeGreaterThan(0);
});

test("decided per request from the context", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs,
    seed,
    timing: (context) =>
      (context as { jwt?: { roles?: string[] } }).jwt?.roles?.includes(
        "admin",
      ) ?? false,
  });
  const query = { source: "{ fs { key } }" };
  expect(
    timingOf(
      await t.lora.execute({
        ...query,
        context: { jwt: { sub: "u", roles: [] } },
      }),
    ),
  ).toBeUndefined();
  expect(
    timingOf(
      await t.lora.execute({
        ...query,
        context: { jwt: { sub: "a", roles: ["admin"] } },
      }),
    ),
  ).toBeDefined();
});
