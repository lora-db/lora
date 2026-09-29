// Phase 14: query surface gaps against @neo4j/graphql.

import { describe, expect, test } from "vitest";
import { festivalHarness } from "./harness.js";
import { festivalTypeDefs } from "./fixtures.js";

const sortableFollows = festivalTypeDefs.replace(
  "since: Int! @filterable",
  "since: Int! @sortable @filterable",
);

describe("sort connections by edge properties", () => {
  test("pages forward and backward by an edge property", async () => {
    const h = await festivalHarness({ typeDefs: sortableFollows });
    // User u0 follows f0..f4 (since 2000..2004) and f10, f20.
    const page = async (args: string) =>
      (
        await h.data<{
          user: {
            followsConnection: {
              edges: Array<{
                cursor: string;
                properties: { since: number };
                node: { key: string };
              }>;
              pageInfo: {
                endCursor: string;
                startCursor: string;
                hasNextPage: boolean;
              };
            };
          };
        }>(`{ user(key: "u0") { followsConnection(${args}) {
          edges { cursor properties { since } node { key } }
          pageInfo { startCursor endCursor hasNextPage }
        } } }`)
      ).user.followsConnection;

    const sort = `sort: [{ edge: { since: DESC } }]`;
    const seen: number[] = [];
    let after = "";
    for (;;) {
      const p = await page(
        `first: 3, ${sort}${after ? `, after: "${after}"` : ""}`,
      );
      seen.push(...p.edges.map((e) => e.properties.since));
      if (!p.pageInfo.hasNextPage) break;
      after = p.pageInfo.endCursor;
    }
    expect(seen).toEqual([...seen].sort((a, b) => b - a));
    expect(seen).toHaveLength(7);

    const last = await page(`last: 2, before: "${after}", ${sort}`);
    const all = await page(`first: 20, ${sort}`);
    const i = all.edges.findIndex((e) => e.cursor === after);
    expect(last.edges.map((e) => e.node.key)).toEqual(
      all.edges.slice(i - 2, i).map((e) => e.node.key),
    );
  });

  test("an edge key mixes with node keys; a cursor is tied to its sort", async () => {
    const h = await festivalHarness({ typeDefs: sortableFollows });
    const r = await h.data<{
      user: {
        followsConnection: {
          edges: Array<{ node: { name: string } }>;
          pageInfo: { endCursor: string };
        };
      };
    }>(
      `{ user(key: "u0") { followsConnection(first: 2, sort: [{ edge: { since: ASC } }, { name: DESC }]) {
        edges { node { name } } pageInfo { endCursor } } } }`,
    );
    expect(r.user.followsConnection.edges.map((e) => e.node.name)).toEqual([
      "Sunland 0",
      "Moonfest 1",
    ]);
    const replay = await h.run(
      `{ user(key: "u0") { followsConnection(first: 2, after: "${r.user.followsConnection.pageInfo.endCursor}", sort: [{ name: ASC }]) { edges { node { key } } } } }`,
    );
    expect(replay.errors?.[0]?.extensions?.["code"]).toBe("INVALID_CURSOR");
  });

  test("the schema offers edge sort only where properties are sortable", async () => {
    const h = await festivalHarness({ typeDefs: sortableFollows });
    const sdl = h.lora.printPublicSchema();
    expect(sdl).toContain("input FollowsSort {\n  since: SortDirection\n}");
    expect(sdl).toMatch(
      /input UserFollowsConnectionSort \{[^}]*edge: FollowsSort/,
    );
    const plain = await festivalHarness();
    expect(plain.lora.printPublicSchema()).not.toContain("FollowsSort");
  });
});

describe("relationship fields on interfaces", () => {
  const typeDefs = /* GraphQL */ `
    interface Event {
      key: String!
      title: String! @sortable
      venue: Venue @declareRelationship
    }
    type Concert implements Event @node {
      key: String! @key
      title: String!
      venue: Venue @relationship(type: "PLAYS_AT", direction: OUT)
    }
    type Exhibition implements Event @node {
      key: String! @key
      title: String!
      venue: Venue @relationship(type: "SHOWN_AT", direction: OUT)
    }
    type Venue @node {
      key: String! @key
      name: String!
    }
  `;
  const seed = [
    `CREATE (v1:Venue {key: 'v1', name: 'Hall'}), (v2:Venue {key: 'v2', name: 'Gallery'}),
            (c:Concert {key: 'c1', title: 'A'}), (e:Exhibition {key: 'e1', title: 'B'}),
            (e2:Exhibition {key: 'e2', title: 'C'})
     CREATE (c)-[:PLAYS_AT]->(v1), (e)-[:SHOWN_AT]->(v2)`,
  ];

  test("selects a declared relationship at interface level", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const d = await h.data<{ events: unknown[] }>(
      `{ events(sort: [{ title: ASC }]) { key venue { name } } }`,
    );
    expect(d.events).toEqual([
      { key: "c1", venue: { name: "Hall" } },
      { key: "e1", venue: { name: "Gallery" } },
      { key: "e2", venue: null },
    ]);
  });

  test("every implementation must declare it the same way", async () => {
    const broken = typeDefs.replace(
      'venue: Venue @relationship(type: "SHOWN_AT", direction: OUT)',
      "venue: Venue",
    );
    await expect(
      festivalHarness({ typeDefs: broken, seed: [] }),
    ).rejects.toThrow(
      "Exhibition implements Event but venue on it is not a @relationship field",
    );
  });
});

describe("nestedOperations and aggregate: false", () => {
  const typeDefs = /* GraphQL */ `
    type Venue @node @mutation @query(aggregate: true) {
      key: String! @key
      name: String!
      stages: [Stage!]!
        @relationship(
          type: "HAS"
          direction: OUT
          nestedOperations: [CONNECT]
          aggregate: false
        )
        @filterable
      staff: [Person!]!
        @relationship(type: "WORKS_AT", direction: IN, nestedOperations: [])
    }
    type Stage @node @mutation @query(aggregate: true) {
      key: String! @key
      capacity: Int @filterable(byValue: [GT])
    }
    type Person @node @mutation {
      key: String! @key
    }
  `;

  test("the schema offers only the listed nested writes, and no aggregates", async () => {
    const h = await festivalHarness({ typeDefs, seed: [] });
    const sdl = h.lora.printPublicSchema();
    const block = (name: string) =>
      sdl.match(new RegExp(`input ${name} \\{[^}]*\\}`))?.[0] ?? "";
    expect(block("VenueStagesCreateRelationInput")).toMatch(/connect:/);
    expect(block("VenueStagesCreateRelationInput")).not.toMatch(/create:/);
    expect(block("VenueStagesUpdateRelationInput")).not.toMatch(/disconnect:/);
    expect(block("VenueCreateInput")).not.toMatch(/staff:/);
    expect(block("VenueUpdateInput")).not.toMatch(/staff:/);
    expect(sdl).toMatch(/stagesConnection\([^)]*\): VenueStagesConnection!/);
    const conn = sdl.match(/type VenueStagesConnection \{[^}]*\}/)![0];
    expect(conn).not.toContain("aggregate");
    // The relation filter keeps some / all / none / count, not aggregate.
    const filters = [
      ...sdl.matchAll(/input (\w+) \{[^}]*some: StageWhere[^}]*\}/g),
    ];
    expect(filters.length).toBeGreaterThan(0);
    for (const f of filters) expect(f[0]).not.toContain("aggregate:");
  });

  test("the trimmed connection still pages and counts", async () => {
    const h = await festivalHarness({
      typeDefs,
      seed: [
        "CREATE (v:Venue {key: 'v1', name: 'V'}) WITH v UNWIND range(1, 3) AS i CREATE (v)-[:HAS]->(:Stage {key: 's' + toString(i), capacity: i})",
      ],
    });
    const d = await h.data<{
      venue: { stagesConnection: { totalCount: number; edges: unknown[] } };
    }>(
      `{ venue(key: "v1") { stagesConnection(first: 2) { totalCount edges { node { key } } } } }`,
    );
    expect(d.venue.stagesConnection.totalCount).toBe(3);
    expect(d.venue.stagesConnection.edges).toHaveLength(2);
  });

  test("a required relationship must stay creatable", async () => {
    const broken = /* GraphQL */ `
      type A @node @mutation {
        key: String! @key
        b: B!
          @relationship(
            type: "R"
            direction: OUT
            nestedOperations: [DISCONNECT]
          )
      }
      type B @node {
        key: String! @key
      }
    `;
    await expect(
      festivalHarness({ typeDefs: broken, seed: [] }),
    ).rejects.toThrow("a required relationship needs CONNECT or CREATE");
  });
});
