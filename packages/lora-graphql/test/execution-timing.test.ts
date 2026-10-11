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
  // The request's database time is the sum over its root fields, which run
  // two at a time: it can exceed the request's wall-clock total.
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

// Regression: a mutation's statements go to the database in one call, and
// each was timed for the whole call, so the database time reported was a
// multiple of the time the mutation took.
test("a batched mutation's database time is counted once", async () => {
  const t = await createTestLoraGraphQL({ typeDefs, seed, timing: true });
  for (let i = 0; i < 20; i++) {
    const r = await t.lora.execute({
      source: `mutation { createFs(input: [{ key: "x${i}", name: "n" }]) { fs { key name } } }`,
    });
    expect(r.errors).toBeUndefined();
    const timing = timingOf(r)!;
    expect(timing.databaseMs).toBeGreaterThan(0);
    expect(timing.databaseMs).toBeLessThanOrEqual(timing.totalMs + 0.01);
  }
  t.close();
});
