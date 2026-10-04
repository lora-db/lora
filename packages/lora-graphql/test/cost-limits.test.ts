import { festivalHarness, type Harness } from "./harness.js";
import { LoraGraphQL, loraDriver, type Statistics } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type User @node @query(aggregate: true) {
    key: String! @key
    name: String
      @filterable(byValue: [EQ, IN, CONTAINS, ENDS_WITH, CASE_INSENSITIVE])
    age: Int @filterable(byValue: [GT, LT])
    follows: [User!]! @relationship(type: "FOLLOWS", direction: OUT) @filterable
    followers: [User!]!
      @relationship(type: "FOLLOWS", direction: IN)
      @filterable
    city: City @relationship(type: "LIVES_IN", direction: OUT) @filterable
  }
  type City @node {
    key: String! @key
    name: String @filterable
    country: Country @relationship(type: "IN", direction: OUT) @filterable
  }
  type Country @node {
    key: String! @key
    name: String @filterable
  }
`;

const seed = [
  `CREATE (a:User {key: 'a', name: 'Ann', age: 30}), (b:User {key: 'b', name: 'Bob', age: 40}),
          (c:User {key: 'c', name: 'Cat', age: 50})
   CREATE (a)-[:FOLLOWS]->(b), (b)-[:FOLLOWS]->(c), (c)-[:FOLLOWS]->(a)
   CREATE (nl:Country {key: 'nl', name: 'NL'}), (ams:City {key: 'ams', name: 'Amsterdam'})
   CREATE (ams)-[:IN]->(nl), (a)-[:LIVES_IN]->(ams)`,
];

const stats: Statistics = {
  nodes: { User: 5000, City: 10, Country: 1 },
  degrees: {
    "User.follows": { sampled: 1000, mean: 10, median: 8, p99: 40, max: 300 },
    "User.followers": { sampled: 1000, mean: 10, median: 8, p99: 40, max: 900 },
  },
};

let h: Harness;
beforeAll(async () => {
  h = await festivalHarness({ typeDefs, seed });
});

const cost = (source: string, lora = h.lora) =>
  lora.compile(source).reduce((sum, f) => sum + f.compiled.cost, 0);

describe("filter cost", () => {
  test("an indexed filter costs its page; a scan also costs the rows it examines", () => {
    const page = cost(`{ users(limit: 10) { key } }`);
    expect(page).toBe(10);
    expect(
      cost(`{ users(limit: 10, where: { name: { eq: "Ann" } }) { key } }`),
    ).toBe(10);
    // Without statistics, a scan is charged the page size it examines.
    for (const w of [
      `{ name: { contains: "n" } }`,
      `{ name: { endsWith: "n" } }`,
      `{ name: { caseInsensitive: { eq: "ann" } } }`,
      `{ NOT: { name: { eq: "Ann" } } }`,
    ]) {
      expect(cost(`{ users(limit: 10, where: ${w}) { key } }`)).toBe(20);
    }
  });

  test("relationship quantifiers multiply by the degree, per level", () => {
    // 10 candidates x 25 related (the default page) per level.
    const one = cost(
      `{ users(limit: 10, where: { follows: { some: { name: { eq: "Bob" } } } }) { key } }`,
    );
    const two = cost(
      `{ users(limit: 10, where: { follows: { some: { follows: { some: { name: { eq: "Bob" } } } } } }) { key } }`,
    );
    expect(one).toBe(10 + 10 + 10 * 25);
    expect(two).toBe(10 + 10 + 10 * 25 * (1 + 25));
    for (const q of ["none", "all", "single"]) {
      expect(
        cost(
          `{ users(limit: 10, where: { follows: { ${q}: { name: { eq: "Bob" } } } }) { key } }`,
        ),
      ).toBe(one);
    }
    expect(
      cost(
        `{ users(limit: 10, where: { follows: { count: { gt: 2 } } }) { key } }`,
      ),
    ).toBe(10 + 10 + 10 * 25);
    expect(
      cost(
        `{ users(limit: 10, where: { follows: { aggregate: { node: { age: { avg: { gt: 2 } } } } } }) { key } }`,
      ),
    ).toBeGreaterThan(10 + 10);
  });

  test("with statistics: the label count for scans, mean degrees, max from chosen parents", () => {
    h.lora.useStatistics(stats);
    try {
      expect(
        cost(
          `{ users(limit: 10, where: { name: { contains: "n" } }) { key } }`,
        ),
      ).toBe(10 + 5000);
      // Every user is a candidate: the mean degree.
      expect(
        cost(
          `{ users(limit: 10, where: { follows: { some: { name: { eq: "B" } } } }) { key } }`,
        ),
      ).toBe(10 + 5000 + 5000 * 10);
      // A user picked by key may be the hub: the maximum.
      expect(
        cost(
          `{ users(where: { key: { eq: "a" }, follows: { some: { name: { eq: "B" } } } }) { key } }`,
        ),
      ).toBe(25 + 1 * (1 + 300));
      // Anchored on a related key: the anchor's followers, at most.
      expect(
        cost(
          `{ users(limit: 10, where: { follows: { some: { key: { eq: "b" } } } }) { key } }`,
        ),
      ).toBe(10 + 900 * 10);
      // A filter on a nested list examines every related node of each parent.
      expect(
        cost(
          `{ users(limit: 2) { follows(limit: 5, where: { follows: { some: { name: { eq: "B" } } } }) { key } } }`,
        ),
      ).toBe(2 + 2 * 5 + 2 * 300 * (10 * 1));
      // Counting reads every match.
      expect(cost(`{ usersAggregate { count } }`)).toBe(5000);
      expect(cost(`{ usersConnection(first: 5) { totalCount } }`)).toBe(5000);
    } finally {
      h.lora.useStatistics({ nodes: {}, degrees: {} });
    }
  });

  test("a nested quantifier over a large label exceeds maxCost before it runs", async () => {
    h.lora.useStatistics(stats);
    try {
      h.statements.length = 0;
      const r = await h.run(
        `{ users(where: { follows: { some: { follows: { some: { name: { contains: "x" } } } } } }) { key } }`,
      );
      expect(r.errors?.[0]?.extensions?.["code"]).toBe("COST_EXCEEDED");
      expect(h.statements).toEqual([]);
    } finally {
      h.lora.useStatistics({ nodes: {}, degrees: {} });
    }
  });
});

describe("maxFilterDepth", () => {
  test("relationship levels deeper than 2 are refused", async () => {
    const two = await h.data<{ users: Array<{ key: string }> }>(
      `{ users(where: { follows: { some: { follows: { some: { name: { eq: "Cat" } } } } } }) { key } }`,
    );
    expect(two.users.map((u) => u.key)).toEqual(["a"]);
    h.statements.length = 0;
    const r = await h.run(
      `{ users(where: { follows: { some: { follows: { some: { follows: { some: { name: { eq: "Cat" } } } } } } } }) { key } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    expect(r.errors?.[0]?.message).toMatch(/deeper than 2 levels/);
    expect(h.statements).toEqual([]);
  });

  test("single relationships, counts and nested-list filters count too", async () => {
    // user -> city -> country: two single-relationship levels.
    const ok = await h.data<{ users: Array<{ key: string }> }>(
      `{ users(where: { city: { country: { name: { eq: "NL" } } } }) { key } }`,
    );
    expect(ok.users.map((u) => u.key)).toEqual(["a"]);
    const deep = [
      `{ users(where: { follows: { some: { city: { country: { name: { eq: "NL" } } } } } }) { key } }`,
      `{ users(where: { follows: { some: { follows: { some: { follows: { count: { gt: 0 } } } } } } }) { key } }`,
      `{ users(where: { followers: { all: { follows: { none: { cityExists: true } } } } }) { key } }`,
      `{ users { follows(where: { follows: { some: { follows: { some: { cityExists: true } } } } }) { key } } }`,
      `{ users(where: { OR: [{ NOT: { follows: { some: { follows: { some: { city: { name: { eq: "x" } } } } } } } }] }) { key } }`,
    ];
    for (const q of deep) {
      const r = await h.run(q);
      expect(r.errors?.[0]?.extensions?.["code"], q).toBe("BAD_USER_INPUT");
    }
  });

  test("the limit is configurable", async () => {
    const deep = await festivalHarness({ typeDefs, seed });
    const loose = new LoraGraphQL({
      typeDefs,
      driver: loraDriver(deep.db),
      maxFilterDepth: 3,
      maxCost: Infinity,
    });
    const r = await loose.execute({
      source: `{ users(where: { follows: { some: { follows: { some: { follows: { some: { name: { eq: "Cat" } } } } } } } }) { key } }`,
    });
    expect(r.errors).toBeUndefined();
  });
});

describe("operation budget", () => {
  /** A driver whose reads take `ms`, recording how many run at once. */
  function slowDriver(ms: number) {
    const inner = loraDriver(h.db);
    const seen = { inFlight: 0, max: 0, aborted: 0, timeouts: [] as number[] };
    const driver: typeof inner = {
      ...inner,
      run: async (statements, options) => {
        seen.inFlight++;
        seen.max = Math.max(seen.max, seen.inFlight);
        if (options.timeoutMs !== undefined) {
          seen.timeouts.push(options.timeoutMs);
        }
        try {
          await new Promise<void>((resolve, reject) => {
            let done = false;
            const timer = setTimeout(() => {
              done = true;
              resolve();
            }, ms);
            options.signal?.addEventListener("abort", () => {
              if (done) return;
              clearTimeout(timer);
              seen.aborted++;
              reject(new Error("aborted"));
            });
          });
          return await inner.run(statements, options);
        } finally {
          seen.inFlight--;
        }
      },
    };
    return { driver, seen };
  }
  const aliases = (n: number) =>
    `{ ${Array.from({ length: n }, (_, i) => `a${i}: user(key: "a") { key follows(limit: 1) { key } }`).join(" ")} }`;

  test("aliased root fields run at most maxConcurrentStatements at once", async () => {
    const { driver, seen } = slowDriver(10);
    const lora = new LoraGraphQL({ typeDefs, driver });
    const r = await lora.execute({ source: aliases(6) });
    expect(r.errors).toBeUndefined();
    expect(seen.max).toBe(2);
    const one = slowDriver(10);
    const serial = new LoraGraphQL({
      typeDefs,
      driver: one.driver,
      maxConcurrentStatements: 1,
    });
    expect(
      (await serial.execute({ source: aliases(3) })).errors,
    ).toBeUndefined();
    expect(one.seen.max).toBe(1);
  });

  test("operationTimeoutMs bounds the whole operation and aborts its statements", async () => {
    const { driver, seen } = slowDriver(60);
    const lora = new LoraGraphQL({
      typeDefs,
      driver,
      operationTimeoutMs: 100,
      maxConcurrentStatements: 1,
    });
    const context = {};
    const started = performance.now();
    const r = await lora.execute({ source: aliases(4), context });
    expect(performance.now() - started).toBeLessThan(200);
    expect(r.data).toEqual({
      a0: { key: "a", follows: [{ key: "b" }] },
      a1: null,
      a2: null,
      a3: null,
    });
    const codes = (r.errors ?? []).map((e) => e.extensions["code"]);
    expect(codes).toEqual(["TIMEOUT", "TIMEOUT", "TIMEOUT"]);
    expect(r.errors?.[0]?.extensions["operationTimeoutMs"]).toBe(100);
    // The statement in flight at the deadline was aborted, and each got
    // at most the time left.
    expect(seen.aborted).toBe(1);
    expect(seen.timeouts[1]).toBeLessThanOrEqual(100 - 60 + 5);
    // The context, reused for the next request, starts a fresh budget.
    await new Promise((resolve) => setTimeout(resolve, 5));
    const again = await lora.execute({ source: aliases(1), context });
    expect(again.errors).toBeUndefined();
  });

  test("operationTimeoutMs defaults to twice timeoutMs; 0 disables it", async () => {
    const { driver, seen } = slowDriver(1);
    const lora = new LoraGraphQL({ typeDefs, driver, timeoutMs: 500 });
    await lora.execute({ source: aliases(1) });
    expect(seen.timeouts[0]).toBeLessThanOrEqual(500);
    const off = slowDriver(1);
    const unbounded = new LoraGraphQL({
      typeDefs,
      driver: off.driver,
      timeoutMs: 500,
      operationTimeoutMs: 0,
    });
    await unbounded.execute({ source: aliases(1) });
    expect(off.seen.timeouts[0]).toBe(500);
  });
});
