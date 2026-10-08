// What the tools know about validation rules: they show in the public
// schema, they are exported as JSON Schema for clients, a schema diff
// sees one tightened, check() lints the usual mistakes and lists stored
// values that would be refused today.

import { describe, expect, test } from "vitest";
import { buildModel, diffSchemas, inputSchemas } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  enum Plan {
    FREE
    PRO
  }
  type Account @node @mutation {
    key: String! @key @length(max: 20) @pattern(regex: "[a-z0-9-]+")
    "How the account signs in."
    email: String!
      @normalize(trim: true, case: LOWER)
      @format(as: "email")
      @unique
    name: String @length(min: 1, max: 80)
    age: Int @range(min: 0, exclusiveMax: 150)
    price: Float @multipleOf(value: 0.01)
    plan: Plan
    born: Date
    tags: [String!] @size(max: 3, unique: true) @length(max: 10)
    members: [Account!]!
      @relationship(type: "HAS", direction: OUT, properties: "Seat")
  }
  type Seat @relationshipProperties {
    role: String @length(max: 10)
  }
`;

describe("the public schema", () => {
  test("an input field says its rules in words", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const create = t.schema.getType("AccountCreateInput") as unknown as {
      getFields(): Record<string, { description?: string | null }>;
    };
    const describe = (field: string) => create.getFields()[field]!.description;
    expect(describe("email")).toBe(
      "How the account signs in. Stored trimmed, lower-cased. Must be an email address.",
    );
    expect(describe("name")).toBe("1 to 80 characters.");
    expect(describe("age")).toBe("At least 0. Less than 150.");
    expect(describe("tags")).toBe(
      "At most 3 items. No item twice. Each item: At most 10 characters.",
    );
    expect(describe("key")).toBe(
      "At most 20 characters. Must match `[a-z0-9-]+`.",
    );
    expect(describe("plan")).toBeFalsy();
    const update = t.schema.getType("AccountUpdateInput") as typeof create;
    expect(update.getFields()["name"]!.description).toBe(
      "1 to 80 characters. null removes the value.",
    );
  });
});

describe("inputSchemas()", () => {
  const schemas = inputSchemas(buildModel(typeDefs));

  test("one JSON Schema per create and update input", () => {
    expect(Object.keys(schemas).sort()).toEqual([
      "AccountCreateInput",
      "AccountUpdateInput",
      "SeatCreateInput",
      "SeatUpdateInput",
    ]);
  });

  test("types, required fields and rules", () => {
    const create = schemas["AccountCreateInput"] as {
      required: string[];
      properties: Record<string, Record<string, unknown>>;
    };
    expect(create.required).toEqual(["key", "email"]);
    expect(create.properties["name"]).toEqual({
      type: "string",
      minLength: 1,
      maxLength: 80,
    });
    expect(create.properties["age"]).toEqual({
      type: "integer",
      minimum: 0,
      exclusiveMaximum: 150,
    });
    expect(create.properties["price"]).toEqual({
      type: "number",
      multipleOf: 0.01,
    });
    expect(create.properties["plan"]).toEqual({ enum: ["FREE", "PRO"] });
    expect(create.properties["born"]).toEqual({
      type: "string",
      format: "date",
    });
    expect(create.properties["tags"]).toEqual({
      type: "array",
      items: { type: "string", maxLength: 10 },
      maxItems: 3,
      uniqueItems: true,
    });
  });

  test("a pattern is anchored, and a format carries its expression", () => {
    const { properties } = schemas["AccountCreateInput"] as {
      properties: Record<string, Record<string, unknown>>;
    };
    // Here a pattern matches the whole value; in JSON Schema it need not.
    expect(properties["key"]!["pattern"]).toBe("^(?:[a-z0-9-]+)$");
    expect(properties["email"]).toMatchObject({
      description: "How the account signs in.",
      type: "string",
      format: "email",
      maxLength: 254,
    });
    const pattern = new RegExp(properties["email"]!["pattern"] as string, "u");
    expect(pattern.test("ada@example.com")).toBe(true);
    expect(pattern.test("nope")).toBe(false);
  });

  test("an update's optional field may be null", () => {
    const { properties, required } = schemas["AccountUpdateInput"] as {
      properties: Record<string, Record<string, unknown>>;
      required?: string[];
    };
    expect(required).toBeUndefined();
    expect(properties["name"]).toEqual({
      anyOf: [
        { type: "string", minLength: 1, maxLength: 80 },
        { type: "null" },
      ],
    });
    expect(properties["key"]).toBeUndefined();
  });
});

describe("diffSchemas()", () => {
  const sdl = (rules: string) =>
    `type P @node @mutation { key: String! @key  name: String ${rules}  n: Int }`;
  const changes = (before: string, after: string) => {
    const d = diffSchemas(sdl(before), sdl(after));
    return {
      breaking: d.api.breaking
        .filter((c) => c.type === "RULE_TIGHTENED")
        .map((c) => c.description),
      dangerous: d.api.dangerous
        .filter((c) => c.type === "RULE_CHANGED")
        .map((c) => c.description),
      notes: d.notes,
    };
  };

  test("a tightened rule breaks clients, though no type changed", () => {
    const d = changes("@length(max: 80)", "@length(max: 40)");
    expect(d.breaking).toEqual([
      "P.name: @length(max:) went from 80 to 40. Input the old rule accepted is now refused.",
    ]);
    expect(d.notes.join(" ")).toMatch(
      /values already stored are not revisited/,
    );
  });

  test("a new rule is breaking, a changed format is dangerous", () => {
    expect(changes("", '@format(as: "email")').breaking).toEqual([
      "P.name: @format was added. Input the old rule accepted is now refused.",
    ]);
    expect(
      changes('@format(as: "email")', '@format(as: "email-strict")').dangerous,
    ).toEqual(["P.name: @format changed from email to email-strict."]);
  });

  test("a loosened or removed rule breaks nothing", () => {
    for (const [before, after] of [
      ["@length(max: 40)", "@length(max: 80)"],
      ["@length(min: 2, max: 40)", "@length(max: 40)"],
      ['@format(as: "email")', ""],
    ] as const) {
      const d = changes(before, after);
      expect(d.breaking, `${before} -> ${after}`).toEqual([]);
      expect(d.dangerous).toEqual([]);
    }
  });
});

describe("check()", () => {
  test("lints a @unique string that is not normalised, and a rule on a generated key", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: /* GraphQL */ `
        type Claims @jwt {
          sub: String
        }
        type U @node @mutation @authentication {
          id: ID! @key(generate: true) @length(max: 40)
          email: String @unique
          handle: String @unique @normalize(case: LOWER)
        }
      `,
    });
    const lint = (await t.lora.check()).lint.map(
      (l) => `${l.type}.${l.field}: ${l.message.split(":")[0]}`,
    );
    expect(lint).toEqual([
      "U.id: validation rules on a @key(generate",
      "U.email: @unique on text a client supplies, without @normalize",
    ]);
  });

  test("lists stored values the rules would refuse today, without failing", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      seed: [
        // Written with Cypher, which no rule checks.
        "CREATE (:Account {key: 'ok', email: 'ok@example.com', name: 'Ok', age: 30})",
        "CREATE (:Account {key: 'Bad Key', email: ' Mixed@Example.com', age: 200, tags: ['a', 'a']})",
      ],
    });
    const report = await t.lora.check();
    expect(report.stored.map((v) => [v.key, v.field, v.rule])).toEqual([
      ["Bad Key", "key", "pattern"],
      ["Bad Key", "email", "normalize"],
      ["Bad Key", "email", "format"],
      ["Bad Key", "age", "exclusiveMaximum"],
      ["Bad Key", "tags", "uniqueItems"],
    ]);
    expect(report.stored[0]).toEqual({
      type: "Account",
      key: "Bad Key",
      field: "key",
      rule: "pattern",
      message: "does not match the required pattern",
    });
    // Reported, not a failure: the schema is sound, the data is old.
    expect(report.ok).toBe(report.security.length === 0);
  });

  test("storedViolations() reads a bounded number of nodes per type", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type N @node { key: String! @key  v: Int @range(max: 1) }`,
      seed: ["UNWIND range(1, 5) AS i CREATE (:N {key: toString(i), v: i})"],
    });
    const all = await t.lora.storedViolations();
    expect(all.scanned).toEqual({ N: 5 });
    expect(all.partial).toEqual([]);
    expect(all.violations).toHaveLength(4);
    const some = await t.lora.storedViolations({ limit: 2 });
    expect(some.scanned).toEqual({ N: 2 });
    expect(some.partial).toEqual(["N"]);
  });
});

describe("rules on @cypher arguments", () => {
  const sdl = /* GraphQL */ `
    type Item @node {
      key: String! @key
      name: String
    }
    type Query {
      search(
        q: String! @length(min: 2, max: 20) @pattern(regex: "[a-z ]+")
        kinds: [String!] @size(min: 1, max: 2, unique: true) @length(max: 4)
        ratio: Float @range(exclusiveMin: 0, max: 1)
        contact: String @format(as: "email")
        limit: Int = 10 @range(min: 1, max: 50)
      ): [Item!]!
        @cypher(
          statement: """
          MATCH (i:Item)
          WHERE i.name CONTAINS $q
            AND ($kinds IS NULL OR size($kinds) > 0)
            AND ($ratio IS NULL OR $ratio > 0)
            AND ($contact IS NULL OR size($contact) > 0)
          RETURN i LIMIT $limit
          """
        )
    }
  `;

  test("every broken rule at once, before the statement runs", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: sdl });
    t.statements.length = 0;
    const r = await t.lora.execute({
      source: `{ search(q: "A", kinds: ["a", "a", "toolong"], ratio: 0, contact: "nope") { key } }`,
    });
    const error = r.errors?.[0];
    expect(error?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    expect(
      (
        error?.extensions?.["violations"] as Array<{
          path: unknown[];
          coordinate: string;
          rule: string;
        }>
      ).map((v) => [v.path, v.coordinate, v.rule]),
    ).toEqual([
      [["q"], "Query.search(q:)", "minLength"],
      [["q"], "Query.search(q:)", "pattern"],
      // Over its size, a list is not searched for duplicates as well.
      [["kinds"], "Query.search(kinds:)", "maxItems"],
      [["kinds", 2], "Query.search(kinds:)", "maxLength"],
      [["ratio"], "Query.search(ratio:)", "exclusiveMinimum"],
      [["contact"], "Query.search(contact:)", "format"],
    ]);
    expect(t.statements).toHaveLength(0);
  });

  test("valid arguments run the statement", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: sdl,
      seed: ["CREATE (:Item {key: 'a', name: 'red apple'})"],
    });
    const r = await t.lora.execute({
      source: `{ search(q: "apple", kinds: ["a"], ratio: 0.5, contact: "a@b.co") { key } }`,
    });
    expect(r.data).toEqual({ search: [{ key: "a" }] });
  });

  test("a plain @range keeps its own message", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: sdl });
    const r = await t.lora.execute({
      source: `{ search(q: "apple", limit: 500) { key } }`,
    });
    expect(r.errors?.[0]?.message).toBe(
      "Query.search: argument limit must be between 1 and 50 (got 500)",
    );
  });

  test("startup: the rule must fit the argument, and its default", () => {
    const problems = (args: string): string[] => {
      try {
        buildModel(`type Item @node { key: String! @key }
          type Query { f(${args}): [Item!]! @cypher(statement: "MATCH (i:Item) WHERE $a IS NOT NULL RETURN i") }`);
        return [];
      } catch (err) {
        return (err as { problems?: Array<{ message: string }> }).problems!.map(
          (p) => p.message,
        );
      }
    };
    expect(problems("a: Int @length(max: 3)")).toContain(
      "argument a: @length applies to String and ID fields",
    );
    expect(problems('a: String = "toolong" @length(max: 3)')).toContain(
      "argument a: its default must be at most 3 characters",
    );
    expect(problems('a: String @pattern(regex: "x+")')[0]).toMatch(
      /^argument a: @pattern needs @length\(max:\)/,
    );
    expect(problems("a: String @length(max: 3)")).toEqual([]);
  });
});
