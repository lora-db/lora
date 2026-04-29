import { describe, it, expect, beforeEach, vi } from "vitest";
import { Buffer } from "node:buffer";
import {
  createDatabase,
  type Database,
  LoraError,
  isNode,
  isRelationship,
  isPath,
  isPoint,
  isTemporal,
  date,
  duration,
  cartesian,
  cartesian3d,
  wgs84,
  wgs84_3d,
  type LoraNode,
  type LoraRelationship,
  type LoraValue,
} from "../ts/index.js";

describe("Database — basics", () => {
  let db: Database;

  beforeEach(async () => {
    db = await createDatabase();
  });

  it("returns typed empty result for empty graph MATCH", async () => {
    const result = await db.execute("MATCH (n) RETURN n");
    expect(result.rows).toEqual([]);
    expect(result.columns).toEqual([]);
  });

  it("creates and returns a node with typed properties", async () => {
    await db.execute("CREATE (:Person {name: 'Alice', age: 30})");
    expect(await db.nodeCount()).toBe(1);

    const result = await db.execute<{ n: LoraNode }>(
      "MATCH (n:Person) RETURN n",
    );
    expect(result.rows).toHaveLength(1);

    const n = result.rows[0]!.n;
    expect(isNode(n)).toBe(true);
    if (!isNode(n)) throw new Error("expected node");
    expect(n.labels).toEqual(["Person"]);
    expect(n.properties.name).toBe("Alice");
    expect(n.properties.age).toBe(30);
  });

  it("accepts typed params — string / number / boolean", async () => {
    await db.execute("CREATE (:Item {name: $n, qty: $q, active: $a})", {
      n: "widget",
      q: 42,
      a: true,
    });
    const result = await db.execute(
      "MATCH (i:Item) RETURN i.name AS name, i.qty AS qty, i.active AS active",
    );
    expect(result.rows).toEqual([{ name: "widget", qty: 42, active: true }]);
  });

  it("returns typed relationship with discriminator", async () => {
    await db.execute("CREATE (:A {n:1})-[:R {w:2}]->(:B {n:3})");
    const result = await db.execute<{ r: LoraRelationship }>(
      "MATCH ()-[r:R]->() RETURN r",
    );
    const r = result.rows[0]!.r;
    expect(isRelationship(r)).toBe(true);
    if (!isRelationship(r)) throw new Error("expected relationship");
    expect(r.type).toBe("R");
    expect(r.properties.w).toBe(2);
  });

  it("clear() empties the graph", async () => {
    await db.execute("CREATE (:X), (:Y)-[:R]->(:Z)");
    expect(await db.nodeCount()).toBe(3);
    expect(await db.relationshipCount()).toBe(1);
    await db.clear();
    expect(await db.nodeCount()).toBe(0);
    expect(await db.relationshipCount()).toBe(0);
  });

  it("warns and falls back when default worker startup fails", async () => {
    const originalWorker = (globalThis as { Worker?: unknown }).Worker;
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    Object.defineProperty(globalThis, "Worker", {
      configurable: true,
      value: class {
        constructor() {
          throw new Error("worker disabled");
        }
      },
    });

    try {
      const fallback = await createDatabase();
      expect(await fallback.nodeCount()).toBe(0);
      expect(warn).toHaveBeenCalledWith(
        expect.stringContaining("falling back to main-thread WASM"),
      );
      await fallback.dispose();
    } finally {
      warn.mockRestore();
      if (originalWorker === undefined) {
        Reflect.deleteProperty(globalThis, "Worker");
      } else {
        Object.defineProperty(globalThis, "Worker", {
          configurable: true,
          value: originalWorker,
        });
      }
    }
  });

  it("saves and loads snapshots as browser-native binary objects", async () => {
    const source = await createDatabase();
    await source.execute("CREATE (:Snapshot {name: 'Ada'})");
    const bytes = await source.saveSnapshot();
    const arrayBuffer = await source.saveSnapshot({ format: "arrayBuffer" });
    const blob = await source.saveSnapshot({ format: "blob" });
    const response = await source.saveSnapshot({ format: "response" });
    const stream = await source.saveSnapshot({ format: "stream" });
    const dataUrl = new URL(
      `data:application/octet-stream;base64,${Buffer.from(bytes).toString("base64")}`,
    );
    let objectUrl: URL | null = null;
    if (typeof URL.createObjectURL === "function") {
      objectUrl = await source.saveSnapshot({ format: "url" });
    }
    expect(bytes).toBeInstanceOf(Uint8Array);
    source.dispose();

    const inputs = [
      bytes,
      arrayBuffer,
      blob,
      response,
      stream,
      new Blob([bytes]),
      new Response(bytes),
      new ReadableStream<Uint8Array>({
        start(controller) {
          controller.enqueue(bytes);
          controller.close();
        },
      }),
      dataUrl,
      ...(objectUrl ? [objectUrl] : []),
    ];

    try {
      for (const input of inputs) {
        const db = await createDatabase();
        const meta = await db.loadSnapshot(input);
        expect(meta.nodeCount).toBe(1);
        const { rows } = await db.execute<{ name: string }>(
          "MATCH (n:Snapshot) RETURN n.name AS name",
        );
        expect(rows).toEqual([{ name: "Ada" }]);
        await db.dispose();
      }
    } finally {
      if (objectUrl) {
        URL.revokeObjectURL(objectUrl.href);
      }
    }
  });

  it("saves and loads gzip-compressed snapshots", async () => {
    const source = await createDatabase({ runtime: "main-thread" });
    const repeated = "compress-me-".repeat(128);
    for (let i = 0; i < 32; i += 1) {
      await source.execute("CREATE (:Compressed {i: $i, repeated: $repeated})", {
        i,
        repeated,
      });
    }

    const plain = await source.saveSnapshot({ compression: "none" });
    const compressed = await source.saveSnapshot({
      compression: { format: "gzip", level: 1 },
    });
    expect(compressed.byteLength).toBeLessThan(plain.byteLength);

    const target = await createDatabase({ runtime: "main-thread" });
    const meta = await target.loadSnapshot(compressed);
    expect(meta.nodeCount).toBe(32);
    const { rows } = await target.execute<{ count: number }>(
      "MATCH (n:Compressed) RETURN count(n) AS count",
    );
    expect(rows).toEqual([{ count: 32 }]);

    await source.dispose();
    await target.dispose();
  });

  it("saves and loads encrypted snapshots", async () => {
    const encryption = {
      type: "password" as const,
      keyId: "wasm-test",
      password: "open sesame",
      params: { memoryCostKib: 512, timeCost: 1, parallelism: 1 },
    };
    const source = await createDatabase({ runtime: "main-thread" });
    await source.execute("CREATE (:Secret {name: 'Ada'})");

    const bytes = await source.saveSnapshot({
      compression: { format: "gzip" as const, level: 1 },
      encryption,
    });

    const target = await createDatabase({ runtime: "main-thread" });
    await expect(target.loadSnapshot(bytes)).rejects.toThrow(/password encrypted/);
    const meta = await target.loadSnapshot(bytes, { credentials: encryption });
    expect(meta.nodeCount).toBe(1);
    const { rows } = await target.execute<{ name: string }>(
      "MATCH (n:Secret) RETURN n.name AS name",
    );
    expect(rows).toEqual([{ name: "Ada" }]);

    await source.dispose();
    await target.dispose();
  });
});

describe("Database — value model", () => {
  it("roundtrips a list of mixed-scalar values", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:N {xs: $xs})", { xs: [1, "two", true, null] });
    const { rows } = await db.execute("MATCH (n:N) RETURN n.xs AS xs");
    expect(rows[0]!.xs).toEqual([1, "two", true, null]);
  });

  it("roundtrips a nested map", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:N {meta: $m})", {
      m: { a: 1, b: { c: "deep", d: [true, false] } },
    });
    const { rows } = await db.execute("MATCH (n:N) RETURN n.meta AS m");
    expect(rows[0]!.m).toEqual({ a: 1, b: { c: "deep", d: [true, false] } });
  });

  it("returns tagged date values from stored properties", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:E {d: date('2025-03-14')})");
    const { rows } = await db.execute("MATCH (n:E) RETURN n.d AS d");
    const d = rows[0]!.d;
    expect(isTemporal(d)).toBe(true);
    expect(d).toEqual({ kind: "date", iso: "2025-03-14" });
  });

  it("accepts typed date + duration params", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:E {on: $d, span: $dur})", {
      d: date("2025-01-15"),
      dur: duration("P1M"),
    });
    const { rows } = await db.execute(
      "MATCH (n:E) RETURN n.on AS on, n.span AS span",
    );
    expect(rows[0]!.on).toEqual({ kind: "date", iso: "2025-01-15" });
    expect(rows[0]!.span).toEqual({ kind: "duration", iso: "P1M" });
  });

  it("returns tagged point values (cartesian + wgs84)", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:P {c: $c, g: $g})", {
      c: cartesian(1.5, 2.5),
      g: wgs84(4.9, 52.37),
    });
    const { rows } = await db.execute("MATCH (n:P) RETURN n.c AS c, n.g AS g");
    const c = rows[0]!.c;
    const g = rows[0]!.g;
    expect(isPoint(c)).toBe(true);
    expect(isPoint(g)).toBe(true);
    if (!isPoint(c) || !isPoint(g)) throw new Error("expected points");
    expect(c.srid).toBe(7203);
    expect(c.crs).toBe("cartesian");
    expect(c.x).toBeCloseTo(1.5, 10);
    expect(c.y).toBeCloseTo(2.5, 10);
    expect((c as { z?: number }).z).toBeUndefined();
    expect(g.srid).toBe(4326);
    expect(g.crs).toBe("WGS-84-2D");
    if (g.srid !== 4326) throw new Error("expected WGS-84-2D narrowing");
    expect(g.longitude).toBeCloseTo(4.9, 10);
    expect(g.latitude).toBeCloseTo(52.37, 10);
  });

  it("returns 3D cartesian points with z", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:P3 {p: $p})", { p: cartesian3d(1.0, 2.0, 3.0) });
    const { rows } = await db.execute("MATCH (n:P3) RETURN n.p AS p");
    const p = rows[0]!.p;
    if (!isPoint(p)) throw new Error("expected point");
    expect(p.srid).toBe(9157);
    expect(p.crs).toBe("cartesian-3D");
    if (p.srid !== 9157) throw new Error("expected cartesian-3D narrowing");
    expect(p.z).toBeCloseTo(3.0, 10);
  });

  it("returns 3D WGS-84 points with height + geographic aliases", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:G3 {p: $p})", {
      p: wgs84_3d(4.89, 52.37, 15.0),
    });
    const { rows } = await db.execute("MATCH (n:G3) RETURN n.p AS p");
    const p = rows[0]!.p;
    if (!isPoint(p)) throw new Error("expected point");
    expect(p.srid).toBe(4979);
    expect(p.crs).toBe("WGS-84-3D");
    if (p.srid !== 4979) throw new Error("expected WGS-84-3D narrowing");
    expect(p.longitude).toBeCloseTo(4.89, 10);
    expect(p.latitude).toBeCloseTo(52.37, 10);
    expect(p.height).toBeCloseTo(15.0, 10);
    expect(p.z).toBeCloseTo(15.0, 10);
  });

  it("3D point constructed via point() Cypher round-trips unchanged", async () => {
    const db = await createDatabase();
    const { rows } = await db.execute(
      "RETURN point({x: 1.0, y: 2.0, z: 3.0}) AS p",
    );
    const p = rows[0]!.p;
    if (!isPoint(p)) throw new Error("expected point");
    if (p.srid !== 9157) throw new Error("expected cartesian-3D");
    expect(p).toEqual({
      kind: "point",
      srid: 9157,
      crs: "cartesian-3D",
      x: 1.0,
      y: 2.0,
      z: 3.0,
    });
  });

  it("returns a path with nodes/rels invariant", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:A {n:1})-[:R]->(:B {n:2})");
    const { rows } = await db.execute("MATCH p = (:A)-[:R]->(:B) RETURN p");
    const p = rows[0]!.p as LoraValue;
    expect(isPath(p)).toBe(true);
    if (!isPath(p)) throw new Error("expected path");
    expect(p.nodes.length).toBe(p.rels.length + 1);
  });
});

describe("Database — temporal now() in wasm", () => {
  // Regression: the wasm32 target previously panicked on any call that hit
  // `std::time::SystemTime::now()`. `lora-store::temporal::unix_now()`
  // now routes through `js_sys::Date::now()` on wasm32.

  it("evaluates date() / datetime() / time() no-arg forms", async () => {
    const db = await createDatabase();
    const { rows } = await db.execute(
      "RETURN date() AS d, datetime() AS dt, time() AS t, localdatetime() AS ldt, localtime() AS lt",
    );
    const row = rows[0]!;
    expect(isTemporal(row.d as LoraValue)).toBe(true);
    expect(isTemporal(row.dt as LoraValue)).toBe(true);
    expect(isTemporal(row.t as LoraValue)).toBe(true);
    expect(isTemporal(row.ldt as LoraValue)).toBe(true);
    expect(isTemporal(row.lt as LoraValue)).toBe(true);

    // Sanity: year component is at least 2024 — ensures we actually read the
    // wall clock rather than returning epoch zero.
    const iso = (row.d as { iso: string }).iso;
    const year = parseInt(iso.slice(0, 4), 10);
    expect(year).toBeGreaterThanOrEqual(2024);
  });
});

describe("Database — errors", () => {
  it("throws LoraError for a parse error", async () => {
    const db = await createDatabase();
    await expect(db.execute("THIS IS NOT CYPHER")).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "LORA_ERROR",
    );
  });

  it("throws INVALID_PARAMS for a malformed temporal param", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("RETURN $d AS d", { d: { kind: "date", iso: "not-a-date" } }),
    ).rejects.toSatisfy((e) => e instanceof LoraError && e.code === "INVALID_PARAMS");
  });
});

describe("Database — vector values (wasm)", () => {
  it("returns a vector value and accepts one as a parameter", async () => {
    const db = await createDatabase();
    const { vector, isVector } = await import("../ts/types.js");
    const { rows } = await db.execute<{ v: LoraValue }>(
      "RETURN vector([1,2,3], 3, FLOAT32) AS v",
    );
    const v = rows[0]!.v;
    expect(isVector(v)).toBe(true);
    if (!isVector(v)) throw new Error("expected vector");
    expect(v.dimension).toBe(3);
    expect(v.coordinateType).toBe("FLOAT32");

    const param = vector([0.1, 0.2, 0.3], 3, "FLOAT32");
    const { rows: rows2 } = await db.execute<{ v: LoraValue }>(
      "RETURN $v AS v",
      { v: param },
    );
    const v2 = rows2[0]!.v;
    expect(isVector(v2)).toBe(true);
  });

  it("uses a vector() parameter inside vector.similarity.cosine (wasm)", async () => {
    const db = await createDatabase();
    const { vector } = await import("../ts/types.js");
    const query = vector([1.0, 0.0, 0.0], 3, "FLOAT32");
    const { rows } = await db.execute<{ s: number }>(
      "RETURN vector.similarity.cosine(vector([1.0, 0.0, 0.0], 3, FLOAT32), $q) AS s",
      { q: query },
    );
    expect(Math.abs((rows[0]!.s as number) - 1.0)).toBeLessThan(1e-6);
  });

  it("stores a vector parameter as a node property (wasm)", async () => {
    const db = await createDatabase();
    const { vector, isVector } = await import("../ts/types.js");
    const param = vector([1, 2, 3], 3, "INTEGER8");
    await db.execute("CREATE (:Doc {id: 1, embedding: $e})", { e: param });
    const { rows } = await db.execute<{ e: LoraValue }>(
      "MATCH (d:Doc) RETURN d.embedding AS e",
    );
    const stored = rows[0]!.e;
    expect(isVector(stored)).toBe(true);
    if (!isVector(stored)) throw new Error("expected vector");
    expect(stored.coordinateType).toBe("INTEGER8");
    expect(stored.values).toEqual([1, 2, 3]);
  });

  it("rejects a malformed vector parameter (wasm)", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("RETURN $v AS v", {
        v: {
          kind: "vector",
          dimension: 2,
          coordinateType: "FLOAT32",
          values: [1.0, "oops"],
        },
      }),
    ).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "INVALID_PARAMS",
    );
  });

  it("streams rows with async iteration and toArray (wasm)", async () => {
    const db = await createDatabase();
    await db.execute("UNWIND range(1, 3) AS i CREATE (:S {i: i})");

    const seen: number[] = [];
    for await (const row of db.stream<{ i: number }>(
      "MATCH (n:S) RETURN n.i AS i ORDER BY i",
    )) {
      seen.push(row.i);
    }
    expect(seen).toEqual([1, 2, 3]);

    const rows = await db
      .rows<{ i: number }>("MATCH (n:S) RETURN n.i AS i ORDER BY i")
      .toArray();
    expect(rows.map((row) => row.i)).toEqual([1, 2, 3]);
  });

  it("rolls back a mutating stream when closed before exhaustion (wasm)", async () => {
    const db = await createDatabase();
    const stream = db.stream<{ i: number }>(
      "UNWIND range(1, 3) AS i CREATE (:EarlyClose {i: i}) RETURN i",
    );
    expect((await stream.next()).value?.i).toBe(1);
    stream.close();

    expect(await db.nodeCount()).toBe(0);
  });

  it("executes statement batches inside one transaction (wasm)", async () => {
    const db = await createDatabase();
    const results = await db.transaction<{ v: number }>([
      { query: "CREATE (:Tx {id: $id})", params: { id: 1 } },
      { query: "MATCH (n:Tx) RETURN n.id AS v" },
    ]);
    expect(results[1]!.rows[0]!.v).toBe(1);

    await expect(
      db.transaction([
        { query: "CREATE (:Tx {id: 2})" },
        { query: "THIS IS NOT CYPHER" },
      ]),
    ).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "LORA_ERROR",
    );
    const after = await db.execute<{ v: number }>(
      "MATCH (n:Tx) RETURN n.id AS v ORDER BY v",
    );
    expect(after.rows.map((row) => row.v)).toEqual([1]);
  });

  it("isVector returns false for non-vector values (wasm)", async () => {
    const { isVector } = await import("../ts/types.js");
    expect(isVector(null)).toBe(false);
    expect(isVector([1, 2, 3])).toBe(false);
    expect(isVector({})).toBe(false);
    expect(isVector({ kind: "node", id: 1 })).toBe(false);
    expect(isVector(42)).toBe(false);
    expect(isVector("vector")).toBe(false);
  });
});
