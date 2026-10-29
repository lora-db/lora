import { describe, expect, it } from "vitest";
import { createDatabase } from "../ts/index.js";

// `execute` sends params JSON can carry exactly as JSON text, parsed on the
// worker, and every other params object through napi as before (G-76). A
// `Date` beside a value forces the napi path, so each value can be compared
// across the two.
const PLAIN: [string, unknown][] = [
  ["integer", 42],
  ["negative zero", -0],
  ["float", 1.5],
  ["large float", 1e21],
  ["max safe integer", Number.MAX_SAFE_INTEGER],
  ["string", "plain"],
  ["unicode", "Grüße, 日本, 🎪"],
  ["escapes", 'quote " backslash \\ newline \n tab \t'],
  ["empty string", ""],
  ["boolean", true],
  ["null", null],
  ["list", [1, "two", [3.5, null], { four: 4 }]],
  ["map", { a: 1, nested: { b: ["x", { c: false }] } }],
  ["bigint", 2n ** 60n],
  ["bytes", new Uint8Array([0, 1, 254, 255])],
  [
    "rows",
    Array.from({ length: 50 }, (_, i) => ({
      key: `k${i}`,
      n: i,
      ok: i % 2 === 0,
    })),
  ],
];

describe("execute params", () => {
  it.each(PLAIN)(
    "round-trip %s the same as JSON and through napi",
    async (_, value) => {
      const db = await createDatabase();
      const json = await db.execute("RETURN $v AS v", { v: value });
      const napi = await db.execute("RETURN $v AS v", {
        v: value,
        forced: new Date(0),
      });
      expect(json.rows).toEqual(napi.rows);
    },
  );

  it("keep what only napi carries", async () => {
    const db = await createDatabase();
    // A lone surrogate is no JSON text; napi reads it as U+FFFD.
    const lone = await db.execute("RETURN $v AS v", { v: "a\ud800b" });
    expect(lone.rows[0].v).toBe("a�b");
    // `undefined` and NaN reach the engine as napi gives them, not as
    // JSON.stringify would (dropped, or null).
    const forced = { forced: new Date(0) };
    for (const v of [undefined, Number.NaN]) {
      const got = await db.execute("RETURN $v AS v", { v }).then(
        (r) => r.rows,
        (e: Error) => e.message,
      );
      const want = await db.execute("RETURN $v AS v", { v, ...forced }).then(
        (r) => r.rows,
        (e: Error) => e.message,
      );
      expect(got).toEqual(want);
    }
  });

  it("refuse params that are not an object either way", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("RETURN 1", "nope" as unknown as Record<string, unknown>),
    ).rejects.toThrow(/params must be an object/);
  });

  it("write an UNWIND batch sent as JSON", async () => {
    const db = await createDatabase();
    await db.execute("CREATE CONSTRAINT k FOR (n:N) REQUIRE n.key IS UNIQUE");
    const rows = Array.from({ length: 2000 }, (_, i) => ({
      key: `n${i}`,
      v: i,
    }));
    await db.execute(
      "UNWIND $rows AS row CREATE (:N {key: row.key, v: row.v})",
      { rows },
    );
    await db.execute(
      "UNWIND $rows AS row MATCH (n:N {key: row.key}) SET n.w = row.v * 2, n.s = row.key",
      { rows },
    );
    const { rows: out } = await db.execute(
      "MATCH (n:N) RETURN sum(n.w) AS w, count(n.s) AS s",
    );
    expect(out[0]).toEqual({ w: 2 * ((1999 * 2000) / 2), s: 2000 });
  });
});
