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

  test("a @cypher read or a mutation is opaque: every change affects it", async () => {
    const { lora, changes } = await setup();
    const count = await lora.execute({
      source: "{ postCount }",
      readSet: true,
    });
    expect(count.readSet).toEqual({
      labels: [],
      relationships: [],
      opaque: true,
    });
    const mutation = await lora.execute({
      source:
        'mutation { createNotes(input: [{ key: "n2" }]) { notes { key } } }',
      readSet: true,
    });
    expect(mutation.readSet?.opaque).toBe(true);
    expect(lora.affects(count.readSet!, changes[0]!)).toBe(true);
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
