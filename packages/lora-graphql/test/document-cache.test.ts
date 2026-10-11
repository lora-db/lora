// The parsed-document cache holds 500 documents and drops the least
// recently used: a document a client keeps sending stays parsed.

import { buildSchema } from "graphql";
import { expect, test } from "vitest";
import { Documents } from "../src/runtime/documents.js";

test("a document that is used again outlives ones that are not", () => {
  const schema = buildSchema(`type Query { a: Int }`);
  const documents = new Documents({
    guards: false,
    persistedOnly: false,
    bytesMax: Infinity,
    schema: () => schema,
    rules: () => [],
  });
  const source = (i: number) => `query Q${i} { a }`;
  const parsed = (i: number) => documents.document({ source: source(i) });

  const hot = parsed(0);
  for (let i = 1; i < 500; i++) parsed(i);
  // Used again, then one more document pushes the cache past its size.
  expect(parsed(0)).toBe(hot);
  const second = parsed(1);
  parsed(500);
  parsed(501);
  // The hot ones are the same objects; the oldest unused one was parsed anew.
  expect(parsed(0)).toBe(hot);
  expect(parsed(1)).toBe(second);
  const third = parsed(2);
  expect(parsed(2)).toBe(third);
});

test("the oldest unused document is the one dropped", () => {
  const schema = buildSchema(`type Query { a: Int }`);
  const documents = new Documents({
    guards: false,
    persistedOnly: false,
    bytesMax: Infinity,
    schema: () => schema,
    rules: () => [],
  });
  const parsed = (i: number) =>
    documents.document({ source: `query Q${i} { a }` });
  const first = parsed(0);
  const kept = parsed(1);
  for (let i = 2; i < 500; i++) parsed(i);
  parsed(1);
  parsed(500);
  expect(parsed(1)).toBe(kept);
  expect(parsed(0)).not.toBe(first);
});
