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

test("check --context compiles operations as a signed-in caller", async () => {
  await writeFile(
    join(dir, "guarded.graphql"),
    `type Claims @jwt { sub: String! }
     type Note @node @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
       key: String! @key
       owner: String!
     }`,
  );
  await mkdir(join(dir, "guarded-ops"), { recursive: true });
  await writeFile(
    join(dir, "guarded-ops", "notes.graphql"),
    `query Mine { notes { key } }`,
  );
  await writeFile(
    join(dir, "ctx.json"),
    JSON.stringify({ "*": { jwt: { sub: "u-7" } } }),
  );
  const params = async (...extra: string[]) => {
    const r = await cli(
      "check",
      join(dir, "guarded.graphql"),
      "--operations",
      join(dir, "guarded-ops"),
      "--json",
      ...extra,
    );
    const report = JSON.parse(r.out);
    return Object.values(report.plans[0].reports[0].statement.params);
  };
  expect(await params("--context", join(dir, "ctx.json"))).toContain("u-7");
  expect(await params()).not.toContain("u-7");
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
  expect(r.out).toMatch(/breaking {3}(Field )?Festival\.title was removed\./);
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

test("diff --base reads the SDL at a git ref, joining files", async () => {
  const { execFileSync } = await import("node:child_process");
  const repo = await mkdtemp(join(tmpdir(), "lora-graphql-diff-"));
  const git = (...args: string[]) =>
    execFileSync("git", args, { cwd: repo, stdio: "pipe" });
  git("init", "-q");
  const schema = join(repo, "packages", "schema");
  await mkdir(join(schema, "types"), { recursive: true });
  const festival = `type Festival @node { key: String! @key  name: String @unique }`;
  const tag = `type Tag @node { key: String! @key }`;
  await writeFile(join(schema, "festival.graphql"), festival);
  await writeFile(join(schema, "types", "tag.graphql"), tag);
  git("add", ".");
  git("-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "base");

  // No change: exit 0, from a directory or from its files.
  const same = await cli("diff", "--base", "HEAD", schema);
  expect(same.code).toBe(0);
  expect(same.out).toContain("no changes");
  expect(
    (
      await cli(
        "diff",
        "--base",
        "HEAD",
        join(schema, "festival.graphql"),
        join(schema, "types", "tag.graphql"),
      )
    ).code,
  ).toBe(0);

  // A field removed from one file: breaking.
  await writeFile(
    join(schema, "types", "tag.graphql"),
    tag.replace("}", "label: String }"),
  );
  await writeFile(
    join(schema, "festival.graphql"),
    `type Festival @node { key: String! @key }`,
  );
  const breaking = await cli("diff", "--base", "HEAD", schema);
  expect(breaking.code).toBe(1);
  expect(breaking.out).toMatch(
    /breaking {3}(Field )?Festival\.name was removed/,
  );
  expect(breaking.out).toMatch(/destructive: no longer needed/);
  expect(
    (await cli("diff", "--base", "HEAD", schema, "--allow-breaking")).code,
  ).toBe(0);

  // A destructive statement alone also fails: dropping an index.
  await writeFile(
    join(schema, "festival.graphql"),
    `type Festival @node { key: String! @key  name: String }`,
  );
  const destructive = await cli("diff", "--base", "HEAD", schema);
  expect(destructive.out).toMatch(/destructive: no longer needed/);
  expect(destructive.code).toBe(1);

  // The nested file counts on both sides.
  await writeFile(join(schema, "types", "tag.graphql"), "");
  const nested = await cli("diff", "--base", "HEAD", schema, "--json");
  expect(nested.out).toMatch(/Tag was removed/);

  // A ref without the files: nothing to compare. A bad ref: an error.
  git(
    "-c",
    "user.email=t@t",
    "-c",
    "user.name=t",
    "commit",
    "-qm",
    "empty",
    "--allow-empty",
  );
  const fresh = join(repo, "new");
  await mkdir(fresh);
  await writeFile(join(fresh, "a.graphql"), tag);
  const none = await cli("diff", "--base", "HEAD", fresh);
  expect(none.code).toBe(0);
  expect(none.out).toContain("nothing to compare");
  const bad = await cli("diff", "--base", "no-such-ref", schema);
  expect(bad.code).toBe(1);
  expect(bad.err).toContain("no-such-ref is not a commit");
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
  // Constraint-backed indexes (the @key's) serve their constraint.
  expect(checked.out.match(/^unused/gm)).toHaveLength(1);
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
