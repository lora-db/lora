// An object type without @node may hold other such types, one or a list
// (G-85): a preview carries its counts and its ranked list in one result.
// The inner maps are read as they stand, so the inner types hold no nodes.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

function problems(typeDefs: string): string[] {
  try {
    buildModel(typeDefs);
  } catch (err) {
    if (err instanceof ModelError) {
      return err.problems.map(
        (p) => `${p.type}${p.field ? `.${p.field}` : ""}: ${p.message}`,
      );
    }
    throw err;
  }
  return [];
}

const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}
type Person @node { key: String! @key }
type Event @node { key: String! @key  title: String!  rank: Float! }
type TasteRecommendation { eventKey: String!  score: Float!  reasons: [String!]! }
type TastePreview {
  matchCount: Int!
  recommendations: [TasteRecommendation!]!
  best: TasteRecommendation
}
type Ranked { event: Event!  detail: TasteRecommendation! }
type Tree { label: String!  children: [Tree!]! }
type Query {
  tastePreview(genres: [String!]!): TastePreview! @customResolver @authentication
  genreTree: Tree! @customResolver
  previewFromGraph: TastePreview!
    @cypher(statement: """
      MATCH (e:Event)
      WITH e ORDER BY e.rank DESC
      WITH collect({eventKey: e.key, score: e.rank, reasons: ['rank']}) AS recs
      RETURN {matchCount: size(recs), recommendations: recs, best: head(recs)} AS p
    """, columnName: "p")
  ranked: [Ranked!]!
    @cypher(statement: """
      MATCH (e:Event)
      RETURN {event: e, detail: {eventKey: e.key, score: e.rank, reasons: []}} AS r
      ORDER BY e.rank DESC
    """, columnName: "r")
}`;

const seed = [
  "CREATE (:Event {key: 'e1', title: 'Dub', rank: 0.9}), (:Event {key: 'e2', title: 'Folk', rank: 0.4})",
];

const resolvers = {
  Query: {
    tastePreview: (_: unknown, args: { genres: string[] }) => {
      const recommendations = args.genres.map((g, i) => ({
        eventKey: `e-${g}`,
        score: 1 - i / 10,
        reasons: [g],
      }));
      return {
        matchCount: recommendations.length,
        recommendations,
        best: recommendations[0] ?? null,
      };
    },
    genreTree: () => ({
      label: "music",
      children: [
        { label: "electronic", children: [{ label: "dub", children: [] }] },
        { label: "folk", children: [] },
      ],
    }),
  },
};

describe("an object type without @node holding other such types", () => {
  test("a root @customResolver returns the counts and the list in one result", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers, seed });
    const d = await t.data(
      `{ tastePreview(genres: ["dub", "folk"]) {
           matchCount
           recommendations { eventKey score reasons }
           top: best { key: eventKey }
         } }`,
      {},
      { jwt: { sub: "ann" } },
    );
    expect(d).toEqual({
      tastePreview: {
        matchCount: 2,
        recommendations: [
          { eventKey: "e-dub", score: 1, reasons: ["dub"] },
          { eventKey: "e-folk", score: 0.9, reasons: ["folk"] },
        ],
        top: { key: "e-dub" },
      },
    });
  });

  test("a type may hold itself", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers, seed });
    const d = await t.data(
      "{ genreTree { label children { label children { label } } } }",
    );
    expect(d).toEqual({
      genreTree: {
        label: "music",
        children: [
          { label: "electronic", children: [{ label: "dub" }] },
          { label: "folk", children: [] },
        ],
      },
    });
  });

  test("a @cypher statement may return the nested maps", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers, seed });
    const d = await t.data(
      "{ previewFromGraph { matchCount recommendations { eventKey score } best { eventKey } } }",
    );
    expect(d).toEqual({
      previewFromGraph: {
        matchCount: 2,
        recommendations: [
          { eventKey: "e1", score: 0.9 },
          { eventKey: "e2", score: 0.4 },
        ],
        best: { eventKey: "e1" },
      },
    });
  });

  test("beside a node field of the outer object", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, resolvers, seed });
    const d = await t.data(
      "{ ranked { event { title } info: detail { eventKey score } } }",
    );
    expect(d).toEqual({
      ranked: [
        { event: { title: "Dub" }, info: { eventKey: "e1", score: 0.9 } },
        { event: { title: "Folk" }, info: { eventKey: "e2", score: 0.4 } },
      ],
    });
  });

  test("refuses an inner type holding nodes, and nested lists", () => {
    expect(
      problems(`type Event @node { key: String! @key }
type Pick { event: Event! }
type Picks { all: [Pick!]!  one: Pick  grid: [[String!]!]! }
type Query {
  picks: Picks @cypher(statement: "RETURN {all: []} AS p", columnName: "p")
}`),
    ).toEqual(
      expect.arrayContaining([
        "Picks.all: Pick holds nodes, which are read only at the top of a @cypher result, not inside another object",
        "Picks.one: Pick holds nodes, which are read only at the top of a @cypher result, not inside another object",
        "Picks.grid: nested lists are not supported",
      ]),
    );
  });
});
