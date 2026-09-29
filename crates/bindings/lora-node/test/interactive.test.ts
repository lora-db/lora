import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { createDatabase, LoraError } from "../ts/index.js";

describe("interactive transactions", () => {
  it("reads, decides in JS, then writes atomically", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:Trip {key: 't', capacity: 2, taken: 1})");

    const tx = await db.begin("read_write");
    const { rows } = await tx.execute(
      "MATCH (t:Trip {key: 't'}) RETURN t.capacity - t.taken AS free",
    );
    if ((rows[0].free as number) > 0) {
      await tx.execute("MATCH (t:Trip {key: 't'}) SET t.taken = t.taken + 1");
      await tx.execute("CREATE (:Attendance {trip: 't'})");
    }
    // Uncommitted writes are visible inside the transaction only.
    const inside = await tx.execute("MATCH (t:Trip) RETURN t.taken AS taken");
    expect(inside.rows[0].taken).toBe(2);
    const outside = await db.execute("MATCH (t:Trip) RETURN t.taken AS taken");
    expect(outside.rows[0].taken).toBe(1);

    await tx.commit();
    const after = await db.execute("MATCH (t:Trip) RETURN t.taken AS taken");
    expect(after.rows[0].taken).toBe(2);
    expect(tx.isOpen).toBe(false);
  });

  it("serializes a concurrent writer behind the open transaction", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:Counter {n: 0})");

    const tx = await db.begin();
    const { rows } = await tx.execute("MATCH (c:Counter) RETURN c.n AS n");
    // A write issued now must wait for the transaction to finish.
    const order: string[] = [];
    const concurrent = db
      .execute("MATCH (c:Counter) SET c.n = c.n + 100")
      .then(() => order.push("concurrent"));
    await new Promise((r) => setTimeout(r, 50));
    order.push("tx-write");
    await tx.execute("MATCH (c:Counter) SET c.n = $n", {
      n: (rows[0].n as number) + 1,
    });
    await tx.commit();
    await concurrent;

    expect(order).toEqual(["tx-write", "concurrent"]);
    const final = await db.execute("MATCH (c:Counter) RETURN c.n AS n");
    // No lost update: the concurrent write applied on top of the commit.
    expect(final.rows[0].n).toBe(101);
  });

  it("rolls back explicitly and on a failed statement", async () => {
    const db = await createDatabase();
    const tx = await db.begin();
    await tx.execute("CREATE (:Gone)");
    await tx.rollback();
    expect(
      (await db.execute("MATCH (g:Gone) RETURN count(g) AS c")).rows[0].c,
    ).toBe(0);

    const failing = await db.begin();
    await failing.execute("CREATE (:AlsoGone)");
    await expect(failing.execute("RETURN 1 +")).rejects.toBeInstanceOf(
      LoraError,
    );
    expect(failing.isOpen).toBe(false);
    await expect(failing.commit()).rejects.toMatchObject({
      code: "LORA_TRANSACTION",
    });
    expect(
      (await db.execute("MATCH (g:AlsoGone) RETURN count(g) AS c")).rows[0].c,
    ).toBe(0);
    // The writer lock was released: writes proceed.
    await db.execute("CREATE (:After)");
  });

  it("rolls back an unfinished transaction on async dispose", async () => {
    const db = await createDatabase();
    {
      await using tx = await db.begin();
      await tx.execute("CREATE (:Scoped)");
    }
    expect(
      (await db.execute("MATCH (s:Scoped) RETURN count(s) AS c")).rows[0].c,
    ).toBe(0);
    await db.execute("CREATE (:After)");
  });

  it("rolls back an open transaction when the database is disposed", async () => {
    const dir = await mkdtemp(join(tmpdir(), "lora-itx-"));
    try {
      const db = await createDatabase("app", { databaseDir: dir });
      const tx = await db.begin();
      await tx.execute("CREATE (:Uncommitted)");
      db.dispose();

      const reopened = await createDatabase("app", { databaseDir: dir });
      const { rows } = await reopened.execute(
        "MATCH (u:Uncommitted) RETURN count(u) AS c",
      );
      expect(rows[0].c).toBe(0);
      reopened.dispose();
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });

  it("read_only transactions reject writes", async () => {
    const db = await createDatabase();
    const tx = await db.begin("read_only");
    await expect(tx.execute("CREATE (:X)")).rejects.toBeInstanceOf(LoraError);
  });
});

describe("schema commands in transaction()", () => {
  const constraint =
    "CREATE CONSTRAINT festival_key_unique IF NOT EXISTS FOR (n:Festival) REQUIRE n.key IS UNIQUE";

  it("commits DDL and data together", async () => {
    const db = await createDatabase();
    await db.transaction([
      { query: constraint },
      { query: "CREATE (:__Migration {version: 1})" },
    ]);
    expect((await db.execute("SHOW CONSTRAINTS")).rows).toHaveLength(1);
    expect(
      (await db.execute("MATCH (m:__Migration) RETURN count(m) AS c")).rows[0]
        .c,
    ).toBe(1);
  });

  it("leaves neither behind when the last statement fails", async () => {
    const db = await createDatabase();
    await expect(
      db.transaction([
        { query: constraint },
        { query: "CREATE (:__Migration {version: 1})" },
        { query: "RETURN 1 +" },
      ]),
    ).rejects.toBeInstanceOf(LoraError);
    expect((await db.execute("SHOW CONSTRAINTS")).rows).toHaveLength(0);
    expect(
      (await db.execute("MATCH (m:__Migration) RETURN count(m) AS c")).rows[0]
        .c,
    ).toBe(0);
  });
});
