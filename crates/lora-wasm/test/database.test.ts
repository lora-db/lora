import { describe, it, expect, beforeEach } from "vitest";
import {
  Database,
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
    db = await Database.create();
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
});

describe("Database — value model", () => {
  it("roundtrips a list of mixed-scalar values", async () => {
    const db = await Database.create();
    await db.execute("CREATE (:N {xs: $xs})", { xs: [1, "two", true, null] });
    const { rows } = await db.execute("MATCH (n:N) RETURN n.xs AS xs");
    expect(rows[0]!.xs).toEqual([1, "two", true, null]);
  });

  it("roundtrips a nested map", async () => {
    const db = await Database.create();
    await db.execute("CREATE (:N {meta: $m})", {
      m: { a: 1, b: { c: "deep", d: [true, false] } },
    });
    const { rows } = await db.execute("MATCH (n:N) RETURN n.meta AS m");
    expect(rows[0]!.m).toEqual({ a: 1, b: { c: "deep", d: [true, false] } });
  });

  it("returns tagged date values from stored properties", async () => {
    const db = await Database.create();
    await db.execute("CREATE (:E {d: date('2025-03-14')})");
    const { rows } = await db.execute("MATCH (n:E) RETURN n.d AS d");
    const d = rows[0]!.d;
    expect(isTemporal(d)).toBe(true);
    expect(d).toEqual({ kind: "date", iso: "2025-03-14" });
  });

  it("accepts typed date + duration params", async () => {
    const db = await Database.create();
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
    const db = await Database.create();
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
    const db = await Database.create();
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
    const db = await Database.create();
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
    const db = await Database.create();
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
    const db = await Database.create();
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
    const db = await Database.create();
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
    const db = await Database.create();
    await expect(db.execute("THIS IS NOT CYPHER")).rejects.toSatisfy(
      (e) => e instanceof LoraError && e.code === "LORA_ERROR",
    );
  });

  it("throws INVALID_PARAMS for a malformed temporal param", async () => {
    const db = await Database.create();
    await expect(
      db.execute("RETURN $d AS d", { d: { kind: "date", iso: "not-a-date" } }),
    ).rejects.toSatisfy((e) => e instanceof LoraError && e.code === "INVALID_PARAMS");
  });
});
