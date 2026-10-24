// `tx.touched()`: the application's own Cypher in a `lora.begin()`
// transaction states what it wrote, and the commit reports it like a
// mutation's write-set, so a response cache keyed on read-sets drops the
// reads it changed.
import { describe, expect, test } from "vitest";
import type { ReadSet, WriteChange } from "../src/index.js";
import { createTestLoraGraphQL, type TestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  type Claims @jwt {
    sub: String!
  }
  type MovedKey @node {
    key: String! @key
    to: String!
  }
  type Artist @node {
    key: String! @key
    name: String!
  }
  type Query {
    movedTo(key: String!): String
      @cypher(
        statement: "MATCH (m:MovedKey {key: $key}) RETURN m.to AS to"
        columnName: "to"
      )
  }
`;

const LOOKUP = `{ movedTo(key: "artist:x") artists { key } }`;

/** A response cache that drops every entry a committed write affects. */
function cached(t: TestLoraGraphQL) {
  const entries = new Map<string, { data: unknown; readSet: ReadSet }>();
  t.lora.onWrite((change) => {
    for (const [k, e] of entries) {
      if (t.lora.affects(e.readSet, change)) entries.delete(k);
    }
  });
  return async (source: string) => {
    const hit = entries.get(source);
    if (hit) return hit.data;
    const r = await t.lora.execute({
      source,
      context: { jwt: { sub: "u" } },
      readSet: true,
    });
    if (r.errors) throw r.errors[0];
    entries.set(source, { data: r.data, readSet: r.readSet! });
    return r.data;
  };
}

describe("tx.touched()", () => {
  test("a cached read answers the transaction's write after the commit", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const read = cached(t);
    expect(await read(LOOKUP)).toEqual({ movedTo: null, artists: [] });

    const tx = await t.lora.begin();
    await tx.execute("CREATE (:MovedKey {key: 'artist:x', to: 'y'})");
    tx.touched({ types: ["MovedKey"] });
    // Not before the commit.
    expect(await read(LOOKUP)).toEqual({ movedTo: null, artists: [] });
    await tx.commit();
    expect(await read(LOOKUP)).toEqual({ movedTo: "y", artists: [] });
    t.close();
  });

  test("without it the commit reports nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const read = cached(t);
    expect(await read(LOOKUP)).toEqual({ movedTo: null, artists: [] });
    const tx = await t.lora.begin();
    await tx.execute("CREATE (:MovedKey {key: 'artist:x', to: 'y'})");
    await tx.commit();
    // The cache cannot know: it serves the stale read.
    expect(await read(LOOKUP)).toEqual({ movedTo: null, artists: [] });
    t.close();
  });

  test("the change names what was stated", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const changes: WriteChange[] = [];
    t.lora.onWrite((c) => changes.push(c));

    const tx = await t.lora.begin();
    await tx.execute("CREATE (:Artist {key: 'a1', name: 'A'})");
    tx.touched({
      entities: [{ type: "Artist", key: "a1" }],
      relationshipTypes: ["PLAYS"],
      field: "adoptCatalogue",
    });
    await tx.commit();
    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({
      operation: "CYPHER",
      field: "adoptCatalogue",
      entities: [{ type: "Artist", key: "a1" }],
      updated: [{ type: "Artist", key: "a1" }],
      types: ["Artist"],
      relationshipTypes: ["PLAYS"],
      broad: false,
    });
    t.close();
  });

  test("without an argument everything counts as changed", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const changes: WriteChange[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const tx = await t.lora.begin();
    await tx.execute("CREATE (:Artist {key: 'a1', name: 'A'})");
    tx.touched();
    await tx.commit();
    expect(changes.map((c) => c.broad)).toEqual([true]);
    expect(
      t.lora.affects({ labels: ["Nothing"], relationships: [] }, changes[0]!),
    ).toBe(true);
    t.close();
  });

  test("a rollback reports nothing", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const changes: WriteChange[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const tx = await t.lora.begin();
    await tx.execute("CREATE (:Artist {key: 'a1', name: 'A'})");
    tx.touched({ types: ["Artist"] });
    await tx.rollback();
    expect(changes).toEqual([]);
    t.close();
  });

  test("an unknown type is refused", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const tx = await t.lora.begin();
    expect(() => tx.touched({ types: ["Nope"] })).toThrow(
      "touched(): Nope is not a node type of the schema",
    );
    await tx.rollback();
    t.close();
  });
});
