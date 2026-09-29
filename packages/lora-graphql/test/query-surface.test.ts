// Phase 14: query surface gaps against @neo4j/graphql.

import { describe, expect, test } from "vitest";
import { festivalHarness } from "./harness.js";
import { appTypeDefs, festivalTypeDefs } from "./fixtures.js";

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

describe("richer aggregates", () => {
  const typeDefs = /* GraphQL */ `
    type Band @node @query(aggregate: true) {
      key: String! @key
      name: String! @sortable
      set: Duration
      fans: [Fan!]! @relationship(type: "LIKES", direction: IN) @filterable
    }
    type Fan @node @query(aggregate: true) {
      key: String! @key
      nick: String! @sortable
      likes: [Band!]! @relationship(type: "LIKES", direction: OUT) @filterable
    }
  `;
  const seed = [
    `CREATE (:Band {key: 'b1', name: 'Blur', set: duration('PT1H')}),
            (:Band {key: 'b2', name: 'Pulp', set: duration('PT3H')}),
            (:Band {key: 'b3', name: 'The Verve', set: duration('PT2H')})`,
    `CREATE (:Fan {key: 'f1', nick: 'al'}), (:Fan {key: 'f2', nick: 'bea'}), (:Fan {key: 'f3', nick: 'cordelia'})`,
    `MATCH (f:Fan), (b:Band) WHERE b.key = 'b1' CREATE (f)-[:LIKES]->(b)`,
    // A parallel relationship: f1 likes b1 twice.
    `MATCH (f:Fan {key: 'f1'}), (b:Band {key: 'b1'}) CREATE (f)-[:LIKES]->(b)`,
    `MATCH (f:Fan {key: 'f2'}), (b:Band {key: 'b2'}) CREATE (f)-[:LIKES]->(b)`,
  ];

  test("strings: shortest and longest, at the root and through a relationship", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const d = await h.data<Record<string, unknown>>(`{
      bandsAggregate { name { shortest longest min } }
      bandsConnection { aggregate { node { name { shortest longest } } } }
      band(key: "b1") { fansConnection { aggregate { node { nick { shortest longest } } } } }
    }`);
    expect(d).toEqual({
      bandsAggregate: {
        name: { shortest: "Blur", longest: "The Verve", min: "Blur" },
      },
      bandsConnection: {
        aggregate: {
          node: { name: { shortest: "Blur", longest: "The Verve" } },
        },
      },
      band: {
        fansConnection: {
          aggregate: {
            node: { nick: { shortest: "al", longest: "cordelia" } },
          },
        },
      },
    });
  });

  test("durations: min, max, sum and avg (not LoraDB's max, see E25)", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const d = await h.data<Record<string, unknown>>(`{
      bandsAggregate { set { min max sum avg } }
      fan(key: "f1") { likesConnection { aggregate { node { set { max sum } } } } }
    }`);
    expect(d).toEqual({
      bandsAggregate: {
        set: { min: "PT1H", max: "PT3H", sum: "PT6H", avg: "PT2H" },
      },
      fan: {
        likesConnection: {
          aggregate: { node: { set: { max: "PT1H", sum: "PT2H" } } },
        },
      },
    });
  });

  test("count { nodes edges } tells parallel relationships apart", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const d = await h.data<{ band: unknown }>(
      `{ band(key: "b1") { fansConnection { aggregate { count { nodes edges } } } } }`,
    );
    expect(d.band).toEqual({
      fansConnection: { aggregate: { count: { nodes: 3, edges: 4 } } },
    });
  });

  test("filters on string length", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const keys = async (where: string) =>
      (
        await h.data<{ bands: Array<{ key: string }> }>(
          `{ bands(where: ${where}, sort: [{ name: ASC }]) { key } }`,
        )
      ).bands.map((b) => b.key);
    expect(
      await keys(
        `{ fans: { aggregate: { node: { nick: { longestLength: { gte: 5 } } } } } }`,
      ),
    ).toEqual(["b1"]);
    expect(
      await keys(
        `{ fans: { aggregate: { node: { nick: { shortestLength: { eq: 3 } } } } } }`,
      ),
    ).toEqual(["b2"]);
    expect(
      await keys(
        `{ fans: { aggregate: { node: { nick: { averageLength: { lt: 3.5 } } } } } }`,
      ),
    ).toEqual(["b2"]);
  });
});

describe("filter and sort on @cypher fields", () => {
  const typeDefs = appTypeDefs.replace(
    'followerCount: Int!\n      @cypher(statement: "RETURN size([(this)<-[:FOLLOWS]-(:User) | 1]) AS n")',
    'followerCount: Int!\n      @cypher(statement: "RETURN size([(this)<-[:FOLLOWS]-(:User) | 1]) AS n")\n      @filterable(byValue: [EQ, GT])\n      @sortable',
  );
  const seed = [
    "UNWIND range(1, 6) AS i CREATE (:Festival {key: 'f' + toString(i), name: 'F' + toString(i)})",
    "UNWIND range(1, 4) AS i CREATE (:User {key: 'u' + toString(i)})",
    // f1: 4 followers, f2: 3, f3: 2, f4: 1, f5 and f6: none.
    "MATCH (u:User), (f:Festival) WHERE toInteger(substring(f.key, 1)) <= 5 - toInteger(substring(u.key, 1)) CREATE (u)-[:FOLLOWS]->(f)",
  ];

  test("root lists filter and sort by the computed value", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const d = await h.data<{ festivals: unknown[] }>(`{
      festivals(where: { followerCount: { gt: 1 } }, sort: [{ followerCount: ASC }]) { key followerCount }
    }`);
    expect(d.festivals).toEqual([
      { key: "f3", followerCount: 2 },
      { key: "f2", followerCount: 3 },
      { key: "f1", followerCount: 4 },
    ]);
    const mixed = await h.data<{ festivals: Array<{ key: string }> }>(`{
      festivals(where: { OR: [{ followerCount: { eq: 0 } }, { name: { eq: "F1" } }] }) { key }
    }`);
    expect(mixed.festivals.map((f) => f.key).sort()).toEqual([
      "f1",
      "f5",
      "f6",
    ]);
  });

  test("connections page by the computed value", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const keys: string[] = [];
    let after: string | null = null;
    for (;;) {
      const d: {
        festivalsConnection: {
          edges: Array<{ node: { key: string } }>;
          pageInfo: { endCursor: string; hasNextPage: boolean };
        };
      } = await h.data(
        `query($after: String) { festivalsConnection(first: 4, after: $after, sort: [{ followerCount: DESC }]) {
            edges { node { key } } pageInfo { endCursor hasNextPage } } }`,
        { after },
      );
      keys.push(...d.festivalsConnection.edges.map((e) => e.node.key));
      if (!d.festivalsConnection.pageInfo.hasNextPage) break;
      after = d.festivalsConnection.pageInfo.endCursor;
    }
    expect(keys).toEqual(["f1", "f2", "f3", "f4", "f5", "f6"]);
  });

  test("through a relationship it is refused; the model warns it scans", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const r = await h.run(
      `{ users { follows(sort: [{ followerCount: DESC }]) { key } } }`,
    );
    expect(r.errors?.[0]?.message).toContain("sort by it in a root field only");
    expect(h.lora.model.warnings.map((w) => w.message)).toContain(
      "filtering or sorting by followerCount runs its statement for every Festival considered: no index applies",
    );
  });
});

describe("search results as connections", () => {
  const typeDefs = /* GraphQL */ `
    type Doc
      @node
      @fulltext(indexes: [{ fields: ["body"] }])
      @limit(default: 10, max: 10) {
      key: String! @key
      body: String!
      embedding: [Float!] @vector(dimensions: 2, similarity: COSINE)
    }
  `;
  const seed = [
    `UNWIND range(1, 12) AS i CREATE (:Doc {
       key: 'd' + right('0' + toString(i), 2),
       body: CASE WHEN i % 3 = 0 THEN 'lora lora lora' ELSE 'lora graph' END,
       embedding: [toFloat(i), toFloat(13 - i)] })`,
  ];

  const walk = async (
    h: Awaited<ReturnType<typeof festivalHarness>>,
    field: string,
    args: string,
  ) => {
    const pages: string[][] = [];
    let after: string | null = null;
    for (;;) {
      const d: Record<
        string,
        {
          edges: Array<{ node: { key: string }; score: number }>;
          pageInfo: { endCursor: string; hasNextPage: boolean };
        }
      > = await h.data(
        `query($after: String) { ${field}(${args}, first: 5, after: $after) {
            edges { score node { key } } pageInfo { endCursor hasNextPage } } }`,
        { after },
      );
      const conn = d[field]!;
      pages.push(conn.edges.map((e) => e.node.key));
      if (!conn.pageInfo.hasNextPage) break;
      after = conn.pageInfo.endCursor;
    }
    return pages;
  };

  test("full-text: every match once, in score order, page by page", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const list = await h.data<{ searchDocs: Array<{ node: { key: string } }> }>(
      `{ searchDocs(query: "lora", limit: 10) { node { key } } }`,
    );
    const pages = await walk(h, "searchDocsConnection", `query: "lora"`);
    expect(pages.map((p) => p.length)).toEqual([5, 5, 2]);
    expect(pages.flat().slice(0, 10)).toEqual(
      list.searchDocs.map((r) => r.node.key),
    );
    expect(new Set(pages.flat()).size).toBe(12);
  });

  test("vector: pages within the candidate pool", async () => {
    const h = await festivalHarness({
      typeDefs: typeDefs.replace("@node\n", "@node\n      @mutation\n"),
      seed: [],
    });
    const input = Array.from({ length: 12 }, (_, n) => {
      const i = n + 1;
      return `{ key: "d${String(i).padStart(2, "0")}", body: "x", embedding: [${i}, ${13 - i}] }`;
    });
    await h.data(
      `mutation { createDocs(input: [${input.join(", ")}]) { info { nodesCreated } } }`,
    );
    const pages = await walk(h, "similarDocsConnection", `vector: [1, 0]`);
    const all = pages.flat();
    expect(all).toHaveLength(12);
    expect(all[0]).toBe("d12");
  });
});

describe("nested delete", () => {
  const typeDefs = /* GraphQL */ `
    type Venue @node @mutation {
      key: String! @key
      stages: [Stage!]! @relationship(type: "HAS", direction: OUT)
      logo: Image @relationship(type: "SHOWS", direction: OUT)
    }
    type Stage @node @mutation {
      key: String! @key
      size: Int @filterable(byValue: [LT, GT])
      venue: Venue! @relationship(type: "HAS", direction: IN)
    }
    type Image @node @mutation {
      key: String! @key
    }
  `;
  const seed = [
    `CREATE (v:Venue {key: 'v1'}), (w:Venue {key: 'v2'}), (i:Image {key: 'i1'})
     CREATE (v)-[:SHOWS]->(i)
     WITH v, w UNWIND range(1, 4) AS n
     CREATE (v)-[:HAS]->(:Stage {key: 's' + toString(n), size: n})
     CREATE (w)-[:HAS]->(:Stage {key: 'w' + toString(n), size: n})`,
  ];

  test("deletes the connected nodes where matches, and only those", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const d = await h.data<{
      updateVenue: { info: { nodesDeleted: number } };
    }>(`mutation {
      updateVenue(key: "v1", update: { stages: { delete: { where: { size: { gt: 2 } } } } }) {
        info { nodesDeleted } venue { stages { key } }
      }
    }`);
    expect(d.updateVenue).toEqual({
      info: { nodesDeleted: 2 },
      venue: { stages: [{ key: "s1" }, { key: "s2" }] },
    });
    // The other venue's stages of the same size stay.
    const rows = await h.db.execute(
      "MATCH (s:Stage) WHERE s.size > 2 RETURN s.key AS k ORDER BY k",
    );
    expect(rows.rows.map((r) => r["k"])).toEqual(["w3", "w4"]);
  });

  test("bounded by limit; a single relationship takes true", async () => {
    const h = await festivalHarness({ typeDefs, seed });
    const over = await h.run(`mutation {
      updateVenue(key: "v1", update: { stages: { delete: { limit: 3 } } }) { info { nodesDeleted } }
    }`);
    expect(over.errors?.[0]?.extensions?.["code"]).toBe("LIMIT_EXCEEDED");
    const logo = await h.data<{
      updateVenue: { info: { nodesDeleted: number } };
    }>(`mutation {
      updateVenue(key: "v1", update: { logo: { delete: true } }) { info { nodesDeleted } }
    }`);
    expect(logo.updateVenue.info.nodesDeleted).toBe(1);
  });
});
