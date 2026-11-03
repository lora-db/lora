// `<plural>Connection` over an interface or a union (G-79): every
// implementation's nodes merged in one order, paged by keyset cursors,
// with a totalCount across implementations.

import { createTestLoraGraphQL } from "../src/testing.js";
import { festivalHarness, type Harness } from "./harness.js";

// The same types as abstract.test.ts (importing them would run its tests).
const abstractTypeDefs = /* GraphQL */ `
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
`;

const seed = [
  `CREATE (:Concert {key: 'c1', title: 'Rock night', starts: 20, band: 'Blur'}),
          (:Concert {key: 'c2', title: 'Jazz brunch', starts: 11, band: 'Trio'}),
          (:Concert {key: 'c3', title: 'Open mic', band: 'Anyone'}),
          (:Exhibition {key: 'e1', title: 'Modern art', starts: 10, artist: 'Ann'}),
          (:Exhibition {key: 'e2', title: 'Rock photos', starts: 15, artist: 'Bo'}),
          (:Exhibition {key: 'e3', title: 'Sculpture', starts: 11, artist: 'Cy'}),
          (:Exhibition {key: 'e4', title: 'Prints', artist: 'Di'})`,
];

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness({ typeDefs: abstractTypeDefs, seed });
});

interface Page {
  edges: Array<{ cursor: string; node: { __typename: string; key: string } }>;
  pageInfo: {
    hasNextPage: boolean;
    hasPreviousPage: boolean;
    startCursor: string | null;
    endCursor: string | null;
  };
  totalCount: number;
}

const PAGE = `
  edges { cursor node { __typename key } }
  pageInfo { hasNextPage hasPreviousPage startCursor endCursor }
  totalCount
`;

/** Walks every page of `eventsConnection` forward, `size` at a time. */
async function walkForward(sort: string, size: number): Promise<string[]> {
  const keys: string[] = [];
  let after: string | null = null;
  for (let i = 0; i < 20; i++) {
    const d: { eventsConnection: Page } = await h.data(
      `query($after: String) { eventsConnection(sort: ${sort}, first: ${size}, after: $after) { ${PAGE} } }`,
      { after },
    );
    keys.push(...d.eventsConnection.edges.map((e) => e.node.key));
    expect(d.eventsConnection.totalCount).toBe(7);
    if (!d.eventsConnection.pageInfo.hasNextPage) return keys;
    after = d.eventsConnection.pageInfo.endCursor;
  }
  throw new Error("did not finish");
}

/** Walks every page backward from the end, `size` at a time. */
async function walkBackward(sort: string, size: number): Promise<string[]> {
  const keys: string[] = [];
  let before: string | null = null;
  for (let i = 0; i < 20; i++) {
    const d: { eventsConnection: Page } = await h.data(
      `query($before: String) { eventsConnection(sort: ${sort}, last: ${size}, before: $before) { ${PAGE} } }`,
      { before },
    );
    keys.unshift(...d.eventsConnection.edges.map((e) => e.node.key));
    if (!d.eventsConnection.pageInfo.hasPreviousPage) return keys;
    before = d.eventsConnection.pageInfo.startCursor;
  }
  throw new Error("did not finish");
}

test("the schema has a connection root per interface and union", () => {
  const query = h.lora.getSchema().getQueryType()!.getFields();
  expect(query["eventsConnection"]!.type.toString()).toBe("EventConnection!");
  expect(query["headlinesConnection"]!.type.toString()).toBe(
    "HeadlineConnection!",
  );
  expect(query["eventsConnection"]!.args.map((a) => a.name)).toEqual([
    "where",
    "sort",
    "first",
    "after",
    "last",
    "before",
  ]);
});

test("a page and its total across implementations", async () => {
  const d = await h.data<{ eventsConnection: Page }>(
    `{ eventsConnection(sort: [{ starts: ASC }], first: 3) { ${PAGE} } }`,
  );
  expect(d.eventsConnection.edges.map((e) => e.node)).toEqual([
    { __typename: "Exhibition", key: "e1" },
    { __typename: "Concert", key: "c2" },
    { __typename: "Exhibition", key: "e3" },
  ]);
  expect(d.eventsConnection.totalCount).toBe(7);
  expect(d.eventsConnection.pageInfo.hasNextPage).toBe(true);
  expect(d.eventsConnection.pageInfo.hasPreviousPage).toBe(false);
});

test("every sort pages through every node once, forward and backward", async () => {
  for (const sort of [
    "[]",
    "[{ starts: ASC }]",
    "[{ starts: DESC }]",
    "[{ title: ASC }]",
    "[{ title: DESC }, { starts: ASC }]",
    "[{ starts: DESC }, { title: DESC }]",
  ]) {
    const list = await h.data<{ events: Array<{ key: string }> }>(
      `{ events(sort: ${sort}, limit: 50) { key } }`,
    );
    const expected = list.events.map((e) => e.key);
    expect(expected).toHaveLength(7);
    for (const size of [1, 2, 3, 7]) {
      expect(await walkForward(sort, size), `${sort} first ${size}`).toEqual(
        expected,
      );
      expect(await walkBackward(sort, size), `${sort} last ${size}`).toEqual(
        expected,
      );
    }
  }
});

test("where filters the page and the total", async () => {
  const d = await h.data<{ eventsConnection: Page }>(
    `{ eventsConnection(where: { title: { contains: "Rock" } }, first: 1) { ${PAGE} } }`,
  );
  expect(d.eventsConnection.edges.map((e) => e.node.key)).toEqual(["c1"]);
  expect(d.eventsConnection.totalCount).toBe(2);
  const typed = await h.data<{ eventsConnection: Page }>(
    `{ eventsConnection(where: { typename: [Exhibition] }) { totalCount } }`,
  );
  expect(typed.eventsConnection.totalCount).toBe(4);
});

test("a union's connection takes a where per member", async () => {
  const UNION_PAGE = `edges { node { __typename ... on Event { key } } } totalCount`;
  const d = await h.data<{ headlinesConnection: Page }>(
    `{ headlinesConnection(where: { Exhibition: { title: { eq: "Prints" } } }, first: 10) { ${UNION_PAGE} } }`,
  );
  // Once a member is named, members not named are left out.
  expect(d.headlinesConnection.edges.map((e) => e.node)).toEqual([
    { __typename: "Exhibition", key: "e4" },
  ]);
  expect(d.headlinesConnection.totalCount).toBe(1);
  const all = await h.data<{ headlinesConnection: Page }>(
    `{ headlinesConnection(first: 4) { ${UNION_PAGE} } }`,
  );
  expect(all.headlinesConnection.edges.map((e) => e.node.key)).toEqual([
    "c1",
    "c2",
    "c3",
    "e1",
  ]);
  expect(all.headlinesConnection.totalCount).toBe(7);
});

test("only the total runs one statement; only the page runs one", async () => {
  h.statements.length = 0;
  const total = await h.data<{ eventsConnection: { totalCount: number } }>(
    `{ eventsConnection { totalCount } }`,
  );
  expect(total.eventsConnection.totalCount).toBe(7);
  expect(h.statements).toHaveLength(1);
  h.statements.length = 0;
  const page = await h.data<{ eventsConnection: Page }>(
    `{ eventsConnection(first: 2) { edges { node { key } } } }`,
  );
  expect(page.eventsConnection.edges).toHaveLength(2);
  expect(h.statements).toHaveLength(1);
});

test("fields of each implementation, aliases included", async () => {
  const d = await h.data<{
    page: { list: Array<{ n: Record<string, unknown> }>; n: number };
  }>(`{
    page: eventsConnection(sort: [{ starts: DESC }], first: 2) {
      list: edges { n: node { key ... on Concert { band } ... on Exhibition { artist } } }
      n: totalCount
    }
  }`);
  expect(d.page).toEqual({
    list: [
      { n: { key: "c3", band: "Anyone" } },
      { n: { key: "e4", artist: "Di" } },
    ],
    n: 7,
  });
});

test("a cursor from another sort is refused", async () => {
  const d = await h.data<{ eventsConnection: Page }>(
    `{ eventsConnection(sort: [{ starts: ASC }], first: 1) { ${PAGE} } }`,
  );
  const r = await h.run(
    `query($after: String) { eventsConnection(sort: [{ title: ASC }], first: 1, after: $after) { totalCount } }`,
    { after: d.eventsConnection.pageInfo.endCursor },
  );
  expect(r.errors?.[0]?.extensions?.["code"]).toBe("INVALID_CURSOR");
});

test("first and last together are refused", async () => {
  const r = await h.run(
    `{ eventsConnection(first: 1, last: 1) { totalCount } }`,
  );
  expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
});

test("READ rules filter each member's page and total", async () => {
  const t = await createTestLoraGraphQL({
    typeDefs: `
      type Claims @jwt { sub: String! }
      interface Note { key: String! owner: String! }
      type Memo implements Note @node
        @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
        key: String! @key
        owner: String!
      }
      type Draft implements Note @node
        @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
        key: String! @key
        owner: String!
      }`,
    seed: [
      `CREATE (:Memo {key: 'm1', owner: 'ann'}), (:Memo {key: 'm2', owner: 'bo'}),
              (:Draft {key: 'd1', owner: 'ann'}), (:Draft {key: 'd2', owner: 'bo'}),
              (:Draft {key: 'd3', owner: 'ann'})`,
    ],
  });
  const ann = { jwt: { sub: "ann" } };
  expect(
    await t.data(
      `{ notesConnection(first: 2) { edges { node { key } } totalCount pageInfo { hasNextPage } } }`,
      {},
      ann,
    ),
  ).toEqual({
    notesConnection: {
      edges: [{ node: { key: "d1" } }, { node: { key: "d3" } }],
      totalCount: 3,
      pageInfo: { hasNextPage: true },
    },
  });
});
