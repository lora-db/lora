import { afterEach, describe, expect, it } from "vitest";

import { createDatabase, type Database } from "../ts/index.js";

const cleanups = new Set<Database>();

afterEach(async () => {
  for (const db of cleanups) {
    await db.dispose();
  }
  cleanups.clear();
});

async function newDb(): Promise<Database> {
  const db = await createDatabase();
  cleanups.add(db);
  return db;
}

describe("explain", () => {
  it("does not execute mutating queries", async () => {
    const db = await newDb();
    const plan = await db.explain("CREATE (:Foo {n: 1})");
    expect(plan.shape).toBe("mutating");
    expect(await db.nodeCount()).toBe(0);
  });

  it("returns a populated operator tree for a label scan", async () => {
    const db = await newDb();
    await db.execute("CREATE (:Person {name: 'Alice'})");
    const plan = await db.explain("MATCH (p:Person) RETURN p");
    expect(plan.shape).toBe("readOnly");
    expect(plan.resultColumns).toEqual(["p"]);
    expect(plan.tree).toBeDefined();
    expect(plan.query).toBe("MATCH (p:Person) RETURN p");
  });

  it("forwards parameters", async () => {
    const db = await newDb();
    const plan = await db.explain(
      "MATCH (p:Person) WHERE p.name = $name RETURN p",
      { name: "Alice" },
    );
    expect(plan.shape).toBe("readOnly");
  });

  it("surfaces parse errors with the same shape as execute", async () => {
    const db = await newDb();
    let execErr: unknown;
    let explainErr: unknown;
    try {
      await db.execute("INVALID");
    } catch (e) {
      execErr = e;
    }
    try {
      await db.explain("INVALID");
    } catch (e) {
      explainErr = e;
    }
    expect(execErr).toBeDefined();
    expect(explainErr).toBeDefined();
    // Both errors should share the LORA_ error code prefix.
    expect(String(execErr).split(":")[0]).toBe(
      String(explainErr).split(":")[0],
    );
  });
});

describe("profile", () => {
  it("executes mutating queries (PROFILE runs writes)", async () => {
    const db = await newDb();
    const profile = await db.profile("CREATE (:Foo {n: 1}) RETURN 1 AS one");
    expect(profile.metrics.mutated).toBe(true);
    expect(profile.metrics.totalRows).toBe(1);
    expect(await db.nodeCount()).toBe(1);
  });

  it("reports per-operator step timing", async () => {
    const db = await newDb();
    for (const name of ["Alice", "Bob", "Carol", "Dave"]) {
      await db.execute(`CREATE (:Person {name: '${name}'})`);
    }
    const profile = await db.profile(
      "MATCH (p:Person) WHERE p.name <> 'Bob' RETURN p.name AS name",
    );
    expect(profile.metrics.totalRows).toBe(3);
    expect(profile.metrics.mutated).toBe(false);
    expect(Object.keys(profile.metrics.perOperator).length).toBeGreaterThan(0);

    for (const id of Object.keys(profile.metrics.perOperator)) {
      const op = profile.metrics.perOperator[id]!;
      expect(op.nextCalls).toBeGreaterThan(0);
    }
  });

  it("forwards parameters", async () => {
    const db = await newDb();
    await db.execute("CREATE (:Person {name: 'Alice'})");
    await db.execute("CREATE (:Person {name: 'Bob'})");
    const profile = await db.profile(
      "MATCH (p:Person) WHERE p.name = $name RETURN p",
      { name: "Alice" },
    );
    expect(profile.metrics.totalRows).toBe(1);
  });
});
