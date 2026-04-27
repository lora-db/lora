import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pathToFileURL } from "node:url";

import { afterEach, beforeEach, describe, expect, it } from "vitest";
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

const cleanupPaths = new Set<string>();

async function makeTempDir(prefix = "lora-node-wal-"): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), prefix));
  cleanupPaths.add(dir);
  return dir;
}

afterEach(async () => {
  await Promise.all(
    Array.from(cleanupPaths, (path) =>
      rm(path, { recursive: true, force: true }),
    ),
  );
  cleanupPaths.clear();
});

describe("Database — basics", () => {
  let db: Database;

  beforeEach(async () => {
    db = await createDatabase();
  });

  it("returns typed empty result for empty graph MATCH", async () => {
    const result = await db.execute("MATCH (n) RETURN n");
    // With RowArrays format, columns are inferred from the first row, so an
    // empty result set produces an empty column list. Callers must read
    // columns off the first non-empty result or from a `RETURN` they know.
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
    const result = await db.execute("MATCH (i:Item) RETURN i.name AS name, i.qty AS qty, i.active AS active");
    expect(result.rows).toEqual([
      { name: "widget", qty: 42, active: true },
    ]);
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
});

describe("Database — WAL-backed initialization", () => {
  it("createDatabase() still creates an empty in-memory database", async () => {
    const db = await createDatabase();
    expect(await db.nodeCount()).toBe(0);
    expect(await db.relationshipCount()).toBe(0);
  });

  it("persists committed writes across reopen with the same WAL directory", async () => {
    const walDir = await makeTempDir();

    const first = await createDatabase("app", { databaseDir: walDir });
    await first.execute(
      "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {name: 'Grace'})",
    );
    first.dispose();

    const second = await createDatabase("app", { databaseDir: walDir });
    expect(await second.nodeCount()).toBe(2);
    expect(await second.relationshipCount()).toBe(1);

    const { rows } = await second.execute<{ name: string }>(
      "MATCH (p:Person) RETURN p.name AS name ORDER BY name",
    );
    expect(rows).toEqual([{ name: "Ada" }, { name: "Grace" }]);
    second.dispose();
  });

  it("restores existing data on a read-only reopen", async () => {
    const walDir = await makeTempDir();

    const writer = await createDatabase("app", { databaseDir: walDir });
    await writer.execute(
      "CREATE (:User {id: 1})-[:FOLLOWS]->(:User {id: 2}) RETURN 1",
    );
    writer.dispose();

    const reader = await createDatabase("app", { databaseDir: walDir });
    expect(await reader.nodeCount()).toBe(2);
    expect(await reader.relationshipCount()).toBe(1);

    const result = await reader.execute<{ id: number }>(
      "MATCH (u:User) RETURN u.id AS id ORDER BY id",
    );
    expect(result.rows).toEqual([{ id: 1 }, { id: 2 }]);
    reader.dispose();
  });

  it("accepts a relative WAL directory path", async () => {
    const relativeWalDir = `.tmp-lora-node-wal-${process.pid}-${Date.now()}`;
    cleanupPaths.add(resolve(relativeWalDir));

    const first = await createDatabase("app", { databaseDir: relativeWalDir });
    await first.execute("CREATE (:Session {value: 'ok'})");
    first.dispose();

    const second = await createDatabase("app", { databaseDir: relativeWalDir });
    const { rows } = await second.execute<{ value: string }>(
      "MATCH (s:Session) RETURN s.value AS value",
    );
    expect(rows).toEqual([{ value: "ok" }]);
    second.dispose();
  });

  it("normalizes WAL open failures through wrapError", async () => {
    const dir = await makeTempDir();
    const notADir = join(dir, "wal-file");
    await writeFile(notADir, "not a directory");

    await expect(createDatabase("app", { databaseDir: notADir })).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "LORA_ERROR",
    );
  });

  it("rejects invalid database names before creating storage", async () => {
    await expect(createDatabase("../bad")).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "LORA_ERROR",
    );
  });

  it("loads snapshots from paths, file URLs, buffers, array buffers, and URLs", async () => {
    const dir = await makeTempDir("lora-node-snapshot-");
    const snapshotPath = join(dir, "graph.bin");

    const source = await createDatabase();
    await source.execute("CREATE (:Snapshot {name: 'Ada'})");
    await source.saveSnapshot(snapshotPath);
    const optionPath = join(dir, "graph-option.bin");
    const pathMeta = await source.saveSnapshot({
      format: "path",
      path: pathToFileURL(optionPath),
    });
    expect(pathMeta.nodeCount).toBe(1);
    const binary = await source.saveSnapshot("binary");
    expect(Buffer.isBuffer(binary)).toBe(true);
    const base64 = await source.saveSnapshot({ format: "base64" });
    source.dispose();

    const bytes = await readFile(snapshotPath);
    const arrayBuffer = bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength,
    );
    const dataUrl = new URL(
      `data:application/octet-stream;base64,${bytes.toString("base64")}`,
    );

    const fileUrl = pathToFileURL(snapshotPath);

    for (const input of [
      snapshotPath,
      fileUrl,
      fileUrl.toString(),
      bytes,
      arrayBuffer,
      binary,
      Buffer.from(base64, "base64"),
      Readable.from([bytes]),
      new ReadableStream({
        start(controller) {
          controller.enqueue(bytes);
          controller.close();
        },
      }),
      dataUrl,
      dataUrl.toString(),
    ]) {
      const db = await createDatabase();
      const meta = await db.loadSnapshot(input);
      expect(meta.nodeCount).toBe(1);
      const { rows } = await db.execute<{ name: string }>(
        "MATCH (n:Snapshot) RETURN n.name AS name",
      );
      expect(rows).toEqual([{ name: "Ada" }]);
      db.dispose();
    }
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
    expect(g.x).toBeCloseTo(4.9, 10);
    expect(g.y).toBeCloseTo(52.37, 10);
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

describe("Database — non-blocking event loop", () => {
  // Regression: `execute()` must dispatch to the libuv threadpool via
  // napi::Task so the JS event loop stays free. A blocking implementation
  // would park the main thread for the duration of the query, so the
  // `setImmediate` yielder below would stay at zero until the query
  // resolved. With the threadpool-backed implementation it ticks.

  it("lets setImmediate callbacks run while a query is in flight", async () => {
    const db = await createDatabase();

    // Seed 2 000 nodes sequentially so the graph state is well-defined
    // before we run the non-blocking probe. The point of this test is the
    // event-loop property of the binding, not the engine's handling of
    // concurrent writes.
    const N = 2_000;
    for (let i = 0; i < N; i++) {
      await db.execute("CREATE (:P {i: $i})", { i });
    }
    expect(await db.nodeCount()).toBe(N);

    let yields = 0;
    let stop = false;
    const yielder = (async () => {
      while (!stop) {
        await new Promise<void>((r) => setImmediate(r));
        yields++;
      }
    })();

    const result = await db.execute<{ i: number }>(
      "MATCH (n:P) RETURN n.i AS i ORDER BY i",
    );
    stop = true;
    await yielder;

    expect(result.rows).toHaveLength(N);
    expect(result.rows[0]!.i).toBe(0);
    expect(result.rows[N - 1]!.i).toBe(N - 1);

    // If the Rust engine blocked the event loop, yields would stay at 0.
    // Even on very fast machines we expect at least one tick while the
    // threadpool resolved the promise. The assertion is deliberately
    // permissive — we only care that the main thread *could* run.
    expect(yields).toBeGreaterThan(0);
  });

  it("runs many queries in parallel without deadlock", async () => {
    const db = await createDatabase();
    const results = await Promise.all(
      Array.from({ length: 50 }, (_, i) =>
        db.execute<{ v: number }>("RETURN $v AS v", { v: i }),
      ),
    );
    expect(results.map((r) => r.rows[0]!.v)).toEqual(
      Array.from({ length: 50 }, (_, i) => i),
    );
  });
});

describe("Database — path results", () => {
  it("returns a path object with node/rel id arrays", async () => {
    const db = await createDatabase();
    await db.execute("CREATE (:A {n:1})-[:R]->(:B {n:2})");
    const { rows } = await db.execute("MATCH p = (:A)-[:R]->(:B) RETURN p");
    const p = rows[0]!.p as LoraValue;
    expect(isPath(p)).toBe(true);
    if (!isPath(p)) throw new Error("expected path");
    // Path invariant: nodes.length == rels.length + 1
    expect(p.nodes.length).toBe(p.rels.length + 1);
    expect(p.nodes.length).toBeGreaterThanOrEqual(2);
  });
});

describe("Database — vector values", () => {
  it("returns a tagged VECTOR from vector()", async () => {
    const db = await createDatabase();
    const { rows } = await db.execute<{ v: LoraValue }>(
      "RETURN vector([1,2,3], 3, INTEGER) AS v",
    );
    const { isVector } = await import("../ts/types.js");
    const v = rows[0]!.v;
    expect(isVector(v)).toBe(true);
    if (!isVector(v)) throw new Error("expected vector");
    expect(v.dimension).toBe(3);
    expect(v.coordinateType).toBe("INTEGER");
    expect(v.values).toEqual([1, 2, 3]);
  });

  it("accepts a VECTOR parameter and round-trips it", async () => {
    const db = await createDatabase();
    const { vector } = await import("../ts/types.js");
    const param = vector([0.1, 0.2, 0.3], 3, "FLOAT32");
    const { rows } = await db.execute<{ v: LoraValue }>(
      "RETURN $v AS v",
      { v: param },
    );
    const { isVector } = await import("../ts/types.js");
    const v = rows[0]!.v;
    expect(isVector(v)).toBe(true);
    if (!isVector(v)) throw new Error("expected vector");
    expect(v.dimension).toBe(3);
    expect(v.coordinateType).toBe("FLOAT32");
  });

  it("uses a vector() parameter inside vector.similarity.cosine", async () => {
    const db = await createDatabase();
    const { vector } = await import("../ts/types.js");
    const query = vector([1.0, 0.0, 0.0], 3, "FLOAT32");
    const { rows } = await db.execute<{ s: number }>(
      "RETURN vector.similarity.cosine(vector([1.0, 0.0, 0.0], 3, FLOAT32), $q) AS s",
      { q: query },
    );
    expect(Math.abs((rows[0]!.s as number) - 1.0)).toBeLessThan(1e-6);
  });

  it("stores a vector parameter as a node property", async () => {
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

  it("rejects a malformed vector parameter", async () => {
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

  it("rejects a vector parameter with unknown coordinateType", async () => {
    const db = await createDatabase();
    await expect(
      db.execute("RETURN $v AS v", {
        v: {
          kind: "vector",
          dimension: 2,
          coordinateType: "BIGINT",
          values: [1, 2],
        },
      }),
    ).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "INVALID_PARAMS",
    );
  });

  it("streams rows with async iteration and toArray", async () => {
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

  it("rolls back a mutating stream when closed before exhaustion", async () => {
    const db = await createDatabase();
    const stream = db.stream<{ i: number }>(
      "UNWIND range(1, 3) AS i CREATE (:EarlyClose {i: i}) RETURN i",
    );
    expect((await stream.next()).value?.i).toBe(1);
    stream.close();

    expect(await db.nodeCount()).toBe(0);
  });

  it("executes statement batches inside one transaction", async () => {
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

  it("isVector returns false for non-vector values", async () => {
    const { isVector } = await import("../ts/types.js");
    expect(isVector(null)).toBe(false);
    expect(isVector([1, 2, 3])).toBe(false);
    expect(isVector({})).toBe(false);
    expect(isVector({ kind: "node", id: 1 })).toBe(false);
    expect(isVector(42)).toBe(false);
    expect(isVector("vector")).toBe(false);
  });
});
