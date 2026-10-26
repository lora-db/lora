/**
 * `maxOpenTransactions`: every open transaction, running read-write batch
 * and actor-backed write stream has an OS thread of its own, so a
 * database caps how many it runs at once and rejects the next one with
 * `LORA_TOO_MANY_OPEN` (queuing it could deadlock behind a transaction
 * JavaScript has not finished).
 */
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore - resolved at runtime via the native.js loader
import native from "../ts/native.js";
import {
  createDatabase,
  LoraError,
  openWalDatabase,
  type Transaction,
} from "../ts/index.js";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function tooManyOpen(promise: Promise<unknown>): Promise<void> {
  const err = await promise.then(
    () => {
      throw new Error("expected LORA_TOO_MANY_OPEN");
    },
    (e: unknown) => e,
  );
  expect(err).toBeInstanceOf(LoraError);
  expect((err as LoraError).code).toBe("LORA_TOO_MANY_OPEN");
  expect((err as LoraError).message).toMatch(/`maxOpenTransactions` is \d+/);
}

/**
 * Run `start` until it is not refused for the limit. For a slot freed by
 * a thread that is still winding down (a closed stream, a panic), which
 * happens just after the call that caused it returns.
 */
async function once<T>(start: () => Promise<T>): Promise<T> {
  for (let i = 0; ; i++) {
    try {
      return await start();
    } catch (e) {
      const full =
        String((e as Error).message).startsWith("LORA_TOO_MANY_OPEN") ||
        (e as LoraError).code === "LORA_TOO_MANY_OPEN";
      if (!full || i === 200) throw e;
      await sleep(10);
    }
  }
}

describe("maxOpenTransactions", () => {
  it("defaults to 1024 open transactions", async () => {
    const db = await createDatabase();
    const open: Transaction[] = [];
    for (let i = 0; i < 1024; i++) open.push(await db.begin("read_only"));
    await tooManyOpen(db.begin("read_only"));
    await Promise.all(open.map((tx) => tx.rollback()));
    const tx = await db.begin("read_only");
    await tx.rollback();
    db.dispose();
  });

  it("rejects past the limit and frees a slot when one finishes", async () => {
    const db = await createDatabase(undefined, { maxOpenTransactions: 2 });
    const a = await db.begin("read_only");
    const b = await db.begin("read_write");
    await tooManyOpen(db.begin("read_only"));
    // A refused begin() holds nothing: still refused, not leaked.
    await tooManyOpen(db.begin("read_write"));

    await a.commit();
    const c = await db.begin("read_only");
    await tooManyOpen(db.begin("read_only"));

    // A failed statement rolls back and frees the slot too.
    await expect(b.execute("THIS IS NOT CYPHER")).rejects.toThrow();
    expect(b.isOpen).toBe(false);
    const d = await db.begin("read_write");
    await d.execute("CREATE (:Ok)");
    await d.commit();
    await c.rollback();
    expect(await db.nodeCount()).toBe(1);
    db.dispose();
  });

  it("frees the slot of a begin() that timed out waiting for the lock", async () => {
    const db = await createDatabase(undefined, { maxOpenTransactions: 2 });
    const holder = await db.begin("read_write");
    await expect(
      db.begin("read_write", { timeoutMs: 50 }),
    ).rejects.toMatchObject({ code: "LORA_TIMEOUT" });
    const reader = await db.begin("read_only");
    await reader.rollback();
    await holder.rollback();
    db.dispose();
  });

  it("counts a read-write transaction() batch while it runs", async () => {
    const db = await createDatabase(undefined, { maxOpenTransactions: 2 });
    const holder = await db.begin("read_write");
    // Waits for the writer lock on its own thread, holding a slot.
    const batch = db.transaction([{ query: "CREATE (:A)" }]);
    await tooManyOpen(db.begin("read_only"));
    await tooManyOpen(db.transaction([{ query: "CREATE (:B)" }]));
    // A read-only batch runs on the libuv pool, not on a thread of its own.
    await expect(
      db.transaction([{ query: "RETURN 1 AS x" }], "read_only"),
    ).resolves.toHaveLength(1);

    await holder.rollback();
    await batch;
    const a = await db.begin("read_only");
    const b = await db.begin("read_only");
    await Promise.all([a.rollback(), b.rollback()]);
    expect(await db.nodeCount()).toBe(1);
    db.dispose();
  });

  it("counts a write stream on its own thread until it ends or is closed", async () => {
    const db = await createDatabase(undefined, { maxOpenTransactions: 2 });
    // An open read-write transaction makes a write stream open on its own
    // thread (it has to wait for the writer lock).
    const holder = await db.begin("read_write");
    const stream = db.stream("CREATE (n:S) RETURN n");
    await tooManyOpen(db.begin("read_only"));
    expect(() => db.stream("CREATE (n:S) RETURN n")).toThrow(
      expect.objectContaining({ code: "LORA_TOO_MANY_OPEN" }),
    );
    // A read-only stream holds no thread.
    const read = db.stream("RETURN 1 AS x");
    expect(await read.toArray()).toEqual([{ x: 1 }]);

    // Closed, it gives its slot back.
    await holder.rollback();
    await stream.next();
    stream.close();
    const reader = await once(() => db.begin("read_only"));
    await reader.rollback();

    // Drained to its end, a write stream gives its slot back as well.
    const holder2 = await db.begin("read_write");
    const drained = db.stream("CREATE (n:S) RETURN n");
    await holder2.rollback();
    expect(await drained.toArray()).toHaveLength(1);
    const a = await db.begin("read_only");
    const b = await db.begin("read_only");
    await Promise.all([a.rollback(), b.rollback()]);
    db.dispose();
  });

  it("frees every slot on closeWriters()", async () => {
    const db = await native.openDatabase(
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      2,
    );
    await db.beginTransaction("read_only", null);
    await db.beginTransaction("read_write", null);
    // The native call throws; the TS wrapper turns that into a rejection.
    expect(() => db.beginTransaction("read_only", null)).toThrow(
      /^LORA_TOO_MANY_OPEN: /,
    );
    db.closeWriters();
    const a = await db.beginTransaction("read_only", null);
    const b = await db.beginTransaction("read_write", null);
    await db.txFinish(a, false);
    await db.txFinish(b, false);
    db.dispose();
  });

  it("frees the slot of a transaction whose thread panicked", async () => {
    const db = await native.openDatabase(
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      null,
      1,
    );
    const tx = await db.beginTransaction("read_write", null);
    db.__panicInTransactionForTesting(tx);
    // The panic rolled the transaction back: its next call is refused,
    // and the slot and the writer lock are free.
    await expect(
      db.txExecute(tx, "CREATE (:Lost)", null, null, null),
    ).rejects.toThrow(/^LORA_TRANSACTION: /);
    const next = await once(() => db.beginTransaction("read_write", 5_000));
    await db.txExecute(next, "CREATE (:Kept)", null, null, null);
    await db.txFinish(next, true);
    expect(db.nodeCount()).toBe(1);
    db.dispose();
  });

  it("is validated, and accepted by openWalDatabase()", async () => {
    for (const bad of [0, -1, 1.5, Number.NaN]) {
      await expect(
        createDatabase(undefined, { maxOpenTransactions: bad }),
      ).rejects.toMatchObject({ code: "LORA_INVALID_PARAMS" });
    }
    expect(
      () =>
        new native.Database(
          null,
          null,
          null,
          null,
          null,
          null,
          null,
          null,
          null,
          null,
          0,
        ),
    ).toThrow(/^LORA_INVALID_PARAMS: /);

    const dir = await mkdtemp(join(tmpdir(), "lora-open-limit-"));
    try {
      const db = await openWalDatabase({
        walDir: join(dir, "wal"),
        maxOpenTransactions: 1,
      });
      const tx = await db.begin();
      await tooManyOpen(db.begin("read_only"));
      await tx.rollback();
      db.dispose();
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
