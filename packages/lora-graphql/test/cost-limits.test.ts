import { parse } from "graphql";
import { festivalHarness, type Harness } from "./harness.js";
import { LoraGraphQL, loraDriver, type Statistics } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type User @node @query(aggregate: true) {
    key: String! @key
    name: String
      @filterable(byValue: [EQ, IN, CONTAINS, ENDS_WITH, CASE_INSENSITIVE])
    age: Int @filterable(byValue: [GT, LT])
    secret: String @authentication
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

describe("analyze() fan-out", () => {
  test("nested lists use the maximum degree, measured over every node", async () => {
    // 40 users; the last one follows 30, the others one each.
    const hub = await festivalHarness({
      typeDefs,
      seed: [
        `UNWIND range(0, 39) AS i CREATE (:User {key: 'u' + toString(i)})`,
        `MATCH (a:User), (b:User) WHERE a.key <> 'u39' AND b.key = 'u0' AND a <> b CREATE (a)-[:FOLLOWS]->(b)`,
        `MATCH (a:User {key: 'u39'}), (b:User) WHERE b.key <> 'u39' WITH a, b LIMIT 30 CREATE (a)-[:FOLLOWS]->(b)`,
      ],
    });
    // The sample (the first 10 users) never sees the hub.
    const stats = await hub.lora.analyze({ sample: 10 });
    expect(stats.degrees["User.follows"]).toMatchObject({ p99: 1, max: 30 });
    const q = `{ user(key: "u39") { follows(limit: 50) { key follows(limit: 50) { key } } } }`;
    // 1 + 30 + 30 x 30: each level at the hub's degree, under the limit.
    expect(cost(q, hub.lora)).toBe(1 + 30 + 30 * 30);
    const r = await hub.data<{
      user: { follows: Array<{ follows: unknown[] }> };
    }>(q);
    const returned =
      1 +
      r.user.follows.length +
      r.user.follows.reduce((n, f) => n + f.follows.length, 0);
    expect(returned).toBeLessThanOrEqual(1 + 30 + 30 * 30);
  });
});

describe("filter operand caps", () => {
  const q = `query ($keys: [String!], $name: String) {
    users(where: { key: { in: $keys }, name: { contains: $name } }) { key }
  }`;
  const run = (lora: LoraGraphQL, variables: Record<string, unknown>) =>
    lora.execute({ source: q, variables });

  test("in lists hold at most maxListFilter items, strings maxStringFilter characters", async () => {
    const ok = await run(h.lora, {
      keys: Array.from({ length: 1000 }, (_, i) => `k${i}`),
    });
    expect(ok.errors).toBeUndefined();
    h.statements.length = 0;
    const long = await run(h.lora, {
      keys: Array.from({ length: 1001 }, (_, i) => `k${i}`),
    });
    expect(long.errors?.[0]?.extensions).toMatchObject({
      code: "BAD_USER_INPUT",
      maxListFilter: 1000,
    });
    const wide = await run(h.lora, { name: "x".repeat(10_001) });
    expect(wide.errors?.[0]?.extensions).toMatchObject({
      code: "BAD_USER_INPUT",
      maxStringFilter: 10_000,
    });
    const item = await run(h.lora, { keys: ["x".repeat(10_001)] });
    expect(item.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    const ci = await h.lora.execute({
      source: `query ($n: [String!]) { users(where: { name: { caseInsensitive: { in: $n } } }) { key } }`,
      variables: { n: Array.from({ length: 1001 }, (_, i) => `n${i}`) },
    });
    expect(ci.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    expect(h.statements).toEqual([]);
  });

  test("the caps are configurable", async () => {
    const lora = new LoraGraphQL({
      typeDefs,
      driver: loraDriver(h.db),
      maxListFilter: 2,
      maxStringFilter: 3,
    });
    expect(
      (await run(lora, { keys: ["a", "b", "c"] })).errors?.[0]?.message,
    ).toMatch(/at most 2 items/);
    expect((await run(lora, { name: "abcd" })).errors?.[0]?.message).toMatch(
      /at most 3 characters/,
    );
    expect(
      (await run(lora, { keys: ["a", "b"], name: "Ann" })).errors,
    ).toBeUndefined();
  });
});

describe("error amplification", () => {
  test("identical errors across list indices collapse into one, with a count", async () => {
    const r = await h.lora.execute({
      source: `{ users { key follows { key secret } } }`,
    });
    expect(r.errors).toHaveLength(1);
    const [error] = r.errors!;
    expect(error!.message).toBe("User.secret needs an authenticated request");
    expect(error!.path).toEqual(["users", 0, "follows", 0, "secret"]);
    expect(error!.locations).toHaveLength(1);
    expect(error!.extensions).toMatchObject({
      code: "UNAUTHENTICATED",
      count: 3,
      pathPattern: ["users", "*", "follows", "*", "secret"],
    });
    // The rows are still there, with the refused field null.
    expect((r.data as { users: unknown[] }).users).toHaveLength(3);
  });

  test("different fields and messages stay apart; the Envelop plugin collapses too", async () => {
    const r = await h.lora.execute({
      source: `{ users { secret follows { secret } } }`,
    });
    expect(r.errors?.map((e) => e.extensions["count"])).toEqual([3, 3]);
    const plugin = h.lora.envelopPlugin();
    const raw = await h.run(`{ users { key secret } }`);
    expect(raw.errors).toHaveLength(3);
    let collapsed: { errors?: readonly unknown[] } | undefined;
    plugin
      .onExecute({
        args: { document: parse("{ users { key } }") } as never,
        executeFn: () => undefined,
        setExecuteFn: () => undefined,
      })
      .onExecuteDone({
        result: raw,
        setResult: (result) => (collapsed = result),
      });
    expect(collapsed?.errors).toHaveLength(1);
  });
});
