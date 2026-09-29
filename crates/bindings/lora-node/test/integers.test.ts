import { describe, expect, it } from "vitest";
import { createDatabase, LoraError } from "../ts/index.js";

// Integers outside Number.MIN_SAFE_INTEGER..MAX_SAFE_INTEGER must
// round-trip exactly (as bigint) or fail loudly; never silently round.
const EDGES: bigint[] = [
  2n ** 53n + 1n,
  -(2n ** 53n + 1n),
  2n ** 63n - 1n,
  -(2n ** 63n - 1n),
  -(2n ** 63n),
];

describe("64-bit integers", () => {
  it("returns an unsafe integer literal as an exact bigint", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:N {key: 'n', v: 9007199254740993})");
    const { rows } = await db.execute("MATCH (n:N) RETURN n.v AS v");
    expect(rows[0].v).toBe(9007199254740993n);
  });

  it.each(EDGES.filter((v) => v !== -(2n ** 63n)))(
    "round-trips literal %s",
    async (v) => {
      const db = await createDatabase();
      const { rows } = await db.execute(`RETURN ${v.toString()} AS v`);
      expect(rows[0].v).toBe(v);
    },
  );

  it.each(EDGES)("round-trips bigint param %s through storage", async (v) => {
    const db = await createDatabase();
    await db.execute("CREATE (:N {v: $v})", { v });
    const { rows } = await db.execute("MATCH (n:N) RETURN n.v AS v, n AS node");
    expect(rows[0].v).toBe(v);
    expect((rows[0].node as { properties: { v: unknown } }).properties.v).toBe(
      v,
    );
  });

  it("round-trips bigints nested in lists and maps", async () => {
    const db = await createDatabase();
    const big = 2n ** 62n + 7n;
    const { rows } = await db.execute("RETURN $l AS l, $m AS m", {
      l: [1, big],
      m: { a: big, b: [big] },
    });
    expect(rows[0].l).toEqual([1, big]);
    expect(rows[0].m).toEqual({ a: big, b: [big] });
  });

  it("keeps safe integers as plain numbers", async () => {
    const db = await createDatabase();
    const { rows } = await db.execute("RETURN $a AS a, $b AS b, 42 AS c", {
      a: Number.MAX_SAFE_INTEGER,
      b: 5n,
    });
    expect(rows[0].a).toBe(Number.MAX_SAFE_INTEGER);
    expect(rows[0].b).toBe(5);
    expect(rows[0].c).toBe(42);
  });

  it("returns exact bigints from stream()", async () => {
    const db = await createDatabase();
    const v = 2n ** 63n - 1n;
    await db.execute("CREATE (:N {v: $v})", { v });
    const seen: unknown[] = [];
    for await (const row of db.stream("MATCH (n:N) RETURN n.v AS v")) {
      seen.push(row.v);
    }
    expect(seen).toEqual([v]);
  });

  it("rejects an unsafe integer passed as a number instead of rounding", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("CREATE (:N {v: $v})", { v: 2 ** 60 }),
    ).rejects.toMatchObject({ code: "LORA_INVALID_PARAMS" });
    const { rows } = await db.execute("MATCH (n:N) RETURN count(n) AS c");
    expect(rows[0].c).toBe(0);
  });

  it("rejects a bigint that does not fit in 64 bits", async () => {
    const db = await createDatabase();
    const err = await db
      .execute("RETURN $v AS v", { v: 2n ** 64n })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LoraError);
    expect((err as LoraError).code).toBe("LORA_INVALID_PARAMS");
  });

  it("accepts bigint params inside transaction()", async () => {
    const db = await createDatabase();
    const v = 2n ** 53n + 1n;
    const [, read] = await db.transaction([
      { query: "CREATE (:N {v: $v})", params: { v } },
      { query: "MATCH (n:N) RETURN n.v AS v" },
    ]);
    expect(read.rows[0].v).toBe(v);
  });
});
