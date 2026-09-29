import { createHmac, createHash } from "node:crypto";
import { createDatabase } from "@loradb/lora-node";
import { graphql, parse, validate, specifiedRules } from "graphql";
import { beforeEach, describe, expect, test } from "vitest";
import {
  envelopPlugin,
  LoraGraphQL,
  loraDriver,
  validationRules,
  type DatabaseErrorEvent,
  type LoraDriver,
} from "../src/index.js";
import { sameValue } from "../src/compile/auth.js";
import { decodeCursor, encodeCursor } from "../src/compile/cursor.js";
import { hmacSha256, sha256 } from "../src/compile/hmac.js";

const typeDefs = /* GraphQL */ `
  type Doc
    @node
    @mutation
    @authorization(
      filter: [{ where: { node: { tenant: { eq: "$context.tenant" } } } }]
    ) {
    key: String! @key
    tenant: String!
    title: String @sortable
  }

  type Query {
    broken: Int @cypher(statement: "RETURN 1 AS n", columnName: "n")
  }
`;

async function setup(options: Record<string, unknown> = {}) {
  const db = await createDatabase();
  const base = loraDriver(db);
  // A driver that fails any statement containing "RETURN 1 AS n" the way
  // the engine does: with a message naming Cypher internals.
  const driver: LoraDriver = {
    ...base,
    run: (statements, opts) =>
      statements.some((s) => s.text.includes("RETURN 1 AS n"))
        ? Promise.reject(
            Object.assign(
              new Error("LORA_INTERNAL: property :Doc(secretIndex) failed"),
              { code: "LORA_INTERNAL" },
            ),
          )
        : base.run(statements, opts),
  };
  const lora = new LoraGraphQL({ typeDefs, driver, ...options });
  await lora.assertSchema({ create: true });
  await db.execute(
    "UNWIND range(1, 5) AS i CREATE (:Doc {key: 'd' + toString(i), tenant: 't1', title: 'T' + toString(i)})",
  );
  const schema = lora.getSchema();
  const run = (
    source: string,
    contextValue: unknown = { tenant: "t1", jwt: { sub: "a" } },
  ) => graphql({ schema, source, contextValue });
  return { lora, run };
}

describe("HMAC-SHA-256", () => {
  test("matches node:crypto", () => {
    const enc = new TextEncoder();
    for (const len of [0, 1, 55, 56, 63, 64, 65, 1000]) {
      const data = new Uint8Array(len).map((_, i) => (i * 31) & 0xff);
      expect(Buffer.from(sha256(data)).toString("hex")).toBe(
        createHash("sha256").update(data).digest("hex"),
      );
      for (const key of ["k", "x".repeat(64), "y".repeat(100)]) {
        expect(
          Buffer.from(hmacSha256(enc.encode(key), data)).toString("hex"),
        ).toBe(createHmac("sha256", key).update(data).digest("hex"));
      }
    }
  });
});

describe("cursors", () => {
  test("signed cursors round-trip and reject tampering", () => {
    const signed = encodeCursor("title:ASC", ["T1", "d1"], "s3cret");
    expect(decodeCursor(signed, "title:ASC", 2, "s3cret")).toEqual([
      "T1",
      "d1",
    ]);

    const forged = encodeCursor("title:ASC", ["T4", "d4"]);
    const [, sig] = signed.split(".");
    for (const bad of [forged, `${forged}.${sig}`, `${signed}x`, "", "."]) {
      expect(() => decodeCursor(bad, "title:ASC", 2, "s3cret")).toThrow(
        "not a valid cursor",
      );
    }
    expect(() => decodeCursor(signed, "title:ASC", 2, "other")).toThrow(
      "not a valid cursor",
    );
  });

  test("a configured secret signs connection cursors end to end", async () => {
    const { run } = await setup({ cursorSecret: "s3cret" });
    const page = await run(
      `{ docsConnection(first: 2, sort: [{ title: ASC }]) { pageInfo { endCursor } } }`,
    );
    const end = (
      page.data as { docsConnection: { pageInfo: { endCursor: string } } }
    ).docsConnection.pageInfo.endCursor;
    expect(end).toContain(".");
    const next = await run(
      `{ docsConnection(first: 2, after: "${end}", sort: [{ title: ASC }]) { edges { node { key } } } }`,
    );
    expect(next.errors).toBeUndefined();
    const forged = await run(
      `{ docsConnection(first: 2, after: "${end.split(".")[0]}", sort: [{ title: ASC }]) { edges { node { key } } } }`,
    );
    expect(forged.errors?.[0]?.extensions?.["code"]).toBe("INVALID_CURSOR");
  });
});

describe("claim and context lookups", () => {
  test("$context reads own properties only", async () => {
    const { run } = await setup();
    const own = await run(`{ docs { key } }`, {
      tenant: "t1",
      jwt: { sub: "a" },
    });
    expect((own.data as { docs: unknown[] }).docs).toHaveLength(5);
    const inherited = await run(
      `{ docs { key } }`,
      Object.assign(Object.create({ tenant: "t1" }) as object, {
        jwt: { sub: "a" },
      }),
    );
    expect((inherited.data as { docs: unknown[] }).docs).toEqual([]);
  });

  test("claim equality is structural", () => {
    expect(sameValue({ a: 1, b: [1, 2] }, { b: [1, 2], a: 1 })).toBe(true);
    expect(sameValue({ a: 1 }, { a: 1, b: undefined })).toBe(false);
    expect(sameValue([1, 2], [2, 1])).toBe(false);
    expect(sameValue(3n, 3)).toBe(true);
    expect(sameValue(Object.create({ a: 1 }), {})).toBe(true);
    expect(sameValue(null, {})).toBe(false);
  });
});

describe("error masking", () => {
  let events: DatabaseErrorEvent[];
  beforeEach(() => {
    events = [];
  });

  test("masked: the client gets a code and an id, onError the detail", async () => {
    const { run } = await setup({
      maskErrors: true,
      onError: (e: DatabaseErrorEvent) => events.push(e),
    });
    const r = await run(`{ broken }`);
    const error = r.errors![0]!;
    expect(error.extensions["code"]).toBe("DATABASE_ERROR");
    expect(error.message).not.toContain("secretIndex");
    // graphql-js wraps what a resolver throws; the masked error has no cause.
    expect(error.originalError?.message).toBe(error.message);
    expect(
      (error.originalError as { originalError?: unknown }).originalError,
    ).toBeUndefined();
    expect(events).toHaveLength(1);
    expect(error.extensions["id"]).toBe(events[0]!.id);
    expect(events[0]).toMatchObject({ field: "broken" });
    expect(events[0]!.message).toContain("secretIndex");
  });

  test("unmasked: the message stays, with an id", async () => {
    const { run } = await setup({ maskErrors: false });
    const error = (await run(`{ broken }`)).errors![0]!;
    expect(error.message).toContain("secretIndex");
    expect(typeof error.extensions["id"]).toBe("string");
  });
});

describe("document guards", () => {
  test("execute() enforces depth, aliases, root fields and tokens", async () => {
    const { lora } = await setup({
      guards: { maxDepth: 2, maxAliases: 2, maxRootFields: 2, maxTokens: 60 },
    });
    const code = async (source: string) =>
      (
        await lora.execute({
          source,
          context: { tenant: "t1", jwt: { sub: "a" } },
        })
      ).errors?.[0]?.message;
    expect(await code(`{ docs { key } }`)).toBeUndefined();
    expect(
      await code(`{ docsConnection { edges { node { key } } } }`),
    ).toContain("deeper than 2");
    expect(await code(`{ docs { a: key b: key c: key } }`)).toContain(
      "more than 2 aliased",
    );
    expect(
      await code(
        `{ docs { key } docsConnection { totalCount } docsAggregate { count } }`,
      ),
    ).toContain("more than 2 root fields");
    expect(await code(`{ docs { ${"key ".repeat(80)} } }`)).toContain("token");
  });

  test("fragments count toward depth, and reusing one is cheap", () => {
    const schema = new LoraGraphQL({
      typeDefs,
      driver: {} as LoraDriver,
    }).getSchema();
    const levels = Array.from({ length: 20 }, (_, i) =>
      i === 0
        ? `fragment F0 on Doc { key }`
        : `fragment F${i} on Doc { ...F${i - 1} ...F${i - 1} ...F${i - 1} }`,
    ).join("\n");
    const doc = parse(`{ docs { ...F19 } }\n${levels}`);
    const start = performance.now();
    const errors = validate(schema, doc, [
      ...specifiedRules.filter(
        (r) => r.name !== "OverlappingFieldsCanBeMergedRule",
      ),
      ...validationRules({ maxDepth: 3 }),
    ]);
    expect(performance.now() - start).toBeLessThan(500);
    expect(errors.filter((e) => e.message.includes("deeper"))).toEqual([]);
    const deep = parse(
      `{ docsConnection { ...E } } fragment E on DocConnection { edges { node { key } } }`,
    );
    expect(
      validate(schema, deep, validationRules({ maxDepth: 2 })).map(
        (e) => e.message,
      ),
    ).toEqual(["the operation nests fields deeper than 2 levels"]);
  });

  test("introspection can be turned off; __typename stays", async () => {
    const { lora } = await setup({ guards: { introspection: false } });
    const r = await lora.execute({
      source: `{ __schema { queryType { name } } }`,
    });
    expect(r.errors?.[0]?.message).toBe("introspection is disabled");
    const t = await lora.execute({
      source: `{ docs { __typename } }`,
      context: { tenant: "t1", jwt: { sub: "a" } },
    });
    expect(t.errors).toBeUndefined();
  });

  test("persistedOnly refuses ad-hoc documents", async () => {
    const { lora } = await setup({ persistedOnly: true });
    lora.persist({ list: `{ docs { key } }` });
    const adhoc = await lora.execute({ source: `{ docs { key } }` });
    expect(adhoc.errors?.[0]?.extensions?.["code"]).toBe(
      "PERSISTED_QUERY_ONLY",
    );
    const byId = await lora.execute({
      id: "list",
      context: { tenant: "t1", jwt: { sub: "a" } },
    });
    expect(byId.errors).toBeUndefined();
  });

  test("the Envelop plugin adds the rules and the token limit", () => {
    const plugin = envelopPlugin({ maxDepth: 1, maxTokens: 10 });
    const rules: unknown[] = [];
    plugin.onValidate({ addValidationRule: (r) => rules.push(r) });
    expect(rules.length).toBeGreaterThanOrEqual(3);
    let wrapped: ((s: unknown, o?: object) => unknown) | undefined;
    plugin.onParse({
      parseFn: (s, o) => parse(s as string, o),
      setParseFn: (fn) => (wrapped = fn),
    });
    expect(() => wrapped!(`{ a b c d e f g h i j k }`)).toThrow(/token/i);
  });
});
