// An object type without @node may hold one node per field: a @cypher
// statement returns maps such as `{ event: e, score: s }`, and the node is
// projected like any node a statement returns, under its type's rules.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
}
type Person @node { key: String! @key }
type Venue @node { key: String! @key  name: String! }
type Event @node
  @authorization(filter: [{ operations: [READ], where: { node: { hidden: { eq: false } } } }]) {
  key: String! @key
  title: String!
  hidden: Boolean!
  venue: Venue @relationship(type: "AT", direction: OUT)
}
type Recommendation {
  event: Event!
  score: Float!
  rail: String
}
type Pick {
  event: Event
  note: String
}
type Query {
  recommendedEvents(rail: String = "taste"): [Recommendation!]!
    @authentication
    @cypher(statement: """
      MATCH (e:Event)
      RETURN { event: e, score: e.rank, rail: $rail } AS rec
      ORDER BY e.rank DESC
    """, columnName: "rec")
  topPick: Pick
    @authentication
    @cypher(statement: """
      MATCH (e:Event {key: 'e2'})
      RETURN { event: e, note: 'staff pick' } AS pick
    """, columnName: "pick")
}
type Mutation {
  noop: Boolean @authentication @cypher(statement: "RETURN true AS noop")
}
extend type Person {
  saved: [Recommendation!]!
    @cypher(statement: """
      MATCH (e:Event)
      WHERE e.key IN ['e1', 'e2']
      RETURN { event: e, score: 1.0 } AS rec
    """, columnName: "rec")
}`;
const seed = [
  "CREATE (:Person {key: 'ann'})",
  "CREATE (v:Venue {key: 'v1', name: 'Paradiso'})",
  "MATCH (v:Venue) CREATE (:Event {key: 'e1', title: 'Dub', hidden: false, rank: 0.9})-[:AT]->(v), (:Event {key: 'e2', title: 'Secret', hidden: true, rank: 0.8}), (:Event {key: 'e3', title: 'Folk', hidden: false, rank: 0.7})",
];
const ann = { jwt: { sub: "ann" } };

describe("node fields on an object type without @node", () => {
  test("project the node with its selection, aliases included", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      await t.data(
        '{ recommendedEvents(rail: "near") { score rail what: event { key venue { name } } e: event { title } } }',
        {},
        ann,
      ),
    ).toEqual({
      recommendedEvents: [
        {
          score: 0.9,
          rail: "near",
          what: { key: "e1", venue: { name: "Paradiso" } },
          e: { title: "Dub" },
        },
        // e2 is hidden: its row is dropped, the ranking goes on.
        {
          score: 0.7,
          rail: "near",
          what: { key: "e3", venue: null },
          e: { title: "Folk" },
        },
      ],
    });
  });

  test("a nullable node field reads null when the node is hidden", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(await t.data("{ topPick { note event { key } } }", {}, ann)).toEqual(
      { topPick: { note: "staff pick", event: null } },
    );
  });

  test("works on a @cypher field of a node type", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      await t.data(
        '{ person(key: "ann") { saved { score event { key } } } }',
        {},
        ann,
      ),
    ).toEqual({
      person: { saved: [{ score: 1, event: { key: "e1" } }] },
    });
  });

  test("refuses a list of nodes and a @customResolver returning one", () => {
    let problems: string[] = [];
    try {
      buildModel(`type Event @node { key: String! @key }
type Bag { events: [Event!]! }
type Rec { event: Event! }
type Query {
  bag: Bag @cypher(statement: "RETURN {events: []} AS b", columnName: "b")
  rec: Rec @customResolver
}`);
    } catch (err) {
      if (!(err instanceof ModelError)) throw err;
      problems = err.problems.map(
        (p) => `${p.type}${p.field ? `.${p.field}` : ""}: ${p.message}`,
      );
    }
    expect(problems).toEqual(
      expect.arrayContaining([
        "Bag.events: a field of an object type without @node holds one node, not a list; return one row per node",
        "Query.rec: Rec holds nodes, which only a @cypher statement can return: their read rules are compiled into it",
      ]),
    );
  });
});
