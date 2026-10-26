/**
 * Auto-commit writes that arrive while JavaScript holds the writer lock (an
 * open read-write transaction, a write stream) wait in a first-in,
 * first-out queue per database handle, run by one thread. Before, each one
 * waited on an OS thread of its own: N writes behind a long transaction
 * started N threads.
 */
import { execFileSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { createDatabase, LoraError, openWalDatabase } from "../ts/index.js";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Threads of this process right now (Linux and macOS). */
function threadCount(): number {
  try {
    return readdirSync("/proc/self/task").length;
  } catch {
    // macOS: `ps -M` prints a header and then one line per thread.
    const out = execFileSync("ps", ["-M", "-p", String(process.pid)], {
      encoding: "utf8",
    });
    return out.trim().split("\n").length - 1;
  }
}

/** Resolve `promise`, or fail when it takes longer than `ms`. */
function within<T>(ms: number, promise: Promise<T>, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return Promise.race([
    promise,
    new Promise<never>((_, reject) => {
      timer = setTimeout(
        () => reject(new Error(`${what} did not settle in ${ms} ms`)),
        ms,
      );
    }),
  ]).finally(() => clearTimeout(timer));
}

const WRITES = 2000;

describe("auto-commit writes behind an open transaction", () => {
  it("queue on a bounded number of threads and all complete after commit", async () => {
    const db = await createDatabase();
    const holder = await db.begin("read_write");
    const before = threadCount();
    const order: number[] = [];
    const writes = Array.from({ length: WRITES }, (_, i) =>
      db.execute("CREATE (:W {i: $i})", { i }).then(() => order.push(i)),
    );
    // Give unbounded threads time to show up.
    await sleep(500);
    const during = threadCount();
    expect(during - before).toBeLessThan(32);

    await holder.commit();
    await within(60_000, Promise.all(writes), "queued writes");
    expect(await db.nodeCount()).toBe(WRITES);
    // First in, first out.
    expect(order).toEqual(Array.from({ length: WRITES }, (_, i) => i));
    db.dispose();
  });

  it("let sync() finish while writes are queued", async () => {
    const dir = await mkdtemp(join(tmpdir(), "lora-write-queue-"));
    try {
      for (const db of [
        await createDatabase(),
        await openWalDatabase({ walDir: join(dir, "wal") }),
      ]) {
        const holder = await db.begin("read_write");
        const writes = Array.from({ length: WRITES }, (_, i) =>
          db.execute("CREATE (:W {i: $i})", { i }),
        );
        await sleep(50);
        await within(10_000, db.sync(), "sync()");
        // A read never waits behind the queue either.
        await expect(
          within(10_000, db.execute("RETURN 1 AS x"), "read"),
        ).resolves.toEqual({ columns: ["x"], rows: [{ x: 1 }] });
        await holder.commit();
        await within(60_000, Promise.all(writes), "queued writes");
        expect(await db.nodeCount()).toBe(WRITES);
        await db.sync();
        db.dispose();
      }
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("time out while queued, counting the queued time against queryTimeoutMs", async () => {
    const db = await createDatabase(undefined, { queryTimeoutMs: 100 });
    const holder = await db.begin("read_write");
    const started = Date.now();
    const writes = Array.from({ length: 200 }, (_, i) =>
      db.execute("CREATE (:Late {i: $i})", { i }),
    );
    const settled = await within(
      10_000,
      Promise.allSettled(writes),
      "queued writes",
    );
    // Rejected while the transaction is still open.
    expect(Date.now() - started).toBeLessThan(5_000);
    for (const s of settled) {
      expect(s.status).toBe("rejected");
      const err = (s as PromiseRejectedResult).reason;
      expect(err).toBeInstanceOf(LoraError);
      expect((err as LoraError).code).toBe("LORA_TIMEOUT");
    }
    await holder.commit();
    // No default deadline for this one: the machine may be slow.
    await db.execute("CREATE (:Ok)", undefined, { timeoutMs: 0 });
    expect(await db.nodeCount()).toBe(1);
    db.dispose();
  });

  it("time out or abort while queued behind a write with no deadline", async () => {
    const db = await createDatabase();
    const holder = await db.begin("read_write");
    // Waits for the writer lock with no deadline: the queue's thread is
    // busy with it until the transaction ends.
    const first = db.execute("CREATE (:A)");
    await sleep(20);

    const started = Date.now();
    await expect(
      db.execute("CREATE (:Late)", undefined, { timeoutMs: 50 }),
    ).rejects.toMatchObject({ code: "LORA_TIMEOUT" });
    expect(Date.now() - started).toBeLessThan(3_000);

    const controller = new AbortController();
    const aborted = db.execute("CREATE (:Late)", undefined, {
      signal: controller.signal,
    });
    const queued = db.execute("CREATE (:B)");
    await sleep(20);
    controller.abort();
    await expect(within(3_000, aborted, "aborted write")).rejects.toMatchObject(
      { name: "AbortError" },
    );

    await holder.rollback();
    await first;
    await queued;
    const { rows } = await db.execute(
      "MATCH (n) RETURN labels(n)[0] AS l ORDER BY l",
    );
    expect(rows.map((r) => r.l)).toEqual(["A", "B"]);
    db.dispose();
  });

  it("reject when the database is disposed", async () => {
    const db = await createDatabase();
    const holder = await db.begin("read_write");
    const writes = Array.from({ length: 50 }, (_, i) =>
      db.execute("CREATE (:W {i: $i})", { i }),
    );
    await sleep(20);
    void holder;
    db.dispose();
    const settled = await within(
      10_000,
      Promise.allSettled(writes),
      "queued writes",
    );
    // The one already waiting for the lock may still run once dispose()
    // rolls the transaction back; every queued one is rejected.
    const rejected = settled.filter((s) => s.status === "rejected");
    expect(rejected.length).toBeGreaterThanOrEqual(49);
    for (const s of rejected) {
      expect(String((s as PromiseRejectedResult).reason.message)).toMatch(
        /database is closed/,
      );
    }
  });
});
