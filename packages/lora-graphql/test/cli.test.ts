import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { main } from "../src/cli.js";
import { festivalTypeDefs } from "./fixtures.js";

let dir: string;
beforeAll(async () => {
  dir = await mkdtemp(join(tmpdir(), "lora-graphql-cli-"));
  await writeFile(join(dir, "schema.graphql"), festivalTypeDefs);
  await mkdir(join(dir, "ops"));
  await writeFile(
    join(dir, "ops", "search.graphql"),
    `query Search($q: String!) { festivals(where: { name: { contains: $q } }) { ...F } }
     query Page($after: String) { festivalsConnection(first: 5, after: $after, sort: [{ name: ASC }]) { edges { node { key } } } }
     fragment F on Festival { key name }`,
  );
});

async function cli(...args: string[]) {
  const out: string[] = [];
  const err: string[] = [];
  const code = await main(args, {
    out: (l) => out.push(l),
    err: (l) => err.push(l),
  });
  return { code, out: out.join("\n"), err: err.join("\n") };
}

test("print", async () => {
  const r = await cli("print", join(dir, "schema.graphql"));
  expect(r.code).toBe(0);
  expect(r.out).toMatch(/type Festival implements Node/);
  expect(r.out).not.toMatch(/@filterable/);
});

test("requirements --ddl", async () => {
  const r = await cli("requirements", join(dir, "schema.graphql"), "--ddl");
  expect(r.out.split("\n")[0]).toBe(
    "CREATE CONSTRAINT `festival_key_key` IF NOT EXISTS FOR (n:`Festival`) REQUIRE n.`key` IS NODE KEY;",
  );
});

test("check plans every operation against an in-memory database", async () => {
  const r = await cli(
    "check",
    join(dir, "schema.graphql"),
    "--operations",
    join(dir, "ops"),
  );
  expect(r.err).toBe("");
  expect(r.out.split("\n").at(-1)).toBe(
    "ok       2 root field(s) in 2 operation(s) seek as expected",
  );
  // Lint notes are reported, not failures.
  expect(r.out).toContain("lint     Festival.followers: no @cardinality");
  expect(r.code).toBe(0);
});

test("check --baseline records plans, then fails when one changes", async () => {
  const baseline = join(dir, "plans.json");
  const args = [
    "check",
    join(dir, "schema.graphql"),
    "--operations",
    join(dir, "ops"),
    "--baseline",
    baseline,
  ];
  expect((await cli(...args)).code).toBe(0);
  const recorded = JSON.parse(await readFile(baseline, "utf8"));
  const key = Object.keys(recorded)[0]!;
  recorded[key][0] = ["Projection"];
  await writeFile(baseline, JSON.stringify(recorded));
  const changed = await cli(...args);
  expect(changed.code).toBe(1);
  expect(changed.out).toContain("the plan differs from the baseline");
  expect((await cli(...args, "--update-baseline")).code).toBe(0);
  expect((await cli(...args)).code).toBe(0);
});

test("check --row-budget and --variables", async () => {
  await writeFile(
    join(dir, "vars.json"),
    JSON.stringify({ Search: { q: "land" } }),
  );
  const r = await cli(
    "check",
    join(dir, "schema.graphql"),
    "--operations",
    join(dir, "ops"),
    "--variables",
    join(dir, "vars.json"),
    "--row-budget",
    "0",
    "--json",
  );
  const report = JSON.parse(r.out);
  const search = report.plans.find((p: { operation: string }) =>
    p.operation.endsWith("#Search"),
  );
  expect(search.reports[0].statement.params).toMatchObject({ p0: "land" });
  expect(r.code).toBe(0);
});

test("compile writes a manifest and types", async () => {
  const out = join(dir, "compiled");
  const r = await cli(
    "compile",
    join(dir, "schema.graphql"),
    "--operations",
    join(dir, "ops"),
    "--out",
    out,
  );
  expect(r.code).toBe(0);
  const manifest = JSON.parse(
    await readFile(join(out, "manifest.json"), "utf8"),
  );
  expect(Object.keys(manifest.operations).map((k) => k.split("#")[1])).toEqual([
    "Search",
    "Page",
  ]);
  const types = await readFile(join(out, "operations.d.ts"), "utf8");
  expect(types).toContain(
    "export interface SearchVariables {\n  q: string;\n}",
  );
});

test("diff exits non-zero on breaking changes", async () => {
  await writeFile(
    join(dir, "next.graphql"),
    festivalTypeDefs.replace("title: String @alias", "headline: String @alias"),
  );
  const r = await cli(
    "diff",
    join(dir, "schema.graphql"),
    join(dir, "next.graphql"),
  );
  expect(r.code).toBe(1);
  expect(r.out).toMatch(/breaking {3}Festival\.title was removed\./);
  expect(r.out).toMatch(
    /note {7}Festival\.title is now Festival\.headline over the same property `displayTitle`/,
  );
  const allowed = await cli(
    "diff",
    join(dir, "schema.graphql"),
    join(dir, "next.graphql"),
    "--allow-breaking",
  );
  expect(allowed.code).toBe(0);
});

test("model errors and usage", async () => {
  await writeFile(join(dir, "bad.graphql"), "type A @node { name: String }");
  const bad = await cli("print", join(dir, "bad.graphql"));
  expect(bad.code).toBe(1);
  expect(bad.err).toMatch(/A: a @node type needs exactly one @key field/);
  expect((await cli("nope")).code).toBe(2);
});

test("check --database reports unused indexes; analyze prints statistics", async () => {
  const { createDatabase } = await import("@loradb/lora-node");
  const databaseDir = join(dir, "db");
  const db = await createDatabase("app", { databaseDir });
  const { LoraGraphQL, loraDriver } = await import("../src/index.js");
  await new LoraGraphQL({
    typeDefs: festivalTypeDefs,
    driver: loraDriver(db),
  }).assertSchema({
    create: true,
  });
  await db.execute(
    "CREATE INDEX extra_idx FOR (f:Festival) ON (f.internalNotes)",
  );
  await db.execute(
    "UNWIND range(1, 3) AS i CREATE (:Festival {key: 'f' + toString(i), name: 'F', displayTitle: 'x'})",
  );
  db.dispose();

  const checked = await cli(
    "check",
    join(dir, "schema.graphql"),
    "--database",
    databaseDir,
  );
  expect(checked.out).toContain(
    "unused   RANGE index extra_idx on :Festival(internalNotes)",
  );
  expect(checked.code).toBe(0);

  const analyzed = await cli(
    "analyze",
    join(dir, "schema.graphql"),
    "--database",
    databaseDir,
  );
  expect(analyzed.code).toBe(0);
  expect(JSON.parse(analyzed.out)).toMatchObject({ nodes: { Festival: 3 } });
});

test("migrate neo4j prints the rewritten SDL with TODOs first", async () => {
  await writeFile(
    join(dir, "neo4j.graphql"),
    `type Movie @node { id: ID! @id title: String! @coalesce(value: "") }`,
  );
  const r = await cli("migrate", "neo4j", join(dir, "neo4j.graphql"));
  expect(r.code).toBe(0);
  const lines = r.out.split("\n");
  expect(lines[0]).toMatch(/^# TODO\(migrate\): /);
  expect(r.out).toContain("id: ID! @key(generate: true)");
});
