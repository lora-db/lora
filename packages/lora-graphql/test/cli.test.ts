import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
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
  expect(r.out).toBe(
    "ok       2 root field(s) in 2 operation(s) seek as expected",
  );
  expect(r.code).toBe(0);
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
