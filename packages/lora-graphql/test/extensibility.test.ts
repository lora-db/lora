// Phase 17: custom resolvers, custom scalars and typed tooling.

import { describe, expect, test } from "vitest";
import { festivalHarness } from "./harness.js";
import { createDatabase } from "@loradb/lora-node";
import { graphql, GraphQLError, GraphQLScalarType, Kind } from "graphql";
import { LoraGraphQL, loraDriver } from "../src/index.js";

describe("@customResolver", () => {
  const typeDefs = /* GraphQL */ `
    type Festival @node {
      key: String! @key
      name: String!
      capacity: Int
      genre: Genre @relationship(type: "IN_GENRE", direction: OUT)
      label: String! @customResolver(requires: "name capacity genre { name }")
    }
    type Genre @node {
      key: String! @key
      name: String!
    }
  `;

  async function setup(
    resolvers: Record<
      string,
      Record<string, (src: Record<string, unknown>) => unknown>
    >,
  ) {
    const db = await createDatabase();
    const lora = new LoraGraphQL({
      typeDefs,
      driver: loraDriver(db),
      resolvers,
    });
    await lora.assertSchema({ create: true });
    await db.execute(
      "CREATE (f:Festival {key: 'f1', name: 'Sunland', capacity: 500})-[:IN_GENRE]->(:Genre {key: 'g1', name: 'Techno'})",
    );
    return lora;
  }

  test("the resolver reads its requires from the same statement", async () => {
    const lora = await setup({
      Festival: {
        label: (src) =>
          `${src["name"]} (${(src["genre"] as { name: string }).name}, ${src["capacity"]})`,
      },
    });
    const r = await graphql({
      schema: lora.getSchema(),
      source: `{ festivals { key label } }`,
      contextValue: {},
    });
    expect(r).toEqual({
      data: { festivals: [{ key: "f1", label: "Sunland (Techno, 500)" }] },
    });
  });

  test("a missing resolver or an invalid requires fails at startup", async () => {
    await expect(setup({})).rejects.toThrow(
      "Festival.label: @customResolver needs resolvers.Festival.label",
    );
    const bad = new LoraGraphQL({
      typeDefs: typeDefs.replace("name capacity genre { name }", "name nope"),
      driver: loraDriver(await createDatabase()),
      resolvers: { Festival: { label: () => "" } },
    });
    expect(() => bad.getSchema()).toThrow(
      /requires: Cannot query field "nope"/,
    );
  });
});

describe("custom scalars", () => {
  const typeDefs = /* GraphQL */ `
    scalar Email @storedAs(type: STRING)
    scalar Instant @storedAs(type: DATETIME)
    type User @node @mutation {
      key: String! @key
      email: Email! @filterable(byValue: [EQ, CONTAINS]) @sortable
      seen: Instant
    }
  `;
  const check = (v: unknown) => {
    if (typeof v !== "string" || !v.includes("@")) {
      throw new GraphQLError(`not an e-mail address: ${String(v)}`);
    }
    return v;
  };
  const Email = new GraphQLScalarType({
    name: "Email",
    serialize: check,
    parseValue: check,
    parseLiteral: (ast) => check(ast.kind === Kind.STRING ? ast.value : null),
  });

  test("stored as their storage type, validated by the given implementation", async () => {
    const db = await createDatabase();
    const lora = new LoraGraphQL({
      typeDefs,
      driver: loraDriver(db),
      scalars: { Email },
    });
    await lora.assertSchema({ create: true });
    const run = (source: string) =>
      graphql({ schema: lora.getSchema(), source, contextValue: {} });
    const ok = await run(`mutation {
      createUsers(input: [{ key: "u1", email: "ann@example.com", seen: "2026-09-29T10:00:00Z" }]) {
        users { email seen }
      }
    }`);
    expect(ok.errors).toBeUndefined();
    expect(ok.data).toEqual({
      createUsers: {
        users: [{ email: "ann@example.com", seen: "2026-09-29T10:00:00Z" }],
      },
    });
    const bad = await run(
      `mutation { createUsers(input: [{ key: "u2", email: "nope" }]) { info { nodesCreated } } }`,
    );
    expect(bad.errors?.[0]?.message).toContain("not an e-mail address: nope");
    const found = await run(
      `{ users(where: { email: { contains: "example" } }) { key } }`,
    );
    expect(found.data).toEqual({ users: [{ key: "u1" }] });
    const stored = await db.execute("MATCH (u:User) RETURN u.email AS e");
    expect(stored.rows).toEqual([{ e: "ann@example.com" }]);
  });

  test("a custom scalar without @storedAs is an error", () => {
    expect(
      () =>
        new LoraGraphQL({
          typeDefs: `scalar Blob type A @node { key: String! @key b: Blob }`,
          driver: {} as never,
        }),
    ).toThrow("a custom scalar needs @storedAs(type:)");
  });
});

void festivalHarness;

describe("manifest and generated types", () => {
  const typeDefs = /* GraphQL */ `
    enum Status {
      OPEN
      CLOSED
    }
    interface Event {
      key: String!
      title: String!
    }
    type Concert implements Event @node @mutation {
      key: String! @key
      title: String! @filterable(byValue: [EQ, CONTAINS]) @sortable
      band: String
      status: Status
      starts: DateTime
    }
    type Exhibition implements Event @node {
      key: String! @key
      title: String!
      artist: String
    }
  `;
  const operations = {
    "concerts#Page": `query Page($where: ConcertWhere, $limit: Int = 10) {
      concerts(where: $where, limit: $limit) { ...Card status starts }
    }
    fragment Card on Concert { key title }`,
    "events#Mixed": `query Mixed { events { __typename key ... on Concert { band } ... on Exhibition { artist } } }`,
    "concerts#Add": `mutation Add($key: String!, $title: String!) {
      createConcerts(input: [{ key: $key, title: $title }]) { info { nodesCreated } }
    }`,
  };

  async function setup(defs = typeDefs) {
    const db = await createDatabase();
    const lora = new LoraGraphQL({ typeDefs: defs, driver: loraDriver(db) });
    await lora.assertSchema({ create: true });
    return { db, lora };
  }

  test("types compile and describe results and variables", async () => {
    const { lora } = await setup();
    const manifest = lora.buildManifest(operations);
    const types = lora.generateTypes(manifest);
    expect(types).toContain('export type Status = "OPEN" | "CLOSED";');
    expect(types).toContain("export interface PageVariables {");
    const { mkdtemp, writeFile } = await import("node:fs/promises");
    const { join } = await import("node:path");
    const { tmpdir } = await import("node:os");
    const { execFileSync } = await import("node:child_process");
    const dir = await mkdtemp(join(tmpdir(), "lora-graphql-types-"));
    await writeFile(join(dir, "operations.d.ts"), types);
    await writeFile(
      join(dir, "use.ts"),
      `import type { PageResult, PageVariables, MixedResult, AddVariables } from "./operations";
const v: PageVariables = { where: { title: { contains: "x" }, AND: [{ title: { eq: "y" } }] } };
const r: PageResult = { concerts: [{ key: "k", title: "t", status: null, starts: "2026-01-01T00:00:00Z" }] };
const m: MixedResult = { events: [{ __typename: "Concert", key: "k", band: "b" }] };
const a: AddVariables = { key: "k", title: "t" };
// @ts-expect-error: key is required
const bad: AddVariables = { title: "t" };
export { v, r, m, a, bad };
`,
    );
    const { createRequire } = await import("node:module");
    const tsc = createRequire(import.meta.url).resolve("typescript/bin/tsc");
    try {
      execFileSync(
        process.execPath,
        [tsc, "--noEmit", "--strict", join(dir, "use.ts")],
        {
          stdio: "pipe",
        },
      );
    } catch (err) {
      const e = err as { stdout?: Buffer };
      throw new Error(`${String(e.stdout)}\n---\n${types}`);
    }
  }, 60_000);

  test("loadManifest registers operations; a manifest for another schema is refused", async () => {
    const { lora } = await setup();
    const manifest = JSON.parse(JSON.stringify(lora.buildManifest(operations)));
    const fresh = (await setup()).lora;
    fresh.loadManifest(manifest);
    const added = await fresh.execute({
      id: "concerts#Add",
      variables: { key: "c1", title: "Opening" },
    });
    expect(added.errors).toBeUndefined();
    const page = await fresh.execute({ id: "concerts#Page" });
    expect((page.data as { concerts: unknown[] }).concerts).toHaveLength(1);

    const other = (
      await setup(
        typeDefs.replace("band: String", "band: String\n      rating: Int"),
      )
    ).lora;
    expect(() => other.loadManifest(manifest)).toThrow(
      "built for a different schema",
    );
  });
});
