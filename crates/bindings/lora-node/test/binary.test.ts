import { randomBytes } from "node:crypto";
import { describe, expect, it } from "vitest";
import {
  binary,
  createDatabase,
  type LoraBinary,
  type LoraParam,
} from "../ts/index.js";

const bytesOf = (v: unknown): number[] => (v as LoraBinary).segments.flat();

describe("binary params", () => {
  it.each<[string, LoraParam]>([
    ["Uint8Array", new Uint8Array([1, 2, 3])],
    ["Buffer", Buffer.from([1, 2, 3])],
    ["ArrayBuffer", new Uint8Array([1, 2, 3]).buffer],
    ["binary()", binary([[1, 2, 3]])],
  ])("stores a %s as a binary value", async (_, v) => {
    const db = await createDatabase();
    const { rows } = await db.execute("CREATE (n:B {v: $v}) RETURN n.v AS v", {
      v,
    });
    expect(rows[0].v).toMatchObject({ kind: "binary", length: 3 });
    expect(bytesOf(rows[0].v)).toEqual([1, 2, 3]);
  });

  it("stores only the bytes a view covers", async () => {
    const db = await createDatabase();
    const whole = Buffer.from([9, 1, 2, 3, 9]);
    const { rows } = await db.execute("RETURN $v AS v", {
      v: whole.subarray(1, 4),
    });
    expect(bytesOf(rows[0].v)).toEqual([1, 2, 3]);
  });

  it("stores bytes nested in lists and maps, next to a bigint", async () => {
    const db = await createDatabase();
    const { rows } = await db.execute("RETURN $v AS v", {
      v: { body: [Buffer.from([0, 255])], id: 2n ** 60n },
    });
    const v = rows[0].v as { body: unknown[]; id: bigint };
    expect(bytesOf(v.body[0])).toEqual([0, 255]);
    expect(v.id).toBe(2n ** 60n);
  });

  it("stores empty bytes", async () => {
    const db = await createDatabase();
    const { rows } = await db.execute("RETURN $v AS v", {
      v: new Uint8Array(0),
    });
    expect(rows[0].v).toMatchObject({ kind: "binary", length: 0 });
  });

  it("round-trips a 1 MB Buffer byte for byte", async () => {
    const db = await createDatabase();
    const body = randomBytes(1024 * 1024 + 1);
    await db.execute("CREATE (:B {key: 'k', v: $v})", { v: body });
    const { rows } = await db.execute("MATCH (n:B {key: 'k'}) RETURN n.v AS v");
    expect(Buffer.from(bytesOf(rows[0].v)).equals(body)).toBe(true);
  });

  it("stores bytes in a transaction batch", async () => {
    const db = await createDatabase();
    await db.transaction([
      { query: "CREATE (:B {v: $v})", params: { v: Buffer.from([4, 5]) } },
    ]);
    const { rows } = await db.execute("MATCH (n:B) RETURN n.v AS v");
    expect(bytesOf(rows[0].v)).toEqual([4, 5]);
  });

  it("refuses other typed arrays", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("RETURN $v AS v", {
        v: new Float32Array([1]) as unknown as LoraParam,
      }),
    ).rejects.toMatchObject({ code: "LORA_INVALID_PARAMS" });
  });

  it("refuses malformed base64", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("RETURN $v AS v", {
        v: { kind: "binary", base64: "AQ=x" } as unknown as LoraParam,
      }),
    ).rejects.toMatchObject({ code: "LORA_INVALID_PARAMS" });
  });
});
