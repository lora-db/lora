// `mutationTransaction: "operation"` runs every root field of a mutation
// in one transaction: a failure rolls all of them back.

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

describe('mutationTransaction: "operation"', () => {
  const typeDefs = `type C @node @mutation { key: String! @key  p: [P!]! @relationship(type: "IN", direction: IN) }
    type P @node { key: String! @key }`;
  const seed = ["CREATE (:P {key: 'u'})"];
  const failing = `mutation {
    a: createCs(input: [{ key: "c1", p: { connect: [{ key: "u" }] } }]) { cs { key } }
    b: createCs(input: [{ key: "c2", p: { connect: [{ key: "nobody" }] } }]) { cs { key } } }`;
  const cs = async (t: Awaited<ReturnType<typeof createTestLoraGraphQL>>) =>
    (
      (await t.db.execute("MATCH (c:C) RETURN c.key AS k ORDER BY k")) as {
        rows: Array<Record<string, unknown>>;
      }
    ).rows.map((r) => r["k"]);

  test("by default each root field commits on its own", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.lora.execute({ source: failing });
    expect(codes(r)).toEqual(["NOT_FOUND"]);
    expect(await cs(t)).toEqual(["c1"]);
  });

  test("with the option a failure rolls every root field back", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed,
      mutationTransaction: "operation",
    });
    const changes: unknown[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const r = await t.lora.execute({ source: failing });
    expect(codes(r)).toEqual(["NOT_FOUND"]);
    expect(r.data).toBeNull();
    expect(await cs(t)).toEqual([]);
    // No change event for a write that was rolled back.
    expect(changes).toEqual([]);
  });

  test("with the option a successful operation commits every root field", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed,
      mutationTransaction: "operation",
    });
    const changes: unknown[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const r = await t.lora.execute({
      source: `mutation {
        a: createCs(input: [{ key: "c1", p: { connect: [{ key: "u" }] } }]) { cs { key } }
        b: createCs(input: [{ key: "c2" }]) { cs { key } } }`,
    });
    expect(r.errors).toBeUndefined();
    expect(await cs(t)).toEqual(["c1", "c2"]);
    expect(changes).toHaveLength(2);
    // Reads and the database stay usable: the transaction was closed.
    expect(await t.data(`{ cs { key } }`)).toEqual({
      cs: [{ key: "c1" }, { key: "c2" }],
    });
  });

  test("persisted mutations take the same path", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed,
      mutationTransaction: "operation",
    });
    t.lora.persist({ StartTrip: failing });
    const r = await t.lora.execute({ id: "StartTrip" });
    expect(codes(r)).toEqual(["NOT_FOUND"]);
    expect(await cs(t)).toEqual([]);
  });
});
