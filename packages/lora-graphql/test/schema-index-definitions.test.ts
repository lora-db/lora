// assertSchema compares a named full-text or vector index's definition with
// the model, not only its name: adding a field to @fulltext must not leave
// search on the old field list.

import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "vitest";
import { createDatabase } from "@loradb/lora-node";
import { LoraGraphQL, loraDriver } from "../src/index.js";

const typeDefs = (fields: string) =>
  `type City @node @fulltext(indexes: [{ fields: ${fields} }]) { key: String! @key  name: String!  aka: [String!]! }`;

test("a changed @fulltext field list is reported, and re-created with create", async () => {
  const dir = await mkdtemp(join(tmpdir(), "lora-graphql-index-"));
  let db = await createDatabase("app", { databaseDir: dir });
  let lora = new LoraGraphQL({
    typeDefs: typeDefs('["name"]'),
    driver: loraDriver(db),
  });
  await lora.assertSchema({ create: true });
  await db.execute(
    "CREATE (:City {key: 'antwerp', name: 'Antwerp', aka: ['Anvers']})",
  );
  db.dispose?.();

  db = await createDatabase("app", { databaseDir: dir });
  lora = new LoraGraphQL({
    typeDefs: typeDefs('["name", "aka"]'),
    driver: loraDriver(db),
  });
  const report = await lora.assertSchema();
  expect(report.mismatched.map((r) => r.name)).toEqual(["city_search"]);
  expect((await lora.check()).ok).toBe(false);

  const fixed = await lora.assertSchema({ create: true });
  expect(fixed.recreated.map((r) => r.name)).toEqual(["city_search"]);
  expect(fixed.mismatched).toEqual([]);
  const found = await lora.execute({
    source: '{ searchCities(query: "anvers") { node { key } } }',
  });
  expect(found.data).toEqual({ searchCities: [{ node: { key: "antwerp" } }] });
  // Nothing left to do.
  expect((await lora.assertSchema()).mismatched).toEqual([]);
  db.dispose?.();
});
