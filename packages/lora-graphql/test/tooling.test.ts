import { diffSchemas } from "../src/index.js";
import { appTypeDefs } from "./fixtures.js";
import { festivalHarness } from "./harness.js";

describe("diffSchemas (S7)", () => {
  const before = /* GraphQL */ `
    type Artist @node {
      key: ID! @key
      name: String! @filterable(byValue: [EQ, CONTAINS])
      bio: String
      country: String @filterable
    }
  `;

  test("reports database statements and API changes from one SDL change", () => {
    const after = /* GraphQL */ `
      type Artist @node(labels: ["Performer"]) {
        key: ID! @key
        displayName: String! @alias(property: "name") @filterable
        bio: String @alias(property: "biography")
        country: String @filterable @sortable
      }
    `;
    const diff = diffSchemas(before, after);
    expect(
      diff.database.statements.map((s) => [s.destructive, s.text]),
    ).toEqual([
      [true, "MATCH (n:Artist) SET n:Performer REMOVE n:Artist"],
      [
        true,
        "MATCH (n:Performer) WHERE n.bio IS NOT NULL SET n.biography = n.bio REMOVE n.bio",
      ],
      [true, "DROP CONSTRAINT `artist_key_key` IF EXISTS"],
      [true, "DROP INDEX `artist_name_text` IF EXISTS"],
      [
        false,
        "CREATE CONSTRAINT `performer_key_key` IF NOT EXISTS FOR (n:`Performer`) REQUIRE n.`key` IS NODE KEY",
      ],
      [
        false,
        "CREATE INDEX `performer_country_range` IF NOT EXISTS FOR (n:`Performer`) ON (n.`country`)",
      ],
    ]);
    expect(diff.notes).toEqual([
      "Artist.name is now Artist.displayName over the same property `name`: an API break with no data migration",
    ]);
    expect(diff.api.breaking.map((c) => c.description)).toContain(
      "Artist.name was removed.",
    );
    expect(diff.api.breaking.map((c) => c.description)).toContain(
      "ArtistNameFilter was removed.",
    );
  });

  test("an unchanged schema has nothing to report", () => {
    expect(diffSchemas(before, before)).toEqual({
      database: { create: [], drop: [], statements: [] },
      api: { breaking: [], dangerous: [] },
      notes: [],
    });
  });
});

describe("check()", () => {
  test("passes on a healthy schema and flags what is missing", async () => {
    const h = await festivalHarness({ typeDefs: appTypeDefs, seed: [] });
    const report = await h.lora.check({
      operations: [
        {
          name: "search",
          document: `{ festivals(where: { name: { contains: "x" } }) { key } }`,
        },
        {
          name: "broken",
          document: `{ festivals(where: { nope: 1 }) { key } }`,
        },
      ],
    });
    expect(report.cypher).toEqual([]);
    expect(report.missing).toEqual([]);
    expect(
      report.plans.map((p) => [p.operation, p.reports[0]!.findings]),
    ).toEqual([["search", []]]);
    expect(report.errors.map((e) => e.operation)).toEqual(["broken"]);
    expect(report.ok).toBe(false);
  });

  test("a @cypher statement the engine rejects fails the check", async () => {
    const h = await festivalHarness({
      seed: [],
      typeDefs: `
        type A @node {
          key: ID! @key
          bad: Int @cypher(statement: "RETURN nosuchfunction(this) AS x")
        }
      `,
    });
    const report = await h.lora.check();
    expect(report.cypher).toEqual([
      {
        type: "A",
        field: "bad",
        message: expect.stringMatching(/nosuchfunction/),
      },
    ]);
    expect(report.ok).toBe(false);
  });

  test("static @cypher checks: unknown parameters fail, unused arguments warn", async () => {
    const { buildModel } = await import("../src/index.js");
    expect(() =>
      buildModel(`
        type A @node { key: ID! @key }
        type Query { total(x: Int): Int @cypher(statement: "RETURN $y AS v") }
      `),
    ).toThrow(
      /Query\.total: the statement uses \$y, which is neither an argument nor \$jwt/,
    );
    const model = buildModel(`
      type A @node { key: ID! @key }
      type Query { total(x: Int): Int @cypher(statement: "OPTIONAL MATCH (n:A) RETURN count(n) AS v") }
    `);
    expect(model.warnings.map((w) => w.message)).toEqual([
      "argument x is never used by the statement",
      "OPTIONAL MATCH is slow on LoraDB; a pattern comprehension is usually 17-130x faster",
    ]);
  });
});

describe("execution helpers", () => {
  test("persisted operations are validated up front and run by id", async () => {
    const h = await festivalHarness({ seed: [] });
    expect(() => h.lora.persist({ bad: "{ nope }" })).toThrow(
      /bad: Cannot query field "nope"/,
    );
    h.lora.persist({ count: "{ festivalsAggregate { count } }" });
    const r = await h.lora.execute({ id: "count" });
    expect(r).toEqual({
      data: { festivalsAggregate: { count: 0 } },
      extensions: { cost: expect.any(Number) },
    });
    expect((await h.lora.execute({ id: "missing" })).errors?.[0]?.message).toBe(
      "unknown persisted operation missing",
    );
    const adhoc = await h.lora.execute({ source: "{ festivals { key } }" });
    expect(adhoc).toEqual({
      data: { festivals: [] },
      extensions: { cost: expect.any(Number) },
    });
  });

  test("changes() streams committed writes", async () => {
    const h = await festivalHarness({ typeDefs: appTypeDefs, seed: [] });
    const controller = new AbortController();
    const stream = h.lora.changes({ signal: controller.signal });
    await h.data(
      `mutation { createUsers(input: [{ key: "u1" }]) { info { nodesCreated } } }`,
    );
    const first = await stream.next();
    expect(first.value).toMatchObject({
      operation: "CREATE",
      created: [{ type: "User", key: "u1" }],
    });
    controller.abort();
    expect(await stream.next()).toEqual({ value: undefined, done: true });
  });
});
