// check() reads stored relationship properties against their rules too.

import { expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

test("a relationship property that breaks its rule is reported, by the declaring node", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: `
      type Person @node {
        key: String! @key
        ratings: [Film!]! @relationship(type: "RATED", direction: OUT, properties: "Rated")
      }
      type Film @node {
        key: String! @key
        ratedBy: [Person!]! @relationship(type: "RATED", direction: IN, properties: "Rated")
      }
      type Rated @relationshipProperties {
        stars: Int! @range(min: 1, max: 5)
        note: String @length(max: 5)
      }
    `,
    seed: [
      "CREATE (:Person {key: 'ada'})-[:RATED {stars: 4, note: 'ok'}]->(:Film {key: 'a'})",
      "MATCH (p:Person {key: 'ada'}) CREATE (p)-[:RATED {stars: 9, note: 'far too long'}]->(:Film {key: 'b'})",
    ],
  });
  const report = await t.lora.check();
  expect(
    report.stored.map((v) => [v.type, v.key, v.field, v.rule]).sort(),
  ).toEqual([
    ["Person", "ada", "ratings.note", "maxLength"],
    ["Person", "ada", "ratings.stars", "maximum"],
  ]);
  t.close();
});
