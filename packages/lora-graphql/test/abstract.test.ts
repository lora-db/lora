import { festivalHarness, type Harness } from "./harness.js";

export const abstractTypeDefs = /* GraphQL */ `
  interface Event @limit(default: 10, max: 50) {
    key: String!
    title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
    starts: Int @sortable
  }
  type Concert implements Event @node @mutation {
    key: String! @key
    title: String!
    starts: Int
    band: String
  }
  type Exhibition implements Event @node @mutation {
    key: String! @key
    title: String!
    starts: Int
    artist: String
  }
  union Headline = Concert | Exhibition
  type Venue @node @mutation {
    key: String! @key
    events: [Event!]! @relationship(type: "HOSTS", direction: OUT) @filterable
    headline: Headline @relationship(type: "HEADLINES", direction: OUT)
  }
`;

const seed = [
  `CREATE (c1:Concert {key: 'c1', title: 'Rock night', starts: 20, band: 'Blur'}),
          (c2:Concert {key: 'c2', title: 'Jazz brunch', starts: 11, band: 'Trio'}),
          (e1:Exhibition {key: 'e1', title: 'Modern art', starts: 10, artist: 'Ann'}),
          (e2:Exhibition {key: 'e2', title: 'Rock photos', starts: 15, artist: 'Bo'}),
          (v:Venue {key: 'v1'}), (w:Venue {key: 'v2'})
   CREATE (v)-[:HOSTS]->(c1), (v)-[:HOSTS]->(e1), (v)-[:HOSTS]->(e2), (w)-[:HOSTS]->(c2),
          (v)-[:HEADLINES]->(e2)`,
];

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness({ typeDefs: abstractTypeDefs, seed });
});

test("root interface list: every implementation, filtered, sorted, merged", async () => {
  const d = await h.data<{ events: unknown[] }>(`{
    events(sort: [{ starts: ASC }], limit: 3) {
      __typename key title
      ... on Concert { band }
      ... on Exhibition { artist }
    }
  }`);
  expect(d.events).toEqual([
    { __typename: "Exhibition", key: "e1", title: "Modern art", artist: "Ann" },
    { __typename: "Concert", key: "c2", title: "Jazz brunch", band: "Trio" },
    { __typename: "Exhibition", key: "e2", title: "Rock photos", artist: "Bo" },
  ]);
  const rock = await h.data<{ events: Array<{ key: string }> }>(
    `{ events(where: { title: { contains: "Rock" } }, sort: [{ title: ASC }]) { key } }`,
  );
  expect(rock.events.map((e) => e.key)).toEqual(["c1", "e2"]);
  const typed = await h.data<{ events: Array<{ key: string }> }>(
    `{ events(where: { typename: [Concert] }) { key } }`,
  );
  expect(typed.events.map((e) => e.key)).toEqual(["c1", "c2"]);
});

test("root union list with per-member where", async () => {
  const d = await h.data<{ headlines: Array<{ __typename: string }> }>(`{
    headlines(where: { Exhibition: { title: { eq: "Modern art" } } }) {
      __typename
      ... on Exhibition { key }
    }
  }`);
  expect(d.headlines).toEqual([{ __typename: "Exhibition", key: "e1" }]);
});

test("nested polymorphic lists and single fields", async () => {
  const d = await h.data<{ venue: unknown }>(`{
    venue(key: "v1") {
      events(sort: [{ title: DESC }]) { __typename key }
      headline { __typename ... on Exhibition { title } }
    }
  }`);
  expect(d.venue).toEqual({
    events: [
      { __typename: "Exhibition", key: "e2" },
      { __typename: "Concert", key: "c1" },
      { __typename: "Exhibition", key: "e1" },
    ],
    headline: { __typename: "Exhibition", title: "Rock photos" },
  });
});

test("relationship filters through an interface", async () => {
  const d = await h.data<{ venues: Array<{ key: string }> }>(`{
    venues(where: { events: { some: { title: { contains: "Jazz" } } } }) { key }
  }`);
  expect(d.venues).toEqual([{ key: "v2" }]);
  const only = await h.data<{ venues: Array<{ key: string }> }>(`{
    venues(where: { events: { count: { eq: 3 } } }) { key }
  }`);
  expect(only.venues).toEqual([{ key: "v1" }]);
});

test("interface filters and sorts become indexes on every implementation", () => {
  const reqs = h.lora
    .requirements()
    .filter((r) => r.kind === "index")
    .map((r) =>
      r.kind === "index" ? `${r.index} ${r.label}.${r.property}` : "",
    );
  expect(reqs).toEqual(
    expect.arrayContaining([
      "TEXT Concert.title",
      "TEXT Exhibition.title",
      "RANGE Concert.starts",
      "RANGE Exhibition.starts",
    ]),
  );
});

describe("mutations through interfaces and unions", () => {
  let m: Harness;
  beforeEach(async () => {
    m = await festivalHarness({ typeDefs: abstractTypeDefs, seed });
  });

  test("connect and create per member", async () => {
    const d = await m.data<{ createVenues: unknown }>(`mutation {
      createVenues(input: [{
        key: "v3"
        events: {
          connect: { Concert: [{ key: "c1" }] }
          create: { Exhibition: [{ node: { key: "e9", title: "New show" } }] }
        }
        headline: { connect: { Concert: { key: "c2" } } }
      }]) {
        venues { events(sort: [{ title: ASC }]) { __typename key } headline { __typename ... on Concert { key } } }
        info { nodesCreated relationshipsCreated }
      }
    }`);
    expect(d.createVenues).toEqual({
      venues: [
        {
          events: [
            { __typename: "Exhibition", key: "e9" },
            { __typename: "Concert", key: "c1" },
          ],
          headline: { __typename: "Concert", key: "c2" },
        },
      ],
      info: { nodesCreated: 2, relationshipsCreated: 3 },
    });
  });

  test("a single union relationship is replaced across members", async () => {
    await m.data(
      `mutation { updateVenue(key: "v1", update: { headline: { connect: { Concert: { key: "c1" } } } }) { info { relationshipsCreated } } }`,
    );
    const d = await m.data<{ venue: unknown }>(
      `{ venue(key: "v1") { headline { __typename ... on Concert { key } } } }`,
    );
    expect(d.venue).toEqual({ headline: { __typename: "Concert", key: "c1" } });
    const two = await m.run(
      `mutation { updateVenue(key: "v1", update: { headline: { connect: { Concert: { key: "c1" }, Exhibition: { key: "e1" } } } }) { info { relationshipsCreated } } }`,
    );
    expect(two.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  });

  test("disconnect per member, and exact write-sets", async () => {
    const changes: Array<{
      disconnected: Array<{ to: { type: string; key: unknown } }>;
    }> = [];
    m.lora.onWrite((c) => changes.push(c));
    await m.data(
      `mutation { updateVenue(key: "v1", update: { events: { disconnect: { Exhibition: ["e1"] } } }) { info { relationshipsDeleted } } }`,
    );
    expect(changes[0]!.disconnected.map((r) => r.to)).toEqual([
      { type: "Exhibition", key: "e1" },
    ]);
    const d = await m.data<{ venue: { events: unknown[] } }>(
      `{ venue(key: "v1") { events { key } } }`,
    );
    expect(d.venue.events).toHaveLength(2);
  });
});

test("each member of an interface list seeks through its own index", async () => {
  const [f] = await h.lora.explain(
    `{ events(where: { title: { contains: "Rock" } }) { key } }`,
  );
  const ops = f!.reports[0]!.operators;
  expect(ops.filter((o) => o === "NodeByTextScan")).toHaveLength(2);
  expect(ops).not.toContain("NodeByLabelScan");
});

test("typename inside AND / OR / NOT is a constant per member", async () => {
  const keys = async (where: string) =>
    (
      await h.data<{ events: Array<{ key: string }> }>(
        `{ events(where: ${where}, sort: [{ title: ASC }]) { key } }`,
      )
    ).events.map((e) => e.key);
  expect(
    await keys(
      `{ OR: [{ typename: [Concert] }, { title: { eq: "Modern art" } }] }`,
    ),
  ).toEqual(["c2", "e1", "c1"]);
  expect(await keys(`{ NOT: { typename: [Concert] } }`)).toEqual(["e1", "e2"]);
  expect(
    await keys(
      `{ AND: [{ typename: [Exhibition] }, { title: { contains: "Rock" } }] }`,
    ),
  ).toEqual(["e2"]);
});
