import { expect, test } from "vitest";
import { createDatabase } from "@loradb/lora-node";
import {
  LoraGraphQL,
  loraDriver,
  type LoraDatabaseLike,
} from "../src/index.js";
import { appTypeDefs } from "./fixtures.js";

// A database that records how the driver calls it.
function recording() {
  const calls: string[] = [];
  const result = { columns: [], rows: [] };
  const db = {
    transaction: async (
      statements: Array<{ query: string }>,
      mode?: string,
      options?: { timeoutMs?: number },
    ) => {
      calls.push(`transaction ${mode} ${options?.timeoutMs}`);
      return statements.map(() => result);
    },
    execute: async (
      _q: string,
      _p: unknown,
      options?: { timeoutMs?: number },
    ) => {
      calls.push(`execute ${options?.timeoutMs}`);
      return result;
    },
    stream: (_q: string, _p: unknown, options?: { timeoutMs?: number }) => {
      calls.push(`stream ${options?.timeoutMs}`);
      return { columns: () => [], toArray: async () => [] };
    },
  };
  return { db: db as unknown as LoraDatabaseLike, calls };
}

const one = [{ text: "MATCH (n) RETURN n", params: {} }];
const read = { mode: "read" as const, timeoutMs: 50, verified: true };

test("only a bounded read streams; other single reads run off the JS thread", async () => {
  // db.stream() runs the whole query on the JS thread: fine for one keyed
  // row, but any larger read there would block every other request.
  const { db, calls } = recording();
  const driver = loraDriver(db);
  await driver.run(one, { ...read, bounded: true });
  await driver.run(one, read);
  expect(calls).toEqual(["stream 50", "execute 50"]);
});

test("several statements share one read-only transaction; writes one read-write", async () => {
  const { db, calls } = recording();
  const driver = loraDriver(db);
  await driver.run([...one, ...one], read);
  await driver.run(one, { mode: "read", timeoutMs: 50 }); // not verified
  await driver.run(one, { mode: "write", timeoutMs: 50 });
  expect(calls).toEqual([
    "transaction read_only 50",
    "transaction read_only 50",
    "transaction read_write 50",
  ]);
});

test("a lookup by @key is bounded only when it projects stored properties", async () => {
  const lora = new LoraGraphQL({
    typeDefs: appTypeDefs,
    driver: loraDriver(await createDatabase()),
  });
  const bounded = (query: string) =>
    lora.compile(query).map(({ compiled }) => compiled.bounded ?? false);
  expect(bounded(`{ festival(key: "f1") { key name capacity } }`)).toEqual([
    true,
  ]);
  expect(bounded(`{ festival(key: "f1") { key followers { key } } }`)).toEqual([
    false,
  ]);
  expect(bounded(`{ festival(key: "f1") { key followerCount } }`)).toEqual([
    false,
  ]);
  expect(bounded(`{ festivals(limit: 1) { key } }`)).toEqual([false]);
});

test("a single read stops at LIMIT under a deadline", async () => {
  const db = await createDatabase();
  await db.execute("UNWIND range(1, 200000) AS i CREATE (:N {i: i})");
  const driver = loraDriver(db);
  const timed = async (text: string) => {
    const run = () =>
      driver.run([{ text, params: {} }], {
        mode: "read",
        timeoutMs: 10_000,
        verified: true,
      });
    // The fastest of several runs: one sample is noisy under parallel tests.
    let rows: unknown[] = [];
    let ms = Infinity;
    for (let i = 0; i < 5; i++) {
      const started = performance.now();
      const [result] = await run();
      ms = Math.min(ms, performance.now() - started);
      rows = result!.rows;
    }
    return { rows, ms };
  };
  const full = await timed("MATCH (n:N) RETURN count(n.i) AS n");
  const limited = await timed("MATCH (n:N) RETURN n.i AS i LIMIT 3");
  expect(limited.rows).toHaveLength(3);
  expect(limited.ms).toBeLessThan(full.ms / 5);
});
