// G-60: a search root takes the type's `sort`. A sort replaces score
// order (score is still selectable), and the connection pages by keyset
// on the sort fields, then the key.
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { createTestLoraGraphQL, type TestLoraGraphQL } from "../src/testing.js";

const J = "type Claims @jwt { sub: String!  roles: [String!] }\n";
const typeDefs =
  J +
  `type F @node @mutation @fulltext(indexes: [{ fields: ["name"] }]) {
    key: String! @key
    name: String!
    startsOn: String @sortable
    rank: Int @sortable
    embedding: [Float!] @vector(dimensions: 2, similarity: COSINE)
  }`;
const viewer = { jwt: { sub: "u" } };

type Edges = {
  edges: Array<{ cursor: string; score: number; node: { key: string } }>;
  pageInfo: { hasNextPage: boolean };
  totalCount: number;
};

describe("search with sort", () => {
  let t: TestLoraGraphQL;
  beforeAll(async () => {
    t = await createTestLoraGraphQL({
      typeDefs,
      seed: [
        "UNWIND range(1, 5) AS i CREATE (:F {key: 'f' + toString(i), name: 'sun', startsOn: '2027-0' + toString(6 - i), rank: i % 2})",
        "CREATE (:F {key: 'g1', name: 'moon', startsOn: '2027-01'})",
      ],
    });
    // Vectors are stored through a mutation, which writes them as VECTOR.
    for (let i = 1; i <= 5; i++) {
      await t.data(
        `mutation { updateFs(where: { key: { eq: "f${i}" } }, update: { embedding: [1, ${i}] }) { info { nodesUpdated } } }`,
        {},
        viewer,
      );
    }
  });
  afterAll(() => t.close());

  const keys = (c: Edges) => c.edges.map((e) => e.node.key);

  test("the repro lists the matches by date", async () => {
    const d = await t.data<{ searchFsConnection: Edges }>(
      `{ searchFsConnection(query: "sun", sort: [{ startsOn: ASC }], first: 3) {
          edges { cursor score node { key } } pageInfo { hasNextPage } totalCount } }`,
      {},
      viewer,
    );
    expect(keys(d.searchFsConnection)).toEqual(["f5", "f4", "f3"]);
    expect(d.searchFsConnection.pageInfo.hasNextPage).toBe(true);
    expect(d.searchFsConnection.totalCount).toBe(5);
    for (const e of d.searchFsConnection.edges) {
      expect(e.score).toBeGreaterThan(0);
    }

    const after = d.searchFsConnection.edges.at(-1)!.cursor;
    const next = await t.data<{ searchFsConnection: Edges }>(
      `query($after: String) { searchFsConnection(query: "sun", sort: [{ startsOn: ASC }], first: 3, after: $after) {
          edges { cursor node { key } } pageInfo { hasNextPage } totalCount } }`,
      { after },
      viewer,
    );
    expect(keys(next.searchFsConnection)).toEqual(["f2", "f1"]);
    expect(next.searchFsConnection.pageInfo.hasNextPage).toBe(false);
    expect(next.searchFsConnection.totalCount).toBe(5);
  });

  test("ties fall back to the key, in both directions and across pages", async () => {
    const all: string[] = [];
    let after: string | null = null;
    for (;;) {
      const d: { searchFsConnection: Edges } = await t.data(
        `query($after: String) { searchFsConnection(query: "sun", sort: [{ rank: DESC }, { startsOn: DESC }], first: 2, after: $after) {
            edges { cursor node { key } } pageInfo { hasNextPage } } }`,
        { after },
        viewer,
      );
      all.push(...keys(d.searchFsConnection));
      if (!d.searchFsConnection.pageInfo.hasNextPage) break;
      after = d.searchFsConnection.edges.at(-1)!.cursor;
    }
    // rank 1: f1, f3, f5 (startsOn 05, 03, 01 descending); rank 0: f2, f4.
    expect(all).toEqual(["f1", "f3", "f5", "f2", "f4"]);
  });

  test("the list root takes it too", async () => {
    const d = await t.data<{
      searchFs: Array<{ score: number; node: { key: string } }>;
    }>(
      `{ searchFs(query: "sun", sort: [{ startsOn: DESC }], limit: 2) { score node { key } } }`,
      {},
      viewer,
    );
    expect(d.searchFs.map((m) => m.node.key)).toEqual(["f1", "f2"]);
  });

  test("without sort, score order is unchanged", async () => {
    const d = await t.data<{ searchFsConnection: Edges }>(
      `{ searchFsConnection(query: "sun", first: 5) { edges { score node { key } } } }`,
      {},
      viewer,
    );
    const scores = d.searchFsConnection.edges.map((e) => e.score);
    expect([...scores].sort((a, b) => b - a)).toEqual(scores);
    expect(t.statements.at(-1)!.statement.text).toMatch(
      /ORDER BY score DESC, this\.key ASC/,
    );
  });

  test("a cursor of one order is refused in another", async () => {
    const d = await t.data<{ searchFsConnection: Edges }>(
      `{ searchFsConnection(query: "sun", first: 1) { edges { cursor } } }`,
      {},
      viewer,
    );
    const r = await t.run(
      `query($after: String) { searchFsConnection(query: "sun", sort: [{ startsOn: ASC }], first: 1, after: $after) { edges { cursor } } }`,
      { after: d.searchFsConnection.edges[0]!.cursor },
      viewer,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("INVALID_CURSOR");
  });

  test("a vector search sorts its candidates", async () => {
    const d = await t.data<{ similarFsConnection: Edges }>(
      `{ similarFsConnection(vector: [1, 1], sort: [{ startsOn: ASC }], first: 2) {
          edges { node { key } } totalCount } }`,
      {},
      viewer,
    );
    expect(keys(d.similarFsConnection)).toEqual(["f5", "f4"]);
    expect(d.similarFsConnection.totalCount).toBe(5);
  });
});
