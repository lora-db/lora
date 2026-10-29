// G-77: `@sortable(nulls: LAST)` puts a nullable field's nulls last whichever
// the direction, so `popularityScore: DESC` leads with the biggest score
// instead of every unscored artist. The page walks the RANGE index in order
// and then the nulls (no sort of the label), and cursors page through the
// scored values, their ties and the nulls in both directions.

import { describe, expect, test } from "vitest";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Artist @node {
    key: ID! @key
    name: String!
    popularityScore: Float @sortable(nulls: LAST)
    rating: Float @sortable
  }
`;

interface Node {
  key: string;
  popularityScore: number | null;
}
interface Page {
  artistsConnection: {
    edges: Array<{ node: Node }>;
    pageInfo: {
      hasNextPage: boolean;
      hasPreviousPage: boolean;
      startCursor: string | null;
      endCursor: string | null;
    };
  };
}

const PAGE = /* GraphQL */ `
  query Page($first: Int, $after: String, $last: Int, $before: String) {
    artistsConnection(
      first: $first
      after: $after
      last: $last
      before: $before
      sort: [{ popularityScore: DESC }]
    ) {
      edges {
        node {
          key
          popularityScore
        }
      }
      pageInfo {
        hasNextPage
        hasPreviousPage
        startCursor
        endCursor
      }
    }
  }
`;

const keys = (p: Page) => p.artistsConnection.edges.map((e) => e.node.key);

describe("G-77: @sortable(nulls:)", () => {
  test("the reproduction answers 90, 10, null", async () => {
    const h = await festivalHarness({
      typeDefs,
      seed: [
        `CREATE (:Artist {key: 'a', name: 'A', popularityScore: 10.0, rating: 10.0}),
                (:Artist {key: 'b', name: 'B'}),
                (:Artist {key: 'c', name: 'C', popularityScore: 90.0, rating: 90.0})`,
      ],
    });
    const scores = async (sort: string) => {
      const r = await h.data<{
        artistsConnection: { edges: Array<{ node: Node }> };
      }>(
        `{ artistsConnection(first: 3, sort: [${sort}]) { edges { node { key popularityScore } } } }`,
      );
      return r.artistsConnection.edges.map((e) => e.node.popularityScore);
    };
    expect(await scores("{ popularityScore: DESC }")).toEqual([90, 10, null]);
    expect(await scores("{ popularityScore: ASC }")).toEqual([10, 90, null]);
    // Without nulls: Cypher's order, nulls first descending.
    const rating = await h.data<{ artists: Array<{ key: string }> }>(
      `{ artists(sort: [{ rating: DESC }]) { key } }`,
    );
    expect(rating.artists.map((a) => a.key)).toEqual(["b", "c", "a"]);
  });

  test("cursors page through values, ties and nulls both ways", async () => {
    // 60 artists: 12 unscored, the rest on 9 scores, so runs of ties.
    const h = await festivalHarness({
      typeDefs,
      seed: [
        `UNWIND range(1, 60) AS i
         CREATE (:Artist {key: 'a' + right('00' + toString((i * 37) % 61), 2), name: 'n',
           popularityScore: CASE WHEN i % 5 = 0 THEN null ELSE toFloat(i % 9) * 10.0 END})`,
      ],
    });
    const all = (
      await h.data<{ artists: Node[] }>(
        `{ artists(limit: 100) { key popularityScore } }`,
      )
    ).artists;
    const expected = [...all]
      .sort((x, y) => {
        if (x.popularityScore === null || y.popularityScore === null) {
          if (x.popularityScore === y.popularityScore)
            return x.key < y.key ? -1 : 1;
          return x.popularityScore === null ? 1 : -1;
        }
        return (
          y.popularityScore - x.popularityScore || (x.key < y.key ? -1 : 1)
        );
      })
      .map((n) => n.key);
    expect(expected).toHaveLength(60);

    for (const size of [1, 7, 24]) {
      // Forward.
      const forward: string[] = [];
      let after: string | null = null;
      for (;;) {
        const p: Page = await h.data<Page>(PAGE, { first: size, after });
        forward.push(...keys(p));
        if (!p.artistsConnection.pageInfo.hasNextPage) break;
        after = p.artistsConnection.pageInfo.endCursor;
      }
      expect(forward).toEqual(expected);
      // Backward.
      const backward: string[] = [];
      let before: string | null = null;
      for (;;) {
        const p: Page = await h.data<Page>(PAGE, { last: size, before });
        backward.unshift(...keys(p));
        if (!p.artistsConnection.pageInfo.hasPreviousPage) break;
        before = p.artistsConnection.pageInfo.startCursor;
      }
      expect(backward).toEqual(expected);
    }
  });

  test("a page walks the index, then the nulls, with no sort of the label", async () => {
    const h = await festivalHarness({
      typeDefs,
      seed: [
        `UNWIND range(1, 500) AS i
         CREATE (:Artist {key: 'a' + right('000' + toString(i), 3), name: 'n',
           popularityScore: CASE WHEN i % 4 = 0 THEN null ELSE toFloat(i % 50) END})`,
      ],
    });
    const plan = async (variables: Record<string, unknown>) => {
      const [explained] = await h.lora.explain(PAGE, variables);
      return explained!.reports[0]!;
    };
    const first = await plan({ first: 24 });
    expect(first.statement.text).toContain(
      "ORDER BY this.popularityScore DESC NULLS LAST, this.key ASC",
    );
    expect(first.findings).toEqual([]);
    expect(first.operators).toContain("PartialSort");
    expect(first.operators).toContain("NodeByPropertyRangeScan");
    expect(first.operators).not.toContain("Sort");

    // After a scored cursor: the range from the cursor, then the nulls.
    const p = await h.data<Page>(PAGE, { first: 24 });
    const after = p.artistsConnection.pageInfo.endCursor;
    const second = await plan({ first: 24, after });
    expect(second.findings).toEqual([]);
    expect(second.operators).toContain("PartialSort");
    expect(second.operators).not.toContain("Sort");
    // Backward from a scored cursor: an ordered range walk too.
    const back = await plan({ last: 24, before: after });
    expect(back.findings).toEqual([]);
    expect(back.operators).not.toContain("Sort");
  });
});

/** A 35,000-artist hub, a page of 24: as cheap as a page by key. */
describe("G-77: cost", () => {
  const time = async (h: Harness, query: string, runs = 30) => {
    await h.data(query);
    const t = performance.now();
    for (let i = 0; i < runs; i++) await h.data(query);
    return (performance.now() - t) / runs;
  };

  test("a nulls-last DESC page of 24 over 35k artists costs about a key page", async () => {
    const h = await festivalHarness({
      typeDefs,
      seed: [
        `UNWIND range(0, 34999) AS i WITH (i * 7919) % 35000 AS j
         CREATE (:Artist {key: 'a' + right('00000' + toString(j), 5), name: 'n',
           popularityScore: CASE WHEN j % 5 = 0 THEN null ELSE toFloat(j) / 7.0 END,
           rating: CASE WHEN j % 5 = 0 THEN null ELSE toFloat(j) / 7.0 END})`,
      ],
    });
    const page = (sort: string) =>
      `{ artistsConnection(first: 24, sort: [${sort}]) { edges { node { key } } } }`;
    const byKey = await time(h, page("{ key: ASC }"));
    const byScore = await time(h, page("{ popularityScore: DESC }"));
    // `rating` has no nulls setting: DESC puts its nulls first, which a
    // page can only find by reading the label (the cost before G-77).
    const byRating = await time(h, page("{ rating: DESC }"));
    console.log(
      `35k artists, page of 24: key ASC ${byKey.toFixed(3)} ms, popularityScore DESC NULLS LAST ${byScore.toFixed(3)} ms, rating DESC (nulls first, top-k) ${byRating.toFixed(3)} ms`,
    );
    expect(byScore).toBeLessThan(byKey * 4 + 1);
    const [explained] = await h.lora.explain(page("{ popularityScore: DESC }"));
    expect(explained!.reports[0]!.operators).not.toContain("Sort");
  });
});
