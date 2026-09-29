import { readFileSync } from "node:fs";
import { buildModel } from "../src/index.js";

test("the README's example schemas are valid", () => {
  const readme = readFileSync(new URL("../README.md", import.meta.url), "utf8");
  const blocks = [
    ...readme.matchAll(/const typeDefs = \/\* GraphQL \*\/ `([\s\S]*?)`;/g),
  ];
  expect(blocks.length).toBeGreaterThan(0);
  for (const [, sdl] of blocks) expect(() => buildModel(sdl!)).not.toThrow();
  // The Post example of the authorization section.
  const post = /```graphql\n(type Post[\s\S]*?)```/.exec(readme)![1]!;
  expect(() =>
    buildModel(post + "\ntype User @node { key: String! @key }"),
  ).not.toThrow();
});
