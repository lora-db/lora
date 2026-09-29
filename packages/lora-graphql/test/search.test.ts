import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Event
    @node
    @mutation
    @fulltext(indexes: [{ fields: ["title", "summary"] }])
    @authorization(
      filter: [
        {
          where: { node: { hidden: { eq: false } } }
          requireAuthentication: false
        }
      ]
    ) {
    key: String! @key
    title: String!
    summary: String
    city: String @filterable
    hidden: Boolean! @default(value: false)
    embedding: [Float!] @vector(dimensions: 3, similarity: COSINE)
  }
`;

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness({ typeDefs, seed: [] });
  await h.data(`mutation {
    createEvents(input: [
      { key: "e1", title: "Techno by the sea", summary: "Sunset techno", city: "Ams", embedding: [1, 0, 0] }
      { key: "e2", title: "Jazz night", summary: "Smooth jazz and techno", city: "Utr", embedding: [0.9, 0.1, 0] }
      { key: "e3", title: "Opera", summary: "Classical", city: "Ams", embedding: [0, 1, 0] }
      { key: "e4", title: "Secret techno", summary: "techno", hidden: true, embedding: [1, 0, 0] }
    ]) { info { nodesCreated } }
  }`);
});

test("S1: full-text and vector indexes are required and created", () => {
  expect(
    h.lora
      .requirements()
      .filter((r) => r.kind === "fulltext" || r.kind === "vector")
      .map((r) => r.name),
  ).toEqual(["event_search", "event_embedding_vector"]);
});

test("full-text search ranks, filters and respects read rules", async () => {
  const d = await h.data<{
    searchEvents: Array<{ node: { key: string }; score: number }>;
  }>(`{ searchEvents(query: "techno") { score node { key title } } }`);
  expect(d.searchEvents.map((m) => m.node.key)).toEqual(["e1", "e2"]);
  expect(d.searchEvents[0]!.score).toBeGreaterThanOrEqual(
    d.searchEvents[1]!.score,
  );
  const filtered = await h.data<{
    searchEvents: Array<{ node: { key: string } }>;
  }>(
    `{ searchEvents(query: "techno", where: { city: { eq: "Utr" } }) { node { key } } }`,
  );
  expect(filtered.searchEvents).toEqual([{ node: { key: "e2" } }]);
  const prefix = await h.data<{ searchEvents: unknown[] }>(
    `{ searchEvents(query: "oper*") { node { key } } }`,
  );
  expect(prefix.searchEvents).toEqual([{ node: { key: "e3" } }]);
});

test("vector search by a query vector or by a stored node", async () => {
  const byVector = await h.data<{
    similarEvents: Array<{ node: { key: string } }>;
  }>(
    `{ similarEvents(vector: [1, 0, 0], limit: 2) { score node { key embedding } } }`,
  );
  expect(byVector.similarEvents.map((m) => m.node.key)).toEqual(["e1", "e2"]);
  expect(byVector.similarEvents[0]).toMatchObject({
    node: { embedding: [1, 0, 0] },
  });
  const byNode = await h.data<{
    similarEvents: Array<{ node: { key: string } }>;
  }>(`{ similarEvents(to: "e1", limit: 2) { node { key } } }`);
  expect(byNode.similarEvents.map((m) => m.node.key)).toEqual(["e2", "e3"]);
});

test("vector input errors", async () => {
  const both = await h.run(
    `{ similarEvents(vector: [1, 0, 0], to: "e1") { score } }`,
  );
  expect(both.errors?.[0]?.message).toBe(
    "similarEvents takes exactly one of `vector` and `to`",
  );
  const dims = await h.run(`{ similarEvents(vector: [1, 0]) { score } }`);
  expect(dims.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  const store = await h.run(
    `mutation { createEvents(input: [{ key: "bad", title: "x", embedding: [1] }]) { info { nodesCreated } } }`,
  );
  expect(store.errors?.[0]?.message).toBe(
    "embedding has 1 dimensions; it needs 3",
  );
});
