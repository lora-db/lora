// `@fulltext(indexes: [{ minPrefixLength, maxMatches }])`: a broad query
// is bounded in the index, and its cost counts the matches it ranks.

import { describe, expect, test } from "vitest";
import { shortPrefixesWhole } from "../src/compile/read/search.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = (options: string) => `
  type Track @node @fulltext(indexes: [{ fields: ["title"]${options} }]) @limit(default: 5, max: 20) {
    key: String! @key
    title: String!
    hidden: Boolean! @filterable
  }`;
// 30 "Night <i>" titles, the first two twice as relevant, and two that
// only match as a whole two-letter word.
const seed = [
  "UNWIND range(1, 30) AS i CREATE (:Track {key: 't' + right('0' + toString(i), 2), title: CASE WHEN i <= 2 THEN 'Night night ' ELSE 'Night ' END + toString(i), hidden: i % 3 = 0})",
  "CREATE (:Track {key: 'u1', title: 'Ni'}), (:Track {key: 'u2', title: 'ni hao'})",
];

type Conn = {
  searchTracksConnection: {
    totalCount: number;
    edges: { node: { key: string }; cursor: string }[];
  };
};

describe("shortPrefixesWhole", () => {
  test("drops the * of words shorter than the minimum", () => {
    expect(shortPrefixesWhole("lo* love* l*", 3)).toBe("lo love* l");
    expect(shortPrefixesWhole("ab* cd", 2)).toBe("ab* cd");
    expect(shortPrefixesWhole("øy* ab*", 3)).toBe("øy ab");
    expect(shortPrefixesWhole("lo*", undefined)).toBe("lo*");
  });
});

describe("minPrefixLength", () => {
  test("a short * word matches whole; a long one stays a prefix", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(", minPrefixLength: 3"),
      seed,
    });
    const short = await t.data<Conn>(
      `{ searchTracksConnection(query: "ni*", first: 20) { totalCount edges { node { key } } } }`,
    );
    expect(short.searchTracksConnection.totalCount).toBe(2);
    expect(
      short.searchTracksConnection.edges.map((e) => e.node.key).sort(),
    ).toEqual(["u1", "u2"]);
    const long = await t.data<Conn>(
      `{ searchTracksConnection(query: "nig*") { totalCount } }`,
    );
    expect(long.searchTracksConnection.totalCount).toBe(30);
  });
});

describe("maxMatches", () => {
  test("ranks only the best matches; where, pages and totalCount apply within them", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(", maxMatches: 10"),
      seed,
    });
    t.statements.length = 0;
    const all = await t.data<Conn>(
      `{ searchTracksConnection(query: "night", first: 20) { totalCount edges { node { key } cursor } } }`,
    );
    const texts = t.statements.map((s) => s.statement.text).join("\n");
    expect(texts).toContain("queryNodes($p0, $p1, { `limit`: $p2 })");
    expect(all.searchTracksConnection.totalCount).toBe(10);
    const keys = all.searchTracksConnection.edges.map((e) => e.node.key);
    // The best by score first: the two "Night night" titles.
    expect(keys.slice(0, 2)).toEqual(["t01", "t02"]);
    expect(keys).toHaveLength(10);

    // Pages walk the same ten.
    const first = await t.data<Conn>(
      `{ searchTracksConnection(query: "night", first: 6) { edges { node { key } cursor } } }`,
    );
    const after = first.searchTracksConnection.edges.at(-1)!.cursor;
    const second = await t.data<Conn>(
      `query($a: String) { searchTracksConnection(query: "night", first: 6, after: $a) { edges { node { key } } } }`,
      { a: after },
    );
    expect([
      ...first.searchTracksConnection.edges.map((e) => e.node.key),
      ...second.searchTracksConnection.edges.map((e) => e.node.key),
    ]).toEqual(keys);

    // `where` filters within the ten ranked.
    const visible = await t.data<Conn>(
      `{ searchTracksConnection(query: "night", where: { hidden: { eq: false } }) { totalCount } }`,
    );
    expect(visible.searchTracksConnection.totalCount).toBe(
      keys.filter((k) => Number(k.slice(1)) % 3 !== 0).length,
    );

    // The list field is bounded the same way.
    const list = await t.data<{ searchTracks: { node: { key: string } }[] }>(
      `{ searchTracks(query: "night", limit: 20) { node { key } } }`,
    );
    expect(list.searchTracks.map((m) => m.node.key)).toEqual(keys);
  });

  test("the model refuses a bound below 1", async () => {
    await expect(
      createTestLoraGraphQL({ typeDefs: typeDefs(", maxMatches: 0"), seed }),
    ).rejects.toThrow(/maxMatches must be at least 1/);
    await expect(
      createTestLoraGraphQL({
        typeDefs: typeDefs(", minPrefixLength: 0"),
        seed,
      }),
    ).rejects.toThrow(/minPrefixLength must be at least 1/);
  });
});

describe("search cost", () => {
  const cost = (
    t: Awaited<ReturnType<typeof createTestLoraGraphQL>>,
    source: string,
  ) => t.lora.compile(source).reduce((sum, f) => sum + f.compiled.cost, 0);

  test("counts the label's nodes as candidates, or maxMatches when set", async () => {
    const open = await createTestLoraGraphQL({ typeDefs: typeDefs(""), seed });
    open.lora.useStatistics({ nodes: { Track: 1_000_000 }, degrees: {} });
    const page = cost(
      open,
      `{ searchTracks(query: "x", limit: 5) { node { key } } }`,
    );
    expect(page).toBeGreaterThanOrEqual(1_000_000);
    const counted = cost(
      open,
      `{ searchTracksConnection(query: "x") { totalCount } }`,
    );
    expect(counted).toBeGreaterThanOrEqual(1_000_000);

    const bounded = await createTestLoraGraphQL({
      typeDefs: typeDefs(", maxMatches: 1000"),
      seed,
    });
    bounded.lora.useStatistics({ nodes: { Track: 1_000_000 }, degrees: {} });
    const capped = cost(
      bounded,
      `{ searchTracksConnection(query: "x", first: 5) { totalCount edges { node { key } } } }`,
    );
    // The page and the count each rank at most 1000.
    expect(capped).toBeGreaterThanOrEqual(2000);
    expect(capped).toBeLessThan(2100);
  });
});
