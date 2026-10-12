// Reads compiled for one request are reused by later ones only when the
// statement text would be the same: same field node (documents are
// cached), variables, claims and `$context` values.

import { createDatabase } from "@loradb/lora-node";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type StatementEvent } from "../src/index.js";
import { GraphQLError, parse as parseDocument, type FieldNode } from "graphql";
import { CompileCache } from "../src/compile/cache.js";
import type { CompileContext } from "../src/compile/context.js";
import type { CompiledRead } from "../src/compile/read/types.js";

const typeDefs = /* GraphQL */ `
  type Doc
    @node
    @authorization(
      filter: [
        {
          where: {
            AND: [
              { node: { tenant: { eq: "$context.tenant" } } }
              {
                OR: [
                  { node: { owner: { eq: "$jwt.sub" } } }
                  { jwt: { roles: { includes: "admin" } } }
                ]
              }
            ]
          }
        }
      ]
    ) {
    key: String! @key
    tenant: String!
    owner: String! @filterable
  }
`;

async function setup() {
  const db = await createDatabase();
  const statements: StatementEvent[] = [];
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    onStatement: (e) => statements.push(e),
  });
  await lora.assertSchema({ create: true });
  await db.execute(
    `CREATE (:Doc {key: 'a1', tenant: 't1', owner: 'a'}), (:Doc {key: 'b1', tenant: 't1', owner: 'b'}),
            (:Doc {key: 'a2', tenant: 't2', owner: 'a'})`,
  );
  const keys = async (
    context: object,
    variables: Record<string, unknown> = {},
  ) => {
    const r = await lora.execute({
      source: `query($owner: String) { docs(where: { owner: { eq: $owner } }) { key } }`,
      variables,
      context,
    });
    if (r.errors) throw r.errors[0];
    return (r.data as { docs: Array<{ key: string }> }).docs
      .map((d) => d.key)
      .sort();
  };
  return { keys, statements };
}

test("claims, context values and variables each get their own compile", async () => {
  const { keys, statements } = await setup();
  const alice = { jwt: { sub: "a" }, tenant: "t1" };
  expect(await keys(alice)).toEqual(["a1"]);
  expect(await keys({ ...alice })).toEqual(["a1"]);
  // The same statement text was reused: identical, parameters included.
  expect(statements[1]!.statement).toEqual(statements[0]!.statement);

  expect(
    await keys({ jwt: { sub: "a", roles: ["admin"] }, tenant: "t1" }),
  ).toEqual(["a1", "b1"]);
  expect(await keys({ jwt: { sub: "a" }, tenant: "t2" })).toEqual(["a2"]);
  expect(await keys({ tenant: "t1" })).toEqual([]);
  expect(await keys(alice, { owner: "b" })).toEqual([]);
  expect(
    await keys({ jwt: { sub: "b" }, tenant: "t1" }, { owner: "b" }),
  ).toEqual(["b1"]);
});

test("a context without the value a rule reads is not served a cached compile", async () => {
  const { keys } = await setup();
  expect(await keys({ jwt: { sub: "a" }, tenant: "t1" })).toEqual(["a1"]);
  expect(await keys({ jwt: { sub: "a" } })).toEqual([]);
  expect(
    await keys(
      Object.assign(Object.create({ tenant: "t1" }), { jwt: { sub: "a" } }),
    ),
  ).toEqual([]);
});

test("per-user tokens share one compile when a rule only binds the claim", async () => {
  const { keys, statements } = await setup();
  for (let round = 0; round < 2; round++) {
    expect(await keys({ jwt: { sub: "a", iat: round }, tenant: "t1" })).toEqual(
      ["a1"],
    );
    expect(await keys({ jwt: { sub: "b", jti: "x" }, tenant: "t1" })).toEqual([
      "b1",
    ]);
    expect(await keys({ jwt: { sub: "c" }, tenant: "t1" })).toEqual([]);
    // A claim of another type is not rebound: it compiles on its own.
    expect(await keys({ jwt: { sub: 7 }, tenant: "t1" })).toEqual([]);
    expect(await keys({ jwt: {}, tenant: "t1" })).toEqual([]);
  }
  // One statement text for every string subject.
  const texts = new Set(
    statements
      .filter((e) => typeof e.statement.params["p1"] === "string")
      .map((e) => e.statement.text),
  );
  expect(texts.size).toBe(1);
});

// ---------------------------------------------------------------------------
// The cache itself, with a stand-in compile that counts its calls.
// ---------------------------------------------------------------------------

function fieldOf(source: string): FieldNode[] {
  const doc = parseDocument(source);
  const op = doc.definitions[0] as unknown as {
    selectionSet: { selections: FieldNode[] };
  };
  return [op.selectionSet.selections[0]!];
}

/** A compile that reads `sub` (bound into a filter) and, if asked, `roles`. */
function fakeCompile(readRoles: boolean) {
  let calls = 0;
  const compile = (jwt: Record<string, unknown> | undefined) => {
    calls++;
    const claimReads = new Map<string, { value: unknown; bound: boolean }>();
    const params: Record<string, unknown> = { p0: 25 };
    let text = "MATCH (n) WHERE n.owner = $p1 RETURN n LIMIT $p0";
    params["p1"] = jwt?.["sub"];
    claimReads.set("sub", { value: jwt?.["sub"], bound: true });
    if (readRoles) {
      const admin = (jwt?.["roles"] as string[] | undefined)?.includes("admin");
      claimReads.set("roles", { value: jwt?.["roles"], bound: false });
      if (admin) text = "MATCH (n) RETURN n LIMIT $p0";
    }
    const ctx = { claimReads, contextReads: [] } as unknown as CompileContext;
    const compiled = {
      statements: [{ text, params }],
      cost: 25,
      columns: ["n"],
    } as unknown as CompiledRead;
    return { ctx, compiled };
  };
  return { compile, calls: () => calls };
}

const request = (
  fieldNodes: FieldNode[],
  jwt: Record<string, unknown> | undefined,
  variables: Record<string, unknown> = {},
) => ({
  fieldNodes,
  fragments: {},
  variables,
  jwt,
  context: {},
  statistics: 0,
});

test("the cache keys on the claims the compile read, rebinding bound ones", () => {
  const cache = new CompileCache();
  const field = fieldOf(`{ docs { key } }`);
  const { compile, calls } = fakeCompile(true);
  // An anonymous request first does not stop later tokens sharing.
  cache.get(request(field, undefined), compile);
  expect(calls()).toBe(1);
  for (let i = 0; i < 100; i++) {
    const got = cache.get(
      request(field, { sub: `user${i}`, iat: i, roles: ["user"] }),
      compile,
    );
    expect(got.statements[0]!.params["p1"]).toBe(`user${i}`);
  }
  // The first compile keys on the value and notes that `sub` was bound;
  // the second compiles with a marker, proving `sub` only a parameter.
  expect(calls()).toBe(3);
  const admin = cache.get(
    request(field, { sub: "root", roles: ["admin"] }),
    compile,
  );
  expect(admin.statements[0]!.text).not.toContain("owner");
  expect(calls()).toBe(4);
  // Unauthenticated requests never share an authenticated compile.
  cache.get(request(field, undefined), compile);
  expect(calls()).toBe(4);
});

test("the cache keys on the variables the field uses, and caps its size", () => {
  const cache = new CompileCache(3);
  const field = fieldOf(`query($a: Int, $b: Int) { docs(limit: $a) { key } }`);
  const { compile, calls } = fakeCompile(false);
  cache.get(request(field, { sub: "x" }, { a: 1, b: 1 }), compile);
  cache.get(request(field, { sub: "y" }, { a: 1, b: 2 }), compile);
  expect(calls()).toBe(2);
  // `$b` is not used: the same variables key.
  cache.get(request(field, { sub: "y" }, { a: 1, b: 3 }), compile);
  expect(calls()).toBe(2);
  cache.get(request(field, { sub: "y" }, { a: 2, b: 2 }), compile);
  expect(calls()).toBe(3);
  for (let a = 3; a < 10; a++) {
    cache.get(request(field, { sub: "y" }, { a }), compile);
  }
  expect(cache.size).toBe(3);
});

test("the cache is bounded by bytes, and skips requests with large variables", () => {
  const cache = new CompileCache(4096, 8 * 1024);
  const field = fieldOf(
    `query($a: [String!]) { docs(where: { key: { in: $a } }) { key } }`,
  );
  const { compile, calls } = fakeCompile(false);
  for (let i = 0; i < 50; i++) {
    cache.get(request(field, undefined, { a: [`key${i}`] }), compile);
  }
  // Each entry is about 1 KiB (its overhead): eight fit, at most.
  expect(cache.size).toBeLessThanOrEqual(8);
  expect(cache.size).toBeGreaterThan(0);
  expect(cache.bytes).toBeLessThanOrEqual(8 * 1024);

  // A 10 000-item list is compiled every time and never kept.
  const big = new CompileCache();
  const list = Array.from({ length: 10_000 }, (_, i) => `key${i}`);
  const before = calls();
  big.get(request(field, undefined, { a: list }), compile);
  big.get(request(field, undefined, { a: list }), compile);
  expect(calls() - before).toBe(2);
  expect(big.size).toBe(0);
});

// ---------------------------------------------------------------------------
// The key of a lookup by @key, rebound per request.
// ---------------------------------------------------------------------------

/** A compile of `doc(key: $k)`: the key bound whole, as a lookup does. */
function keyCompile(
  fail: (jwt: unknown) => Error | undefined = () => undefined,
) {
  let calls = 0;
  const compile =
    (real: Record<string, unknown>) =>
    (
      jwt: Record<string, unknown> | undefined,
      variables?: Record<string, unknown>,
    ) => {
      calls++;
      const error = fail(jwt);
      if (error) throw error;
      const ctx = {
        claimReads: new Map(),
        contextReads: [],
      } as unknown as CompileContext;
      const compiled = {
        statements: [
          {
            text: "MATCH (n) WHERE n.key = $p0 RETURN n",
            params: { p0: { ...real, ...variables }["k"] },
          },
        ],
        cost: 1,
        columns: ["n"],
      } as unknown as CompiledRead;
      return { ctx, compiled };
    };
  return { compile, calls: () => calls };
}

const lookup = (fieldNodes: FieldNode[], k: unknown, authenticated = true) => ({
  ...request(fieldNodes, authenticated ? { sub: "a" } : undefined, { k }),
  rebindable: ["k"],
});

test("one compile serves every key of a lookup by @key", () => {
  const cache = new CompileCache();
  const field = fieldOf(`query($k: String!) { doc(key: $k) { key } }`);
  const { compile, calls } = keyCompile();
  for (const k of ["a", "b", "c"]) {
    const got = cache.get(lookup(field, k), compile({ k }));
    expect(got.statements[0]!.params["p0"]).toBe(k);
  }
  expect(calls()).toBe(1);
  expect(cache.size).toBe(1);
});

test("a request that fails first does not stop later keys sharing", () => {
  const cache = new CompileCache();
  const field = fieldOf(`query($k: String!) { doc(key: $k) { key } }`);
  const { compile, calls } = keyCompile((jwt) =>
    jwt ? undefined : new GraphQLError("needs an authenticated request"),
  );
  expect(() =>
    cache.get(lookup(field, "a", false), compile({ k: "a" })),
  ).toThrow("needs an authenticated request");
  const before = calls();
  for (const k of ["a", "b", "c"]) cache.get(lookup(field, k), compile({ k }));
  expect(calls() - before).toBe(1);
});

test("a key variable the field uses again is keyed on, not rebound", () => {
  const cache = new CompileCache();
  const field = fieldOf(
    `query($k: String!) { doc(key: $k) { parts(where: { name: { eq: $k } }) { key } } }`,
  );
  const { compile, calls } = keyCompile();
  for (const k of ["a", "b", "a"]) {
    const got = cache.get(lookup(field, k), compile({ k }));
    expect(got.statements[0]!.params["p0"]).toBe(k);
  }
  expect(calls()).toBe(2);
});

test("an error that is not a request's is not taken for a refused marker", () => {
  const cache = new CompileCache();
  const field = fieldOf(`query($k: String!) { doc(key: $k) { key } }`);
  const { compile, calls } = keyCompile(() => new TypeError("a bug"));
  expect(() => cache.get(lookup(field, "a"), compile({ k: "a" }))).toThrow(
    TypeError,
  );
  expect(calls()).toBe(1);
});

test("a lookup by @key reads the right node for each key, limits included", async () => {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs: /* GraphQL */ `
      type Folder @node {
        key: String! @key
        files: [File!]! @relationship(type: "HAS", direction: OUT)
      }
      type File @node {
        key: String! @key
        name: String! @filterable
      }
    `,
    driver: loraDriver(db),
    maxStringFilter: 20,
  });
  await lora.assertSchema({ create: true });
  await db.execute(
    `CREATE (:Folder {key: 'a'})-[:HAS]->(:File {key: 'f', name: 'a'}), (:Folder {key: 'b'})`,
  );
  const run = (source: string, k: string) =>
    lora.execute({ source, variables: { k } });

  const plain = `query($k: String!) { folder(key: $k) { key } }`;
  for (const k of ["a", "b", "a"]) {
    expect((await run(plain, k)).data).toEqual({ folder: { key: k } });
  }
  expect((await run(plain, "none")).data).toEqual({ folder: null });

  // The key used again in a filter is still held to the filter's limit.
  const again = `query($k: String!) {
    folder(key: $k) { files(where: { name: { eq: $k } }) { key } }
  }`;
  expect((await run(again, "a")).data).toEqual({
    folder: { files: [{ key: "f" }] },
  });
  const long = await run(again, "x".repeat(50));
  expect(long.errors?.[0]?.message).toMatch(/at most 20 characters/);
});
