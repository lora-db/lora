import { graphql } from "graphql";
import { expect, test } from "vitest";
import { createDatabase } from "@loradb/lora-node";
import {
  LoraGraphQL,
  loraDriver,
  type CostEvent,
  type SpanLike,
  type StatementEndEvent,
  type TracerLike,
} from "../src/index.js";
import { appTypeDefs } from "./fixtures.js";

interface RecordedSpan {
  name: string;
  parent: string | undefined;
  attributes: Record<string, unknown>;
  status?: number;
  ended: boolean;
}

/** A tracer that records spans and their nesting, like OTel's context. */
function recordingTracer() {
  const spans: RecordedSpan[] = [];
  const stack: RecordedSpan[] = [];
  const tracer: TracerLike = {
    startActiveSpan<T>(name: string, fn: (span: SpanLike) => T): T {
      const rec: RecordedSpan = {
        name,
        parent: stack.at(-1)?.name,
        attributes: {},
        ended: false,
      };
      spans.push(rec);
      const span: SpanLike = {
        setAttribute: (k, v) => (rec.attributes[k] = v),
        setStatus: (s) => (rec.status = s.code),
        recordException: () => undefined,
        end: () => {
          rec.ended = true;
        },
      };
      stack.push(rec);
      try {
        const out = fn(span);
        return out instanceof Promise
          ? (out.finally(() => stack.splice(stack.indexOf(rec), 1)) as T)
          : (stack.splice(stack.indexOf(rec), 1), out);
      } catch (err) {
        stack.splice(stack.indexOf(rec), 1);
        throw err;
      }
    },
  };
  return { tracer, spans };
}

async function setup(options: Record<string, unknown> = {}) {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: loraDriver(db),
    ...options,
  });
  await lora.assertSchema({ create: true });
  await db.execute(
    "UNWIND range(1, 5) AS i CREATE (:Festival {key: 'f' + toString(i), name: 'F' + toString(i), capacity: i})",
  );
  await db.execute("CREATE (:User {key: 'u1', name: 'U'})");
  return { db, lora, schema: lora.getSchema() };
}

test("onStatementEnd reports reads, writes, errors and persisted ids", async () => {
  const events: StatementEndEvent[] = [];
  const { lora } = await setup({
    onStatementEnd: (e: StatementEndEvent) => events.push(e),
  });
  lora.persist({ top: `query Top { festivals(limit: 3) { key } }` });

  await lora.execute({ id: "top" });
  expect(events).toHaveLength(1);
  expect(events[0]).toMatchObject({
    field: "festivals",
    mode: "read",
    rows: 3,
    operationName: "Top",
    persistedId: "top",
  });
  expect(events[0]!.durationMs).toBeGreaterThanOrEqual(0);
  expect(events[0]!.cost).toBeGreaterThan(0);

  events.length = 0;
  await lora.execute({
    source: `mutation Add { createFestivals(input: [{ key: "n1", name: "N", followers: { connect: [{ key: "u1" }] } }]) { info { nodesCreated } } }`,
  });
  expect(events.length).toBeGreaterThanOrEqual(2);
  expect(
    events.every((e) => e.mode === "write" && e.statements.length === 1),
  ).toBe(true);
  expect(events.every((e) => e.operationName === "Add")).toBe(true);

  events.length = 0;
  const bad = await lora.execute({
    source: `mutation { createFestivals(input: [{ key: "n1", name: "Dup" }]) { info { nodesCreated } } }`,
  });
  expect(bad.errors).toBeDefined();
  expect(events.some((e) => e.error !== undefined)).toBe(true);
});

test("spans: a field span holds its cypher spans", async () => {
  const { tracer, spans } = recordingTracer();
  const { schema } = await setup({ tracer, traceStatements: true });
  await graphql({
    schema,
    source: `query Q { festivals(limit: 2) { key } }`,
    contextValue: {},
  });
  const field = spans.find((s) => s.name === "lora.graphql.field")!;
  const cypher = spans.find((s) => s.name === "lora.cypher")!;
  expect(field.attributes).toMatchObject({
    "graphql.field.name": "festivals",
    "graphql.operation.name": "Q",
  });
  expect(cypher.parent).toBe("lora.graphql.field");
  expect(cypher.attributes).toMatchObject({
    "db.system": "loradb",
    "db.operation.name": "read",
    "db.response.returned_rows": 2,
  });
  expect(String(cypher.attributes["db.statement"])).toContain("MATCH");
  expect(spans.every((s) => s.ended)).toBe(true);
});

test("db.statement stays out of spans unless asked for", async () => {
  const { tracer, spans } = recordingTracer();
  const { schema } = await setup({ tracer });
  await graphql({
    schema,
    source: `{ festivals(limit: 1) { key } }`,
    contextValue: {},
  });
  expect(spans.some((s) => "db.statement" in s.attributes)).toBe(false);
});

test("metrics: counters and histograms per statement call", async () => {
  const recorded: Array<[string, string, number]> = [];
  const metrics = {
    counter: (name: string, value: number) =>
      recorded.push(["counter", name, value]),
    histogram: (name: string, value: number) =>
      recorded.push(["histogram", name, value]),
  };
  const { schema } = await setup({ metrics });
  await graphql({
    schema,
    source: `{ festivals(limit: 1) { key } }`,
    contextValue: {},
  });
  const names = recorded.map(([kind, name]) => `${kind}:${name}`);
  expect(names).toEqual(
    expect.arrayContaining([
      "counter:lora.graphql.statements",
      "histogram:lora.graphql.statement.duration",
      "histogram:lora.graphql.cost",
    ]),
  );
});

test("budget(context) sets the limit per request; onCost sees each field", async () => {
  const costs: CostEvent[] = [];
  const { lora } = await setup({
    budget: (context: unknown) =>
      (context as { plan?: string }).plan === "free" ? 5 : undefined,
    onCost: (e: CostEvent) => costs.push(e),
  });
  const source = `{ festivals(limit: 20) { key } }`;
  const free = await lora.execute({ source, context: { plan: "free" } });
  expect(free.errors?.[0]?.extensions?.["code"]).toBe("COST_EXCEEDED");
  expect(free.errors?.[0]?.extensions?.["maxCost"]).toBe(5);
  const paid = await lora.execute({ source, context: { plan: "paid" } });
  expect(paid.errors).toBeUndefined();
  expect(costs.map((c) => c.limit)).toEqual([5, 50_000]);
  expect(costs[1]).toMatchObject({ field: "festivals" });
});

test("execute() reports the operation's cost estimate in extensions", async () => {
  const { lora } = await setup();
  const r = await lora.execute({ source: `{ festivals(limit: 3) { key } }` });
  expect(r.extensions?.["cost"]).toBeGreaterThan(0);
});
