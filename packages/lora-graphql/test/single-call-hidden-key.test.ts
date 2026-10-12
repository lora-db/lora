// A single-call update that creates a related node under a key the caller
// cannot see must be refused before it commits, as a transaction refuses
// it. The batch moves the hidden node's key aside to make room, so a batch
// that commits anyway leaves that node with a placeholder key and the
// caller's node holding the key it took.
//
// Regression (found in review before 0.25.0): the create's batch ended
// with the assertion that nothing was moved; the update's did not.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)
  type Claims @jwt {
    sub: String!
  }
  type Post @node @mutation {
    key: String! @key
    title: String
    tags: [Tag!]! @relationship(type: "HAS", direction: OUT)
  }
  type Tag
    @node
    @mutation
    @authorization(
      filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
    ) {
    key: String! @key
    owner: String
    slug: String @unique
  }
`;
const seed = [
  "CREATE (:Post {key: 'p0', title: 'old'})",
  "CREATE (:Tag {key: 'theirs', owner: 'other', slug: 's'})",
  "CREATE (:Tag {key: 'mine', owner: 'me', slug: 'm'})",
];
const me = { jwt: { sub: "me" } };
const graph =
  "MATCH (p:Post) OPTIONAL MATCH (p)-[:HAS]->(t:Tag) WITH p, collect(t.key) AS linked " +
  "MATCH (t:Tag) WITH p, linked, t ORDER BY t.slug, t.key " +
  "RETURN p.title AS title, linked, collect([t.key, t.owner]) AS tags";

const updates = {
  "a nested create under a hidden key": `mutation { updatePost(key: "p0", update: { title: "new",
    tags: { create: [{ node: { key: "theirs", owner: "me" } }] } }) { post { key } } }`,
  "the same beside a create and a connect that are allowed": `mutation { updatePost(key: "p0", update: { title: "new",
    tags: { create: [{ node: { key: "fresh", owner: "me" } }, { node: { key: "theirs", owner: "me" } }],
            connect: [{ key: "mine" }] } }) { post { key } } }`,
};

describe.each([true, false])("singleCallMutations: %s", (single) => {
  test.each(Object.entries(updates))(
    "%s is refused and writes nothing",
    async (_, source) => {
      const t = await createTestLoraGraphQL({
        typeDefs,
        seed,
        singleCallMutations: single,
      });
      const before = (await t.db.execute(graph)).rows;
      const result = await t.run(source, {}, me);
      expect(
        result.errors?.map((e) => [e.extensions?.["code"], e.message]),
      ).toEqual([["FORBIDDEN", "not allowed to create this Tag"]]);
      expect((await t.db.execute(graph)).rows).toEqual(before);
      t.close();
    },
  );
});
