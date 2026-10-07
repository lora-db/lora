// The website's GraphQL docs, held to the library: every complete schema
// they print builds, the many-to-many guide runs top to bottom with the
// results it prints, and the pages written as scenarios (a schema, a
// seed, then requests made as named callers) answer each caller with the
// response printed under the request.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { parse, validate } from "graphql";
import { describe, expect, test } from "vitest";
import { buildModel } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const docs = new URL("../../../apps/loradb.com/docs/graphql/", import.meta.url);
// The project the tutorial builds. Its own test (npm test there) starts
// its servers; here the pages are held to quoting its files verbatim.
const tutorial = new URL("../examples/tutorial/", import.meta.url);
// The package can be checked out without the site (a published tarball).
const present = existsSync(docs);

interface Block {
  language: string;
  meta: string;
  body: string;
}

/** Fenced code blocks of a markdown source, in order. */
function blocks(markdown: string): Block[] {
  return [...markdown.matchAll(/^```(\w+)([^\n]*)\n([\s\S]*?)^```$/gm)].map(
    ([, language, meta, body]) => ({
      language: language!,
      meta: meta!.trim(),
      body: body!,
    }),
  );
}

const read = (file: string) => readFileSync(new URL(file, docs), "utf8");

const title = (block: Block) => /title="([^"]*)"/.exec(block.meta)?.[1];

/**
 * Whether `actual` holds everything `printed` shows: objects may have
 * more keys than the page prints (an error's path and locations), lists
 * must match item for item.
 */
function shows(actual: unknown, printed: unknown): boolean {
  if (Array.isArray(printed)) {
    return (
      Array.isArray(actual) &&
      actual.length === printed.length &&
      printed.every((item, i) => shows(actual[i], item))
    );
  }
  if (printed !== null && typeof printed === "object") {
    if (actual === null || typeof actual !== "object") return false;
    return Object.entries(printed).every(([key, value]) =>
      shows((actual as Record<string, unknown>)[key], value),
    );
  }
  return actual === printed;
}

/** Timestamps differ per run: compare everything else. */
const withoutTimestamps = (value: unknown): unknown =>
  JSON.parse(
    JSON.stringify(value).replace(
      /"\d{4}-\d{2}-\d{2}T[\d:.]+Z"/g,
      '"<timestamp>"',
    ),
  );

describe.skipIf(!present)("website docs", () => {
  test("every schema.graphql block builds", () => {
    let built = 0;
    for (const file of readdirSync(docs).filter((f) => f.endsWith(".md"))) {
      for (const block of blocks(read(file))) {
        if (block.language !== "graphql") continue;
        if (!block.meta.includes('title="schema.graphql"')) continue;
        expect(() => buildModel(block.body), file).not.toThrow();
        built++;
      }
    }
    expect(built).toBeGreaterThan(0);
  });

  // A block marked file="x" quotes examples/tutorial/x: the whole file
  // or one contiguous part of it. The example's test runs that code, so a
  // quote that still matches is code that still works.
  test("code quoted from the tutorial project matches its files", () => {
    let quoted = 0;
    for (const file of readdirSync(docs).filter((f) => f.endsWith(".md"))) {
      for (const block of blocks(read(file))) {
        const source = /\bfile="([^"]+)"/.exec(block.meta)?.[1];
        if (source === undefined) continue;
        const actual = readFileSync(new URL(source, tutorial), "utf8");
        expect(
          actual.includes(block.body.trimEnd()),
          `${file}: the block quoting ${source} no longer matches the file`,
        ).toBe(true);
        quoted++;
      }
    }
    expect(quoted).toBeGreaterThanOrEqual(10);
  });

  test("the tutorial ends on the project's schema and seed", () => {
    const all = blocks(read("tutorial.md"));
    const last = <T>(items: T[]) => items[items.length - 1];
    const schema = last(all.filter((b) => title(b) === "schema.graphql"));
    const seed = last(all.filter((b) => title(b) === "seed.cypher"));
    const project = (name: string) =>
      readFileSync(new URL(name, tutorial), "utf8");
    expect(schema?.body).toBe(project("schema.graphql"));
    expect(seed?.body).toBe(project("seed.cypher"));
  });

  // The directive reference gives every directive an example: each SDL
  // block must build without a model warning, and each operation must be
  // valid against the schema of the SDL block before it.
  test("directives.md: every example builds, every operation is valid", async () => {
    const examples = blocks(read("directives.md")).filter(
      (b) => b.language === "graphql",
    );
    let schema:
      | ReturnType<
          Awaited<ReturnType<typeof createTestLoraGraphQL>>["lora"]["getSchema"]
        >
      | undefined;
    let schemas = 0;
    let operations = 0;
    for (const { body } of examples) {
      if (/^\s*(\{|query\b|mutation\b|subscription\b)/.test(body)) {
        expect(schema, `no schema before:\n${body}`).toBeDefined();
        const errors = validate(schema!, parse(body)).map((e) => e.message);
        expect(errors, body).toEqual([]);
        operations++;
        continue;
      }
      // Stand-ins for what the options would supply.
      const callbacks = Object.fromEntries(
        [...body.matchAll(/callback: "(\w+)"/g)].map((m) => [m[1]!, () => "x"]),
      );
      const resolvers: Record<string, Record<string, () => string>> = {};
      let type = "";
      for (const line of body.split("\n")) {
        type = /^type (\w+)/.exec(line)?.[1] ?? type;
        const field = /^\s+(\w+)[^\n]*@customResolver/.exec(line)?.[1];
        if (field) (resolvers[type] ??= {})[field] = () => "x";
      }
      const t = await createTestLoraGraphQL({
        typeDefs: body,
        callbacks,
        resolvers,
      });
      expect(t.lora.model.warnings, body).toEqual([]);
      schema = t.lora.getSchema();
      t.close();
      schemas++;
    }
    expect(schemas).toBeGreaterThanOrEqual(40);
    expect(operations).toBeGreaterThanOrEqual(15);
  });

  test("the many-to-many guide runs as written", async () => {
    const source = read("many-to-many.md");
    // The walkthrough: from the first schema to the section that
    // switches to other schemas.
    const guide = source.slice(
      source.indexOf("## The same model as a graph"),
      source.indexOf("## A table related to itself"),
    );
    const all = blocks(guide);
    const schema = all.find((b) => b.meta.includes('title="schema.graphql"'));
    expect(schema).toBeDefined();
    const t = await createTestLoraGraphQL({ typeDefs: schema!.body });

    let ran = 0;
    let compared = 0;
    for (const [i, block] of all.entries()) {
      if (block.language !== "graphql" || block === schema) continue;
      if (!/^\s*(\{|query\b|mutation\b)/.test(block.body)) continue;
      const result = await t.lora.execute({ source: block.body });
      expect(result.errors, block.body).toBeUndefined();
      ran++;
      const printed = all[i + 1];
      if (printed?.language === "json") {
        expect(withoutTimestamps(result.data), block.body).toEqual(
          withoutTimestamps(JSON.parse(printed.body)),
        );
        compared++;
      }
    }
    expect(ran).toBeGreaterThan(10);
    expect(compared).toBeGreaterThan(8);
  });

  // A scenario page: `callers.json` names the callers' claims; every
  // `schema.graphql` block starts a fresh database, `seed` blocks fill
  // it, and a request titled "As <caller>", "Anonymous" or "Request"
  // (optionally followed by a `variables` block) must answer with the
  // JSON printed after it. `guarded` pages teach access control, so each
  // of their schemas must also be one `lora-graphql check` accepts.
  test.each([
    { file: "authentication.md", guarded: true, least: [5, 10] },
    { file: "authorization-recipes.md", guarded: true, least: [5, 30] },
    { file: "examples.md", guarded: false, least: [1, 35] },
    // The tutorial's early steps are open on purpose; the finished
    // schema is checked by the example's own test.
    { file: "tutorial.md", guarded: false, least: [5, 15] },
  ])(
    "$file answers each request as printed",
    async ({ file, guarded, least }) => {
      const all = blocks(read(file));
      const callers = JSON.parse(
        all.find((b) => title(b) === "callers.json")?.body ?? "{}",
      ) as Record<string, Record<string, unknown>>;
      let t: Awaited<ReturnType<typeof createTestLoraGraphQL>> | undefined;
      // A `seed.cypher` block is the file a server loads at every start:
      // it is loaded again for each schema that follows it.
      let seedFile: string | undefined;
      let scenarios = 0;
      let requests = 0;
      for (const [i, block] of all.entries()) {
        const name = title(block);
        if (block.language === "graphql" && name === "schema.graphql") {
          t?.close();
          t = await createTestLoraGraphQL({
            typeDefs: block.body,
            maskErrors: false,
          });
          const report = await t.lora.check();
          if (guarded) expect(report.security, block.body).toEqual([]);
          expect(report.cypher, block.body).toEqual([]);
          if (seedFile !== undefined) await t.db.execute(seedFile);
          scenarios++;
          continue;
        }
        if (!t || name === undefined) continue;
        if (block.language === "cypher" && name === "seed") {
          await t.db.execute(block.body);
          continue;
        }
        if (block.language === "cypher" && name === "seed.cypher") {
          seedFile = block.body;
          await t.db.execute(block.body);
          continue;
        }
        if (block.language !== "graphql") continue;
        const anonymous = name === "Anonymous" || name === "Request";
        if (!anonymous && !name.startsWith("As ")) continue;
        const jwt = anonymous ? undefined : callers[name.slice(3)];
        if (!anonymous) expect(jwt, `unknown caller: ${name}`).toBeDefined();

        let printed = all[i + 1];
        let variables: Record<string, unknown> | undefined;
        if (printed && title(printed) === "variables") {
          variables = JSON.parse(printed.body) as Record<string, unknown>;
          printed = all[i + 2];
        }
        const result = await t.lora.execute({
          source: block.body,
          ...(variables ? { variables } : {}),
          context: jwt ? { jwt } : {},
        });
        expect(printed?.language, `no response after:\n${block.body}`).toBe(
          "json",
        );
        const expected = JSON.parse(printed!.body) as Record<string, unknown>;
        // An error without a code is graphql-js's own (validation, syntax)
        // and its wording differs between graphql 16 and 17: hold the page
        // to there being one, not to its text.
        if (Array.isArray(expected["errors"])) {
          expected["errors"] = (
            expected["errors"] as Array<Record<string, unknown>>
          ).map((error) => ("extensions" in error ? error : {}));
        }
        const actual = JSON.parse(JSON.stringify(result)) as Record<
          string,
          unknown
        >;
        // The page prints errors or their absence; hold it to both.
        expect("errors" in actual, `${name}:\n${block.body}`).toBe(
          "errors" in expected,
        );
        if (!shows(actual, expected)) {
          expect(actual, `${name}:\n${block.body}`).toEqual(expected);
        }
        requests++;
      }
      t?.close();
      expect(scenarios).toBeGreaterThanOrEqual(least[0]!);
      expect(requests).toBeGreaterThanOrEqual(least[1]!);
    },
  );
});
