import { describe, expect, it } from "vitest";
import { createDatabase, LoraError } from "../ts/index.js";

describe("Transaction.executeMany", () => {
  it("runs statements in order and returns every result", async () => {
    const db = await createDatabase();
    const tx = await db.begin();
    const results = await tx.executeMany([
      { query: "CREATE (:Item {k: $k})", params: { k: 1 } },
      { query: "CREATE (:Item {k: $k})", params: { k: 2 } },
      // Sees the writes before it in the same batch.
      { query: "MATCH (i:Item) RETURN i.k AS k ORDER BY k" },
      { query: "RETURN $big AS big", params: { big: 2n ** 60n } },
    ]);
    expect(results).toHaveLength(4);
    expect(results[0].rows).toEqual([]);
    expect(results[2].columns).toEqual(["k"]);
    expect(results[2].rows).toEqual([{ k: 1 }, { k: 2 }]);
    expect(results[3].rows[0].big).toBe(2n ** 60n);

    // The transaction stays open for more work.
    expect(tx.isOpen).toBe(true);
    await tx.execute("CREATE (:Item {k: 3})");
    await tx.commit();
    const { rows } = await db.execute("MATCH (i:Item) RETURN count(i) AS c");
    expect(rows[0].c).toBe(3);
  });

  it("an empty batch returns no results", async () => {
    const db = await createDatabase();
    const tx = await db.begin();
    expect(await tx.executeMany([])).toEqual([]);
    await tx.rollback();
  });

  it("stops at the first failing statement and rolls back", async () => {
    const db = await createDatabase();
    const tx = await db.begin();
    await tx.execute("CREATE (:Before)");
    const failure = tx.executeMany([
      { query: "CREATE (:InBatch)" },
      { query: "RETURN 1 +" },
      { query: "CREATE (:NeverRuns)" },
    ]);
    await expect(failure).rejects.toBeInstanceOf(LoraError);
    await expect(failure).rejects.toMatchObject({
      message: expect.stringContaining("(statement 2 of 3)"),
    });
    expect(tx.isOpen).toBe(false);
    await expect(tx.commit()).rejects.toMatchObject({
      code: "LORA_TRANSACTION",
    });
    const { rows } = await db.execute(
      "MATCH (n) WHERE n:Before OR n:InBatch OR n:NeverRuns RETURN count(n) AS c",
    );
    expect(rows[0].c).toBe(0);
    // The writer lock was released.
    await db.execute("CREATE (:After)");
  });

  it("rejects invalid statements and releases the transaction", async () => {
    const db = await createDatabase();
    const tx = await db.begin();
    await expect(
      tx.executeMany([{ nope: true } as unknown as { query: string }]),
    ).rejects.toMatchObject({ code: "LORA_INVALID_PARAMS" });
    expect(tx.isOpen).toBe(false);
    await db.execute("CREATE (:After)");
  });

  it("one timeout bounds the whole batch", async () => {
    const db = await createDatabase();
    await db.execute("UNWIND range(1, 20000) AS i CREATE (:A {i: i})");
    const tx = await db.begin();
    await expect(
      tx.executeMany(
        [
          { query: "CREATE (:Partial)" },
          { query: "MATCH (a:A), (b:A) WHERE a.i + b.i = -1 RETURN a" },
        ],
        { timeoutMs: 50 },
      ),
    ).rejects.toMatchObject({ code: "LORA_TIMEOUT" });
    expect(tx.isOpen).toBe(false);
    const { rows } = await db.execute("MATCH (p:Partial) RETURN count(p) AS c");
    expect(rows[0].c).toBe(0);
  });

  it("works in a read_only transaction", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:R {v: 1})");
    const tx = await db.begin("read_only");
    const [a, b] = await tx.executeMany([
      { query: "MATCH (r:R) RETURN r.v AS v" },
      { query: "RETURN 2 AS v" },
    ]);
    expect(a.rows).toEqual([{ v: 1 }]);
    expect(b.rows).toEqual([{ v: 2 }]);
    await tx.rollback();
  });

  it("costs one round trip instead of one per statement", async () => {
    const db = await createDatabase();
    const n = 200;
    const statements = Array.from({ length: n }, (_, i) => ({
      query: "CREATE (:P {i: $i})",
      params: { i },
    }));

    const time = async (
      run: (tx: Awaited<ReturnType<typeof db.begin>>) => Promise<void>,
    ) => {
      const tx = await db.begin();
      const start = performance.now();
      await run(tx);
      const took = performance.now() - start;
      await tx.rollback();
      return took;
    };
    // Warm up both paths.
    await time(async (tx) => {
      await tx.executeMany(statements.slice(0, 10));
    });
    const sequential = await time(async (tx) => {
      for (const st of statements) await tx.execute(st.query, st.params);
    });
    const batched = await time(async (tx) => {
      await tx.executeMany(statements);
    });
    console.log(
      `${n} statements: execute() ${(sequential / n).toFixed(3)} ms each, ` +
        `executeMany ${(batched / n).toFixed(3)} ms each`,
    );
    expect(batched).toBeLessThan(sequential);
  });
});
