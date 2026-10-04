// Write-side hardening: @cypher mutations in shared transactions, batch
// limits, hidden nodes in write errors, mutation defaults per operation,
// and atomic operations on the getSchema() path.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const codes = (r: Result) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

/** Fails fast instead of hanging when a write waits on the writer lock. */
function within<T>(ms: number, promise: Promise<T>): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`still waiting after ${ms} ms`)), ms),
    ),
  ]);
}

const rows = async (
  t: Awaited<ReturnType<typeof createTestLoraGraphQL>>,
  query: string,
) =>
  (
    (await t.db.execute(query)) as {
      rows: Array<Record<string, unknown>>;
    }
  ).rows;

describe("@cypher mutations in a shared transaction", () => {
  const typeDefs = `type C @node @mutation { key: String! @key }
    type Mutation {
      makeC(key: String!): C
        @cypher(statement: "CREATE (c:C {key: $key}) RETURN c", columnName: "c")
    }`;
  const keys = async (t: Awaited<ReturnType<typeof createTestLoraGraphQL>>) =>
    (await rows(t, "MATCH (c:C) RETURN c.key AS k ORDER BY k")).map(
      (r) => r["k"],
    );

  test('runs in the operation\'s transaction with mutationTransaction: "operation"', async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      mutationTransaction: "operation",
    });
    const changes: Array<{ operation: string }> = [];
    t.lora.onWrite((c) => changes.push(c));
    const r = await within(
      3000,
      t.lora.execute({
        source: `mutation {
          a: createCs(input: [{ key: "c1" }]) { cs { key } }
          b: makeC(key: "c2") { key } }`,
      }),
    );
    expect(r.errors).toBeUndefined();
    expect(r.data).toEqual({ a: { cs: [{ key: "c1" }] }, b: { key: "c2" } });
    expect(await keys(t)).toEqual(["c1", "c2"]);
    expect(changes.map((c) => c.operation)).toEqual(["CREATE", "CYPHER"]);
    // Later writes still go through.
    const after = await within(
      3000,
      t.lora.execute({ source: `mutation { makeC(key: "c3") { key } }` }),
    );
    expect(after.errors).toBeUndefined();
  });

  test("a failing root field rolls the @cypher write back too", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      mutationTransaction: "operation",
    });
    const changes: unknown[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const r = await within(
      3000,
      t.lora.execute({
        source: `mutation {
          b: makeC(key: "c2") { key }
          a: createCs(input: [{ key: "c2" }]) { cs { key } } }`,
      }),
    );
    expect(codes(r)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(await keys(t)).toEqual([]);
    expect(changes).toEqual([]);
  });

  test("runs in a caller-owned transaction; the event waits for the commit", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const changes: unknown[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const tx = await t.lora.begin();
    const r = await within(
      3000,
      t.lora.execute({
        source: `mutation { makeC(key: "c1") { key } }`,
        context: { transaction: tx },
      }),
    );
    expect(r.errors).toBeUndefined();
    expect(changes).toEqual([]);
    await tx.commit();
    expect(changes).toHaveLength(1);
    expect(await keys(t)).toEqual(["c1"]);
  });

  test("waiting for the writer lock honours timeoutMs", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, timeoutMs: 200 });
    const tx = await t.lora.begin();
    try {
      const r = await within(
        3000,
        t.lora.execute({ source: `mutation { makeC(key: "c1") { key } }` }),
      );
      expect(codes(r)).toEqual(["DATABASE_ERROR"]);
      const generated = await within(
        3000,
        t.lora.execute({
          source: `mutation { createCs(input: [{ key: "c2" }]) { cs { key } } }`,
        }),
      );
      expect(codes(generated)).toEqual(["DATABASE_ERROR"]);
    } finally {
      await tx.rollback();
    }
    // The lock is free again, and the late transactions were released.
    const r = await within(
      3000,
      t.lora.execute({ source: `mutation { makeC(key: "c3") { key } }` }),
    );
    expect(r.errors).toBeUndefined();
    expect(await keys(t)).toEqual(["c3"]);
  });
});

describe("@uniqueTogether under @cypher mutations", () => {
  const typeDefs = `type Person @node { key: String! @key }
    type Pair @node @mutation @uniqueTogether(fields: ["a", "b"]) {
      key: String! @key  a: String  b: String
    }
    type Follow @node @uniqueTogether(fields: ["from", "to"]) {
      key: String! @key
      from: Person! @relationship(type: "FROM", direction: OUT)
      to: Person! @relationship(type: "TO", direction: OUT)
    }
    type Mutation {
      makePair(key: String!, a: String!, b: String!): Pair
        @cypher(statement: "CREATE (p:Pair {key: $key, a: $a, b: $b}) RETURN p", columnName: "p")
      setB(key: String!, b: String!): Pair
        @cypher(statement: "MATCH (p) WHERE p.key = $key SET p.b = $b RETURN p", columnName: "p")
      follow(key: String!, from: String!, to: String!): Follow
        @cypher(statement: """
          MATCH (x {key: $from}), (y {key: $to})
          CREATE (f:Follow {key: $key}), (f)-[:FROM]->(x), (f)-[:TO]->(y)
          RETURN f
        """, columnName: "f")
    }`;
  const seed = "CREATE (:Person {key: 'lou'}), (:Person {key: 'bo'})";

  test("a combination a @cypher mutation duplicates is refused and rolled back", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      (await t.run(`mutation { makePair(key: "p1", a: "x", b: "y") { key } }`))
        .errors,
    ).toBeUndefined();
    const dup = await t.run(
      `mutation { makePair(key: "p2", a: "x", b: "y") { key } }`,
    );
    expect(codes(dup)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(dup.errors?.[0]?.message).toMatch(/Pair must be unique by \(a, b\)/);
    expect(await rows(t, "MATCH (p:Pair) RETURN p.key AS k")).toEqual([
      { k: "p1" },
    ]);
    // A write that names no label still reaches the constrained property.
    await t.data(`mutation { makePair(key: "p3", a: "x", b: "z") { key } }`);
    const set = await t.run(`mutation { setB(key: "p3", b: "y") { key } }`);
    expect(codes(set)).toEqual(["CONSTRAINT_VIOLATION"]);
  });

  test("relationship ends written by a @cypher mutation are checked", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(
      `mutation { follow(key: "f1", from: "lou", to: "bo") { key } }`,
    );
    const dup = await t.run(
      `mutation { follow(key: "f2", from: "lou", to: "bo") { key } }`,
    );
    expect(codes(dup)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(
      (
        await t.run(
          `mutation { follow(key: "f3", from: "bo", to: "lou") { key } }`,
        )
      ).errors,
    ).toBeUndefined();
  });

  test("the model warns that such a mutation checks every node of the type", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const warnings = t.lora.model.warnings.filter((w) =>
      w.message.includes("@uniqueTogether"),
    );
    expect(warnings.map((w) => `${w.type}.${w.field}`).sort()).toEqual([
      "Mutation.follow",
      "Mutation.makePair",
      "Mutation.setB",
    ]);
  });
});
