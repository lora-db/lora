import { expect, test } from "vitest";
import { createDatabase } from "@loradb/lora-node";
import { loraDriver, type LoraDatabaseLike } from "../src/index.js";

// A database that records how the driver calls it.
function recording() {
  const calls: string[] = [];
  const db = {
    transaction: async (
      statements: Array<{ query: string }>,
      mode?: string,
      options?: { timeoutMs?: number },
    ) => {
      calls.push(`transaction ${mode} ${options?.timeoutMs}`);
      return statements.map(() => ({ columns: [], rows: [] }));
    },
    stream: () => {
      calls.push("stream");
      throw new Error("stream must not be called");
    },
  };
  return { db: db as unknown as LoraDatabaseLike, calls };
}

test("a verified single read runs in a read-only transaction, not a stream", async () => {
  // db.stream() runs the query on the JS thread: one slow read would
  // block every other request.
  const { db, calls } = recording();
  await loraDriver(db).run([{ text: "MATCH (n) RETURN n", params: {} }], {
    mode: "read",
    timeoutMs: 50,
    verified: true,
  });
  expect(calls).toEqual(["transaction read_only 50"]);
});

test("a read stops at LIMIT in a transaction under a deadline", async () => {
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
