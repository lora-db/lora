import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Band @node {
    key: String! @key
    name: String! @filterable(byValue: [EQ, CONTAINS, CASE_INSENSITIVE])
    tags: [String!] @filterable
    founded: Int @filterable(byValue: [EQ, IS_NULL]) @sortable
    members: [Musician!]!
      @relationship(type: "PLAYS_IN", direction: IN, properties: "Membership")
      @filterable
    friends: [Band!]!
      @relationship(type: "FRIENDS", direction: OUT, queryDirection: UNDIRECTED)
  }
  type Musician @node {
    key: String! @key
    age: Int @sortable
  }
  type Membership @relationshipProperties {
    since: Int @filterable(byValue: [GTE, LT])
    role: String @filterable
  }
`;

const seed = [
  `CREATE (a:Band {key: 'a', name: 'The Abba', tags: ['pop', 'swe'], founded: 1972}),
          (b:Band {key: 'b', name: 'Blur', tags: ['rock']}),
          (c:Band {key: 'c', name: 'Coldplay', tags: ['pop'], founded: 1996})
   CREATE (m1:Musician {key: 'm1', age: 30}), (m2:Musician {key: 'm2', age: 50}), (m3:Musician {key: 'm3', age: 70})
   CREATE (m1)-[:PLAYS_IN {since: 1972, role: 'vocals'}]->(a), (m2)-[:PLAYS_IN {since: 1980, role: 'bass'}]->(a),
          (m2)-[:PLAYS_IN {since: 1990, role: 'bass'}]->(b), (m2)-[:PLAYS_IN {since: 1991, role: 'keys'}]->(b),
          (m3)-[:PLAYS_IN {since: 1996, role: 'drums'}]->(c)
   CREATE (a)-[:FRIENDS]->(b), (c)-[:FRIENDS]->(a)`,
];

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness({ typeDefs, seed });
});

const keys = async (where: string) =>
  (
    await h.data<{ bands: Array<{ key: string }> }>(
      `{ bands(where: ${where}) { key } }`,
    )
  ).bands
    .map((b) => b.key)
    .sort();

test("includes on list fields, isNull, case-insensitive strings", async () => {
  expect(await keys(`{ tags: { includes: "pop" } }`)).toEqual(["a", "c"]);
  expect(await keys(`{ founded: { isNull: true } }`)).toEqual(["b"]);
  expect(await keys(`{ founded: { isNull: false } }`)).toEqual(["a", "c"]);
  expect(
    await keys(`{ name: { caseInsensitive: { contains: "BL" } } }`),
  ).toEqual(["b"]);
  expect(
    await keys(`{ name: { caseInsensitive: { eq: "coldPLAY" } } }`),
  ).toEqual(["c"]);
});

test("connection filters test relationship properties in the parent where", async () => {
  expect(
    await keys(
      `{ membersConnection: { some: { edge: { role: { eq: "bass" } } } } }`,
    ),
  ).toEqual(["a", "b"]);
  expect(
    await keys(
      `{ membersConnection: { all: { edge: { since: { gte: 1980 } } } } }`,
    ),
  ).toEqual(["b", "c"]);
  expect(
    await keys(
      `{ membersConnection: { some: { node: { key: { eq: "m2" } }, edge: { since: { lt: 1985 } } } } }`,
    ),
  ).toEqual(["a"]);
});

test("aggregate filters over related nodes and relationship properties", async () => {
  expect(
    await keys(
      `{ members: { aggregate: { node: { age: { avg: { gt: 45 } } } } } }`,
    ),
  ).toEqual(["b", "c"]);
  expect(
    await keys(
      `{ members: { aggregate: { node: { age: { min: { lt: 40 } } } } } }`,
    ),
  ).toEqual(["a"]);
  expect(
    await keys(
      `{ members: { aggregate: { edge: { since: { max: { gte: 1991 } } } } } }`,
    ),
  ).toEqual(["b", "c"]);
});

test("counts and single count related nodes, not parallel relationships", async () => {
  // m2 plays in b twice (bass and keys): still one member.
  expect(await keys(`{ members: { count: { eq: 1 } } }`)).toEqual(["b", "c"]);
  expect(await keys(`{ members: { single: { key: { eq: "m2" } } } }`)).toEqual([
    "a",
    "b",
  ]);
});

test("UNDIRECTED relationships read both ways", async () => {
  const d = await h.data<{ band: { friends: Array<{ key: string }> } }>(
    `{ band(key: "a") { friends(sort: [{ key: ASC }]) { key } } }`,
  );
  expect(d.band.friends).toEqual([{ key: "b" }, { key: "c" }]);
});
