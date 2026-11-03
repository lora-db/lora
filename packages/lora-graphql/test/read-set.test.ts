// `execute({ readSet: true })`: what an operation read, for a response
// cache that invalidates selectively on `onWrite`.

import { describe, expect, test } from "vitest";
import type { WriteChange } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type Post @node @mutation {
  key: String! @key
  title: String
  tags: [Tag!]! @relationship(type: "TAGGED", direction: OUT)
}
type Tag @node @mutation { key: String! @key }
type Note @node @mutation { key: String! @key }
type Query {
  postCount: Int! @cypher(statement: "MATCH (p:Post) RETURN count(p) AS n")
}`;

async function setup() {
  const t = await createTestLoraGraphQL({
    typeDefs,
    seed: [
      "CREATE (:Post {key: 'p1', title: 'One'})-[:TAGGED]->(:Tag {key: 't1'})",
    ],
  });
  const changes: WriteChange[] = [];
  t.lora.onWrite((c) => changes.push(c));
  return { ...t, changes };
}

describe("execute({ readSet: true })", () => {
  test("names the labels and relationship types a query read, off the wire", async () => {
    const { lora } = await setup();
    const r = await lora.execute({
      source: "{ posts { key tags { key } } }",
      readSet: true,
    });
    expect(r.errors).toBeUndefined();
    expect(r.readSet).toEqual({
      labels: ["Post", "Tag"],
      relationships: ["TAGGED"],
    });
    expect(Object.keys(r)).not.toContain("readSet");
    expect(JSON.stringify(r)).not.toContain("readSet");
    // Without the option there is none.
    expect(
      (await lora.execute({ source: "{ posts { key } }" })).readSet,
    ).toBeUndefined();
  });

  test("lora.affects tells which cached results a write makes stale", async () => {
    const { lora, changes } = await setup();
    const posts = await lora.execute({
      source: "{ posts { key } }",
      readSet: true,
    });
    const tags = await lora.execute({
      source: "{ tags { key } }",
      readSet: true,
    });
    await lora.execute({
      source:
        'mutation { createNotes(input: [{ key: "n1" }]) { notes { key } } }',
    });
    await lora.execute({
      source:
        'mutation { createTags(input: [{ key: "t2" }]) { tags { key } } }',
    });
    const [note, tag] = changes;
    expect(lora.affects(posts.readSet!, note!)).toBe(false);
    expect(lora.affects(tags.readSet!, note!)).toBe(false);
    expect(lora.affects(tags.readSet!, tag!)).toBe(true);
    expect(lora.affects(posts.readSet!, tag!)).toBe(false);
  });

  test("a @cypher read names what its statement's patterns read", async () => {
    const { lora, changes } = await setup();
    const count = await lora.execute({
      source: "{ postCount }",
      readSet: true,
    });
    expect(count.readSet).toEqual({ labels: ["Post"], relationships: [] });
    await lora.execute({
      source:
        'mutation { createNotes(input: [{ key: "n2" }]) { notes { key } } }',
    });
    await lora.execute({
      source:
        'mutation { createPosts(input: [{ key: "p2" }]) { posts { key } } }',
    });
    const [note, post] = changes;
    expect(lora.affects(count.readSet!, note!)).toBe(false);
    expect(lora.affects(count.readSet!, post!)).toBe(true);
  });

  test("a mutation, or a @cypher statement that does not tell, is opaque", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `${typeDefs}
      extend type Query {
        anyCount: Int! @cypher(statement: "MATCH (n) RETURN count(n) AS n")
        declared: Int!
          @cypher(statement: "MATCH (n) RETURN count(n) AS n", reads: ["Note"])
      }`,
    });
    const changes: WriteChange[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const any = await t.lora.execute({ source: "{ anyCount }", readSet: true });
    expect(any.readSet).toEqual({
      labels: [],
      relationships: [],
      opaque: true,
    });
    const mutation = await t.lora.execute({
      source:
        'mutation { createNotes(input: [{ key: "n2" }]) { notes { key } } }',
      readSet: true,
    });
    expect(mutation.readSet?.opaque).toBe(true);
    expect(t.lora.affects(any.readSet!, changes[0]!)).toBe(true);
    // `reads:` stands in for what the statement does not tell.
    const declared = await t.lora.execute({
      source: "{ declared }",
      readSet: true,
    });
    expect(declared.readSet).toEqual({ labels: ["Note"], relationships: [] });
  });

  test("an object @cypher field adds its labels, types and node type (G-78)", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type A @node @mutation {
        key: String! @key  ref: String
        asset: B @cypher(statement: "MATCH (b:B {key: this.ref}) RETURN b LIMIT 1")
        tagged: Int! @cypher(statement: "RETURN COUNT { (this)-[:TAGGED]->(:T) } AS n")
        via: [String!]! @cypher(statement: "MATCH (x:X) RETURN x.key AS k", reads: ["LINKS"])
      }
      type B @node @mutation { key: String! @key }
      type C @node @mutation { key: String! @key }
      type T @node { key: String! @key }`,
      seed: ["CREATE (:A {key: 'a', ref: 'b'}), (:B {key: 'b'})"],
    });
    const changes: WriteChange[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const read = async (source: string) =>
      (await t.lora.execute({ source, readSet: true })).readSet!;
    const asset = await read("{ as { key asset { key } } }");
    expect(asset).toEqual({ labels: ["A", "B"], relationships: [] });
    expect(await read("{ as { tagged } }")).toEqual({
      labels: ["A", "T"],
      relationships: ["TAGGED"],
    });
    expect(await read("{ as { via } }")).toEqual({
      labels: ["A", "LINKS", "X"],
      relationships: ["LINKS"],
    });
    await t.lora.execute({
      source: 'mutation { createCs(input: [{ key: "c" }]) { cs { key } } }',
    });
    await t.lora.execute({
      source: 'mutation { createBs(input: [{ key: "b2" }]) { bs { key } } }',
    });
    await t.lora.execute({
      source:
        'mutation { updateA(key: "a", update: { ref: "b2" }) { a { key } } }',
    });
    const [c, b, a] = changes;
    expect(t.lora.affects(asset, c!)).toBe(false);
    expect(t.lora.affects(asset, b!)).toBe(true);
    expect(t.lora.affects(asset, a!)).toBe(true);
  });

  test("an operation-wide mutation transaction still collects", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      mutationTransaction: "operation",
    });
    const r = await t.lora.execute({
      source: `mutation {
        a: createNotes(input: [{ key: "a" }]) { notes { key } }
        b: createTags(input: [{ key: "b" }]) { tags { key } }
      }`,
      readSet: true,
    });
    expect(r.errors).toBeUndefined();
    expect(r.readSet?.opaque).toBe(true);
  });
});
