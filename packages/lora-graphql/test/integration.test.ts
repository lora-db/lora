import { festivalHarness, type Harness } from "./harness.js";

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness();
});

type Edge = { cursor: string; node: { key: string; capacity: number | null } };
type Page = {
  festivalsConnection: {
    edges: Edge[];
    pageInfo: {
      hasNextPage: boolean;
      hasPreviousPage: boolean;
      endCursor: string | null;
    };
    totalCount: number;
  };
};

// The 30 seeded festivals, as the fixture creates them.
const festivals = Array.from({ length: 30 }, (_, i) => ({
  key: `f${i}`,
  name: `${["Sunland ", "Moonfest ", "Starland "][i % 3]}${i}`,
  capacity: i % 5 === 0 ? null : 1000 * i,
}));

function expectedOrder(
  field: "capacity" | "name",
  dir: "ASC" | "DESC",
): string[] {
  const cmp = (a: unknown, b: unknown) => (a! < b! ? -1 : a! > b! ? 1 : 0);
  return [...festivals]
    .sort((a, b) => {
      const x = a[field];
      const y = b[field];
      let c: number;
      if (x === null && y === null) c = 0;
      // Cypher: nulls last ascending, first descending.
      else if (x === null) c = dir === "ASC" ? 1 : -1;
      else if (y === null) c = dir === "ASC" ? -1 : 1;
      else c = dir === "ASC" ? cmp(x, y) : cmp(y, x);
      return c !== 0 ? c : cmp(a.key, b.key);
    })
    .map((f) => f.key);
}

async function paginate(
  sort: Record<string, string>,
  first: number,
): Promise<string[]> {
  const keys: string[] = [];
  let after: string | null = null;
  for (let pages = 0; pages < 50; pages++) {
    const page: Page = await h.data<Page>(
      `query ($sort: [FestivalSort!], $first: Int, $after: String) {
        festivalsConnection(sort: $sort, first: $first, after: $after) {
          edges { cursor node { key capacity } }
          pageInfo { hasNextPage hasPreviousPage endCursor }
        }
      }`,
      { sort: [sort], first, after },
    );
    const conn = page.festivalsConnection;
    keys.push(...conn.edges.map((e) => e.node.key));
    expect(conn.pageInfo.hasPreviousPage).toBe(after !== null);
    if (!conn.pageInfo.hasNextPage) return keys;
    after = conn.pageInfo.endCursor;
  }
  throw new Error("pagination did not terminate");
}

describe("keyset pagination", () => {
  test.each([
    ["capacity", "ASC", 4],
    ["capacity", "DESC", 4],
    ["capacity", "ASC", 7],
    ["name", "ASC", 4],
    ["name", "DESC", 6],
  ] as const)(
    "by %s %s, pages of %i, visits every row once in order",
    async (field, dir, first) => {
      expect(await paginate({ [field]: dir }, first)).toEqual(
        expectedOrder(field, dir),
      );
    },
  );

  test.each([
    ["capacity", "ASC", 4],
    ["capacity", "DESC", 3],
    ["name", "ASC", 7],
  ] as const)(
    "backward by %s %s, pages of %i, visits every row once in order",
    async (field, dir, last) => {
      const keys: string[] = [];
      let before: string | null = null;
      for (let pages = 0; pages < 50; pages++) {
        const page: {
          festivalsConnection: {
            edges: Array<{ node: { key: string } }>;
            pageInfo: {
              hasPreviousPage: boolean;
              hasNextPage: boolean;
              startCursor: string | null;
            };
          };
        } = await h.data(
          `query ($sort: [FestivalSort!], $last: Int, $before: String) {
            festivalsConnection(sort: $sort, last: $last, before: $before) {
              edges { node { key } }
              pageInfo { hasPreviousPage hasNextPage startCursor }
            }
          }`,
          { sort: [{ [field]: dir }], last, before },
        );
        const conn = page.festivalsConnection;
        keys.unshift(...conn.edges.map((e) => e.node.key));
        expect(conn.pageInfo.hasNextPage).toBe(before !== null);
        if (!conn.pageInfo.hasPreviousPage) break;
        before = conn.pageInfo.startCursor;
      }
      expect(keys).toEqual(expectedOrder(field, dir));
    },
  );

  test("a forward cursor pages backward too", async () => {
    const order = expectedOrder("capacity", "ASC");
    const fwd: { festivalsConnection: { pageInfo: { endCursor: string } } } =
      await h.data(
        `{ festivalsConnection(first: 10, sort: [{ capacity: ASC }]) { pageInfo { endCursor } } }`,
      );
    const back: {
      festivalsConnection: { edges: Array<{ node: { key: string } }> };
    } = await h.data(
      `query ($before: String) { festivalsConnection(last: 3, before: $before, sort: [{ capacity: ASC }]) { edges { node { key } } } }`,
      { before: fwd.festivalsConnection.pageInfo.endCursor },
    );
    expect(back.festivalsConnection.edges.map((e) => e.node.key)).toEqual(
      order.slice(6, 9),
    );
    const r = await h.run(
      `{ festivalsConnection(first: 1, last: 1) { totalCount } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  });

  test("totalCount ignores the page", async () => {
    const d = await h.data<Page>(
      `{ festivalsConnection(first: 2, where: { name: { startsWith: "Sun" } }) { totalCount edges { node { key } } } }`,
    );
    expect(d.festivalsConnection.totalCount).toBe(10);
    expect(d.festivalsConnection.edges).toHaveLength(2);
  });

  test("a cursor from another sort is rejected", async () => {
    const first = await h.data<Page>(
      `{ festivalsConnection(first: 1, sort: [{ name: ASC }]) { pageInfo { endCursor hasNextPage hasPreviousPage } edges { cursor node { key capacity } } } }`,
    );
    const r = await h.run(
      `query ($after: String) { festivalsConnection(after: $after, sort: [{ capacity: ASC }]) { totalCount } }`,
      { after: first.festivalsConnection.pageInfo.endCursor },
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("INVALID_CURSOR");
    expect(r.errors?.[0]?.message).toMatch(/different sort/);
  });

  test("garbage cursors are rejected", async () => {
    const r = await h.run(
      `{ festivalsConnection(after: "nope") { totalCount } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("INVALID_CURSOR");
  });
});

describe("filters", () => {
  const keys = async (where: string) =>
    (
      await h.data<{ festivals: Array<{ key: string }> }>(
        `{ festivals(limit: 50, where: ${where}) { key } }`,
      )
    ).festivals
      .map((f) => f.key)
      .sort((a, b) => Number(a.slice(1)) - Number(b.slice(1)));

  test("scalar operators", async () => {
    expect(await keys(`{ capacity: { gt: 25000 } }`)).toEqual([
      "f26",
      "f27",
      "f28",
      "f29",
    ]);
    expect(await keys(`{ name: { contains: "and 2" } }`)).toEqual([
      "f2",
      "f20",
      "f21",
      "f23",
      "f24",
      "f26",
      "f27",
      "f29",
    ]);
    expect(
      await keys(`{ status: { eq: SOLD_OUT }, capacity: { lt: 9000 } }`),
    ).toEqual(["f2", "f8"]);
    expect(await keys(`{ key: { in: ["f3", "f1", "f3", "nope"] } }`)).toEqual([
      "f1",
      "f3",
    ]);
  });

  test("AND, OR, NOT", async () => {
    expect(
      await keys(
        `{ OR: [{ key: { eq: "f1" } }, { key: { eq: "f2" } }], NOT: { key: { eq: "f2" } } }`,
      ),
    ).toEqual(["f1"]);
    expect(
      await keys(
        `{ AND: [{ capacity: { gte: 28000 } }, { capacity: { lt: 29000 } }] }`,
      ),
    ).toEqual(["f28"]);
  });

  test("absent and null filters are left out, not compiled as IS NULL", async () => {
    const all = await keys(`{}`);
    expect(all).toHaveLength(30);
    const r = await h.data<{ festivals: unknown[] }>(
      `query ($c: Int) { festivals(limit: 50, where: { capacity: { gt: $c } }) { key } }`,
      { c: null },
    );
    expect(r.festivals).toHaveLength(30);
    const last = h.statements.at(-1)!.statement.text;
    expect(last).not.toMatch(/capacity/);
  });

  test("single relationship", async () => {
    expect(
      (await keys(`{ genre: { name: { eq: "House" } } }`)).slice(0, 3),
    ).toEqual(["f1", "f4", "f7"]);
  });

  test("some / none / all / single / count", async () => {
    // u0 follows f0..f4 and f10, f20; everyone else follows f(i), f(i+10), f(i+20).
    expect(
      await keys(`{ followers: { some: { key: { eq: "u0" } } } }`),
    ).toEqual(["f0", "f1", "f2", "f3", "f4", "f10", "f20"]);
    expect(await keys(`{ followers: { count: { gt: 1 } } }`)).toEqual([
      "f1",
      "f2",
      "f3",
      "f4",
    ]);
    expect(
      await keys(
        `{ followers: { single: { key: { eq: "u1" } } }, capacity: { lt: 12000 } }`,
      ),
    ).toEqual(["f1", "f11"]);
    expect(
      (
        await keys(
          `{ followers: { none: { key: { in: ["u0", "u1", "u2"] } } } }`,
        )
      ).length,
    ).toBe(30 - 11); // u0: f0-f4, f10, f20; u1: f11, f21; u2: f12, f22
    expect(await keys(`{ followers: { all: { key: { eq: "u0" } } } }`)).toEqual(
      ["f0", "f10", "f20"],
    );
  });

  test("an empty `all` holds vacuously", async () => {
    expect(await keys(`{ followers: { all: {} } }`)).toHaveLength(30);
  });
});

describe("projection", () => {
  test("aliases, fragments, @alias, enums and relay ids", async () => {
    const d = await h.data<{ festival: Record<string, unknown> }>(`
      query {
        festival(key: "f4") {
          ...Basics
          displayName: name
          t: title
          ... on Festival { status }
          genre { name }
          g2: genre { key }
          followers(sort: [{ name: DESC }]) { name }
        }
      }
      fragment Basics on Festival { id key }
    `);
    expect(d.festival).toEqual({
      id: expect.any(String),
      key: "f4",
      displayName: "Moonfest 4",
      t: "The 4",
      status: "ON_SALE",
      genre: { name: "House" },
      g2: { key: "house" },
      followers: [{ name: "User 4" }, { name: "User 0" }],
    });
  });

  test("the same field twice with different arguments", async () => {
    const d = await h.data<{ genre: { a: unknown[]; b: unknown[] } }>(`{
      genre(key: "techno") {
        a: festivals(limit: 2, sort: [{ name: ASC }]) { key }
        b: festivals(limit: 1, sort: [{ name: DESC }]) { key }
      }
    }`);
    expect(d.genre.a).toEqual([{ key: "f0" }, { key: "f12" }]);
    expect(d.genre.b).toEqual([{ key: "f9" }]);
  });

  test("relationship connections carry properties and filter on them", async () => {
    const d = await h.data<{ festival: { followersConnection: unknown } }>(`{
      festival(key: "f1") {
        followersConnection(first: 1, where: { edge: { since: { gte: 2001 } } }, sort: [{ name: DESC }]) {
          totalCount
          edges { properties { since } node { key } }
          pageInfo { hasNextPage }
        }
      }
    }`);
    expect(d.festival.followersConnection).toEqual({
      totalCount: 2,
      edges: [{ properties: { since: 2001 }, node: { key: "u1" } }],
      pageInfo: { hasNextPage: true },
    });
  });

  test("node(id) round-trips a global id", async () => {
    const { festival } = await h.data<{ festival: { id: string } }>(
      `{ festival(key: "f7") { id } }`,
    );
    const d = await h.data<{ node: unknown }>(
      `query ($id: ID!) { node(id: $id) { __typename id ... on Festival { key name } } }`,
      { id: festival.id },
    );
    expect(d.node).toEqual({
      __typename: "Festival",
      id: festival.id,
      key: "f7",
      name: "Moonfest 7",
    });
    expect(
      (await h.data<{ node: unknown }>(`{ node(id: "bogus") { id } }`)).node,
    ).toBeNull();
  });

  test("@private fields are not exposed", async () => {
    const r = await h.run(`{ festivals { internalNotes } }`);
    expect(r.errors?.[0]?.message).toMatch(
      /Cannot query field "internalNotes"/,
    );
  });
});

describe("scalars", () => {
  test("DateTime filters, sorts and round-trips as ISO-8601", async () => {
    const t = await festivalHarness();
    await t.db.execute(
      `MATCH (f:Festival) WHERE f.key IN ['f1', 'f2', 'f3']
       SET f.startsAt = datetime('2026-07-0' + substring(f.key, 1) + 'T20:00:00Z')`,
    );
    const d = await t.data<{ festivals: unknown[] }>(`{
      festivals(where: { startsAt: { gte: "2026-07-02T00:00:00Z" } }, sort: [{ startsAt: DESC }]) {
        key startsAt
      }
    }`);
    expect(d.festivals).toEqual([
      { key: "f3", startsAt: "2026-07-03T20:00:00Z" },
      { key: "f2", startsAt: "2026-07-02T20:00:00Z" },
    ]);
    const bad = await t.run(
      `{ festivals(where: { startsAt: { gte: 5 } }) { key } }`,
    );
    expect(bad.errors?.[0]?.message).toMatch(/ISO-8601/);
  });
});

describe("aggregates and limits", () => {
  test("aggregate", async () => {
    const d = await h.data<{ festivalsAggregate: unknown }>(`{
      festivalsAggregate(where: { capacity: { gte: 27000 } }) { count capacity { min max sum avg } }
    }`);
    expect(d.festivalsAggregate).toEqual({
      count: 3,
      capacity: { min: 27000, max: 29000, sum: 84000, avg: 28000 },
    });
  });

  test("limits apply by default and are capped", async () => {
    const d = await h.data<{ festivals: unknown[] }>(`{ festivals { key } }`);
    expect(d.festivals).toHaveLength(10); // @limit(default: 10)
    const r = await h.run(`{ festivals(limit: 51) { key } }`);
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("LIMIT_EXCEEDED");
  });

  test("bad sort items are rejected", async () => {
    const r = await h.run(
      `{ festivals(sort: [{ name: ASC, capacity: DESC }]) { key } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  });
});

describe("assertSchema", () => {
  test("is idempotent and reports nothing missing once created", async () => {
    const again = await h.lora.assertSchema();
    expect(again.missing).toEqual([]);
    const created = await h.lora.assertSchema({ create: true });
    expect(created.created).toEqual([]);
  });

  test("reports what a fresh database lacks", async () => {
    const fresh = await festivalHarness({ assert: false });
    const report = await fresh.lora.assertSchema();
    expect(report.missing.map((r) => r.name)).toContain("festival_name_text");
  });
});

describe("connection aggregates", () => {
  test("root: over every match, not just the page", async () => {
    const d = await h.data<{ festivalsConnection: unknown }>(`{
      festivalsConnection(first: 1, where: { capacity: { gte: 27000 } }) {
        totalCount
        aggregate { count node { capacity { min max avg sum } name { min } } }
      }
    }`);
    expect(d.festivalsConnection).toEqual({
      totalCount: 3,
      aggregate: {
        count: 3,
        node: {
          capacity: { min: 27000, max: 29000, avg: 28000, sum: 84000 },
          name: { min: "Moonfest 28" },
        },
      },
    });
  });

  test("nested: per parent, through the relationship", async () => {
    const d = await h.data<{ genre: unknown }>(`{
      genre(key: "techno") {
        festivalsConnection(where: { capacity: { lt: 10000 } }) {
          aggregate { count node { capacity { max } } }
        }
      }
    }`);
    expect(d.genre).toEqual({
      festivalsConnection: {
        aggregate: { count: 3, node: { capacity: { max: 9000 } } },
      },
    });
  });

  test("totalCount alone reads no page", async () => {
    const [c] = h.lora.compile(`{ festivalsConnection { totalCount } }`);
    expect(c!.compiled.statements).toHaveLength(1);
    expect(c!.compiled.statements[0]!.text).toMatch(
      /count\(this\) AS totalCount/,
    );
  });
});
