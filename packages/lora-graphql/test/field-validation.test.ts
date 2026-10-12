// Value rules on fields: @length, @pattern, @format, @range, @multipleOf
// and @size refuse a mutation whose input breaks them, before the
// transaction opens and with every violation at once; values computed
// during the write (an adjust, a @populatedBy callback) are checked inside
// the transaction and roll it back.

import type { ExecutionResult } from "graphql";
import { beforeEach, describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = /* GraphQL */ `
  type Person @node @mutation {
    key: String! @key @length(min: 2, max: 20) @pattern(regex: "[a-z0-9-]+")
    name: String! @length(min: 1, max: 5)
    email: String @format(as: "email")
    age: Int @range(min: 0, max: 150)
    score: Float @range(exclusiveMin: 0, exclusiveMax: 1)
    price: Float @multipleOf(value: 0.01)
    step: Int @multipleOf(value: 5)
    balance: BigInt @range(min: 0)
    tags: [String!] @size(max: 3, unique: true) @length(max: 4)
    nickname: String @alias(property: "nick") @length(max: 3)
    pin: String
      @selectable(onRead: false)
      @length(min: 4, max: 4, message: "must be four digits")
    slug: String @populatedBy(callback: "slug") @length(max: 6)
    friends: [Person!]!
      @relationship(type: "KNOWS", direction: OUT, properties: "Knows")
    team: Team @relationship(type: "IN_TEAM", direction: OUT)
  }

  type Team @node @mutation {
    key: String! @key
    name: String! @length(max: 4)
  }

  type Knows @relationshipProperties {
    since: Int @range(min: 1900)
    note: String @length(max: 3)
  }
`;

type T = Awaited<ReturnType<typeof createTestLoraGraphQL>>;
let t: T;
let slug: (input: Record<string, unknown>) => unknown;

beforeEach(async () => {
  slug = () => "ok";
  t = await createTestLoraGraphQL({
    typeDefs,
    callbacks: { slug: ({ input }) => slug(input) },
    seed: [
      "CREATE (:Person {key: 'ada', name: 'Ada', age: 36, tags: ['a', 'b']})",
      // A key no rule would accept today: stored before the rule existed.
      "CREATE (:Person {key: 'Old Key', name: 'Old'})",
      "CREATE (:Team {key: 't1', name: 'Core'})",
    ],
  });
  t.statements.length = 0;
});

interface Violation {
  path: Array<string | number>;
  coordinate: string;
  rule: string;
  params: Record<string, unknown>;
  message: string;
}

const failure = (result: ExecutionResult) => {
  const error = result.errors?.[0];
  return {
    code: error?.extensions?.["code"],
    message: error?.message ?? "",
    violations: (error?.extensions?.["violations"] ?? []) as Violation[],
  };
};

const count = async (cypher: string) =>
  (
    (await t.db.execute(cypher)) as unknown as {
      rows: Array<Record<string, unknown>>;
    }
  ).rows[0]!["c"];

const create = (input: string) =>
  t.lora.execute({
    source: `mutation { createPersons(input: [${input}]) { persons { key } } }`,
  });

describe("on create", () => {
  test("a valid input is written", async () => {
    const r = await create(
      `{ key: "grace", name: "Grace", email: "grace@example.com", age: 0, score: 0.5, price: 1.15, step: 10, balance: "5000000000", tags: ["x", "y"], pin: "1234" }`,
    );
    expect(r.errors).toBeUndefined();
    expect(
      await count("MATCH (p:Person {key: 'grace'}) RETURN p.age AS c"),
    ).toBe(0);
  });

  test("every violation comes back at once, and nothing runs", async () => {
    const r = await create(
      `{ key: "grace", name: "Much too long", email: "nope", age: 200, pin: "secret-value" }`,
    );
    const { code, message, violations } = failure(r);
    expect(code).toBe("BAD_USER_INPUT");
    expect(violations).toEqual([
      {
        path: ["input", 0, "name"],
        coordinate: "Person.name",
        rule: "maxLength",
        params: { limit: 5 },
        message: "must be at most 5 characters",
      },
      {
        path: ["input", 0, "email"],
        coordinate: "Person.email",
        rule: "format",
        params: { format: "email" },
        message: "must be an email address",
      },
      {
        path: ["input", 0, "age"],
        coordinate: "Person.age",
        rule: "maximum",
        params: { limit: 150, comparison: "<=" },
        message: "must be at most 150",
      },
      {
        path: ["input", 0, "pin"],
        coordinate: "Person.pin",
        rule: "maxLength",
        params: { limit: 4 },
        message: "must be four digits",
      },
    ]);
    expect(message).toContain("4 values are not valid");
    expect(message).toContain("input[0].name must be at most 5 characters");
    // Checked before the transaction: no statement, no lock.
    expect(t.statements).toHaveLength(0);
    // The value itself is never repeated: it may be a secret.
    expect(JSON.stringify(r)).not.toContain("secret-value");
  });

  test("each input of a batch has its own path", async () => {
    const r = await t.lora.execute({
      source: `mutation { createPersons(input: [{ key: "ok", name: "Ok" }, { key: "x2", name: "" }]) { info { nodesCreated } } }`,
    });
    expect(failure(r).violations.map((v) => v.path)).toEqual([
      ["input", 1, "name"],
    ]);
    expect(
      await count("MATCH (p:Person {key: 'ok'}) RETURN count(p) AS c"),
    ).toBe(0);
  });

  test("zero and the empty string are values, null and absent are not", async () => {
    expect(
      failure(await create(`{ key: "z1", name: "" }`)).violations[0],
    ).toMatchObject({
      rule: "minLength",
    });
    expect(
      failure(await create(`{ key: "z2", name: "Z", score: 0 }`)).violations[0],
    ).toMatchObject({
      rule: "exclusiveMinimum",
      params: { limit: 0, comparison: ">" },
    });
    expect(
      (await create(`{ key: "z3", name: "Z", age: 0 }`)).errors,
    ).toBeUndefined();
    expect(
      (await create(`{ key: "z4", name: "Z", email: null, age: null }`)).errors,
    ).toBeUndefined();
  });

  test("length counts code points, not UTF-16 units", async () => {
    // Five emoji: five characters to a person and to LoraDB, ten units to JS.
    expect(
      (await create(`{ key: "e1", name: "😀😀😀😀😀" }`)).errors,
    ).toBeUndefined();
    expect(
      failure(await create(`{ key: "e2", name: "😀😀😀😀😀😀" }`))
        .violations[0],
    ).toMatchObject({
      rule: "maxLength",
    });
  });

  test("a pattern must match the whole value", async () => {
    for (const key of ["ab cd", "xabc!", "ABC"]) {
      const { violations } = failure(
        await create(`{ key: "${key}", name: "K" }`),
      );
      expect(
        violations.map((v) => v.rule),
        key,
      ).toEqual(["pattern"]);
    }
    expect(
      (await create(`{ key: "abc-1", name: "K" }`)).errors,
    ).toBeUndefined();
  });

  test("a value over its length is not matched against the pattern", async () => {
    const { violations } = failure(
      await create(`{ key: "${"a".repeat(30)}!", name: "K" }`),
    );
    expect(violations.map((v) => v.rule)).toEqual(["maxLength"]);
  });

  test("numbers: bounds, multiples and BigInt", async () => {
    const rules = async (fields: string) =>
      failure(
        await create(`{ key: "n1", name: "N", ${fields} }`),
      ).violations.map((v) => v.rule);
    expect(await rules("age: -1")).toEqual(["minimum"]);
    expect(await rules("score: 1")).toEqual(["exclusiveMaximum"]);
    expect(await rules("step: 7")).toEqual(["multipleOf"]);
    // 0.3 % 0.1 is not 0 in floating point; the rule still holds.
    expect(await rules("price: 0.3")).toEqual([]);
    expect(await rules("price: 1.155")).toEqual(["multipleOf"]);
    expect(await rules(`balance: "-1"`)).toEqual(["minimum"]);
  });

  test("a list: its size, its uniqueness, and each item", async () => {
    const violations = async (tags: string) =>
      failure(await create(`{ key: "l1", name: "L", tags: ${tags} }`))
        .violations;
    expect(
      (await violations(`["a", "b", "c", "d"]`)).map((v) => v.rule),
    ).toEqual(["maxItems"]);
    expect((await violations(`["a", "a"]`)).map((v) => v.rule)).toEqual([
      "uniqueItems",
    ]);
    expect(await violations(`["ok", "toolong"]`)).toMatchObject([
      { path: ["input", 0, "tags", 1], rule: "maxLength" },
    ]);
  });

  test("an aliased field is named as clients know it", async () => {
    const [v] = failure(
      await create(`{ key: "al", name: "A", nickname: "long" }`),
    ).violations;
    expect(v).toMatchObject({
      path: ["input", 0, "nickname"],
      coordinate: "Person.nickname",
    });
  });

  test("every root field of an operation is checked", async () => {
    const r = await t.lora.execute({
      source: `mutation {
        a: createPersons(input: [{ key: "a1", name: "A" }]) { __typename }
        b: createPersons(input: [{ key: "b1", name: "Too long" }]) { __typename }
      }`,
    });
    expect(r.errors?.map((e) => e.path)).toEqual([["b"]]);
  });

  test("variables are checked like literals", async () => {
    const r = await t.lora.execute({
      source: `mutation ($input: [PersonCreateInput!]!) { createPersons(input: $input) { info { nodesCreated } } }`,
      variables: { input: [{ key: "v1", name: "Too long" }] },
    });
    expect(failure(r).violations).toHaveLength(1);
  });

  test("a large batch reports the first hundred and the total", async () => {
    const input = Array.from({ length: 150 }, (_, i) => ({
      key: `k${i}`,
      name: "Too long",
    }));
    const r = await t.lora.execute({
      source: `mutation ($input: [PersonCreateInput!]!) { createPersons(input: $input) { info { nodesCreated } } }`,
      variables: { input },
    });
    expect(failure(r).violations).toHaveLength(100);
    expect(r.errors?.[0]?.extensions?.["violationCount"]).toBe(150);
    expect(failure(r).message).toContain("150 values are not valid");
  });
});

describe("nested writes", () => {
  test("a nested create, and the relationship's properties", async () => {
    const r = await create(
      `{ key: "n1", name: "N",
         team: { create: { node: { key: "t2", name: "Too long" } } },
         friends: { connect: [{ key: "ada", edge: { since: 1800, note: "long" } }] } }`,
    );
    expect(failure(r).violations.map((v) => [v.coordinate, v.path])).toEqual([
      ["Knows.since", ["input", 0, "friends", "connect", 0, "edge", "since"]],
      ["Knows.note", ["input", 0, "friends", "connect", 0, "edge", "note"]],
      ["Team.name", ["input", 0, "team", "create", "node", "name"]],
    ]);
    expect(t.statements).toHaveLength(0);
  });

  test("an edge update and a nested node update", async () => {
    const r = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", update: {
        friends: { update: [{ key: "ada", edge: { since: 1 }, node: { name: "Too long" } }] }
      }) { person { key } } }`,
    });
    expect(failure(r).violations.map((v) => v.path)).toEqual([
      ["update", "friends", "update", 0, "edge", "since"],
      ["update", "friends", "update", 0, "node", "name"],
    ]);
  });
});

describe("on update", () => {
  test("only the fields an update sets are checked", async () => {
    const ok = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", update: { age: 37 }) { person { age } } }`,
    });
    expect(ok.data).toEqual({ updatePerson: { person: { age: 37 } } });
    const bad = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", update: { age: 151 }) { person { age } } }`,
    });
    expect(failure(bad)).toMatchObject({
      code: "BAD_USER_INPUT",
      violations: [{ path: ["update", "age"], rule: "maximum" }],
    });
  });

  test("a bulk update is refused before it selects anything", async () => {
    const r = await t.lora.execute({
      source: `mutation { updatePersons(where: { key: { eq: "ada" } }, update: { name: "Too long" }) { info { nodesUpdated } } }`,
    });
    expect(failure(r).code).toBe("BAD_USER_INPUT");
    expect(t.statements).toHaveLength(0);
  });

  test("an upsert checks its input whichever way a key goes", async () => {
    const r = await t.lora.execute({
      source: `mutation { upsertPersons(input: [{ key: "ada", name: "Too long" }, { key: "new", name: "Too long" }]) { info { nodesCreated } } }`,
    });
    expect(failure(r).violations.map((v) => v.path)).toEqual([
      ["input", 0, "name"],
      ["input", 1, "name"],
    ]);
  });

  test("a key is checked when stored, never when it addresses a node", async () => {
    // 'Old Key' breaks today's @pattern, and must stay reachable.
    const read = await t.lora.execute({
      source: `{ person(key: "Old Key") { name } }`,
    });
    expect(read.data).toEqual({ person: { name: "Old" } });
    const updated = await t.lora.execute({
      source: `mutation { updatePerson(key: "Old Key", update: { name: "New" }) { person { name } } }`,
    });
    expect(updated.data).toEqual({ updatePerson: { person: { name: "New" } } });
    const connected = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", update: { friends: { connect: [{ key: "Old Key" }] } }) { info { relationshipsCreated } } }`,
    });
    expect(connected.data).toEqual({
      updatePerson: { info: { relationshipsCreated: 1 } },
    });
  });
});

describe("values computed during the write", () => {
  test("an adjust that leaves the range is rolled back", async () => {
    const r = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", adjust: { age: { add: 500 } }) { person { age } } }`,
    });
    expect(failure(r)).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      violations: [
        {
          path: ["adjust", "age"],
          coordinate: "Person.age",
          rule: "maximum",
          message: "must be at most 150 after the adjustment",
        },
      ],
    });
    expect(await count("MATCH (p:Person {key: 'ada'}) RETURN p.age AS c")).toBe(
      36,
    );
  });

  test("an adjust inside the range is kept", async () => {
    const r = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", adjust: { age: { add: 4 } }) { person { age } } }`,
    });
    expect(r.data).toEqual({ updatePerson: { person: { age: 40 } } });
  });

  test("a push past the list's size is rolled back", async () => {
    const r = await t.lora.execute({
      source: `mutation { updatePerson(key: "ada", adjust: { tags: { push: ["c", "d"] } }) { person { tags } } }`,
    });
    expect(failure(r)).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      violations: [{ path: ["adjust", "tags"], rule: "maxItems" }],
    });
    expect(
      await count("MATCH (p:Person {key: 'ada'}) RETURN size(p.tags) AS c"),
    ).toBe(2);
  });

  test("a callback's result answers to the field's rules", async () => {
    slug = () => "far-too-long";
    const r = await create(`{ key: "cb", name: "C" }`);
    expect(failure(r)).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      violations: [{ coordinate: "Person.slug", rule: "maxLength" }],
    });
    expect(failure(r).message).toContain("@populatedBy callback slug");
    expect(
      await count("MATCH (p:Person {key: 'cb'}) RETURN count(p) AS c"),
    ).toBe(0);
  });
});

describe("with access control", () => {
  const guarded = /* GraphQL */ `
    type Claims @jwt {
      sub: String
    }
    type Note @node @mutation @authentication {
      key: String! @key
      text: String! @length(max: 3)
    }
  `;
  const source = `mutation { createNotes(input: [{ key: "n", text: "too long" }]) { info { nodesCreated } } }`;

  test("a caller who must sign in is told that, not the rules", async () => {
    const g = await createTestLoraGraphQL({ typeDefs: guarded });
    const anonymous = await g.lora.execute({ source });
    expect(failure(anonymous).code).toBe("UNAUTHENTICATED");
    const signedIn = await g.lora.execute({
      source,
      context: { jwt: { sub: "ada" } },
    });
    expect(failure(signedIn).code).toBe("BAD_USER_INPUT");
  });
});

describe("interface and union targets", () => {
  test("a member's part is checked with the member in its path", async () => {
    const m = await createTestLoraGraphQL({
      typeDefs: /* GraphQL */ `
        interface Event {
          key: String!
        }
        type Concert implements Event @node @mutation {
          key: String! @key
          band: String! @length(max: 3)
        }
        type Talk implements Event @node @mutation {
          key: String! @key
        }
        type Festival @node @mutation {
          key: String! @key
          events: [Event!]! @relationship(type: "HAS", direction: OUT)
        }
      `,
    });
    const r = await m.lora.execute({
      source: `mutation { createFestivals(input: [{ key: "f", events: { create: { Concert: [{ node: { key: "c", band: "Too long" } }] } } }]) { info { nodesCreated } } }`,
    });
    expect(failure(r).violations).toMatchObject([
      {
        path: ["input", 0, "events", "create", "Concert", 0, "node", "band"],
        coordinate: "Concert.band",
      },
    ]);
  });
});

describe("formats", () => {
  const cases: Record<string, { ok: string[]; bad: string[] }> = {
    email: {
      ok: ["ada@example.com", "a.b+c@sub.example.co"],
      bad: ["nope", "a@", "@b.c", "a b@c.d", "a@b..c"],
    },
    "email-strict": { ok: ["ada@example.com"], bad: ["ada@localhost"] },
    url: {
      ok: ["https://example.com/a?b=1", "http://localhost:3000"],
      bad: ["javascript:alert(1)", "ftp://example.com", "example.com", "/path"],
    },
    uuid: {
      ok: ["123e4567-e89b-12d3-a456-426614174000"],
      bad: ["123e4567e89b12d3a456426614174000", "123e4567-e89b-12d3-a456"],
    },
    hostname: { ok: ["example.com", "a-b.c"], bad: ["-a.com", "a..b", "a_b"] },
    ipv4: { ok: ["192.168.0.1"], bad: ["256.1.1.1", "1.2.3", "01.2.3.4"] },
    ipv6: {
      ok: [
        "::1",
        "2001:db8::8a2e:370:7334",
        "::ffff:1.2.3.4",
        "1:2:3:4:5:6:7:8",
      ],
      bad: ["1::2::3", "1:2:3:4:5:6:7", "gggg::1", "1:2:3:4:5:6:7:8:9"],
    },
    slug: { ok: ["a-b-1"], bad: ["A-b", "a--b", "-a", "a-"] },
    "hex-color": {
      ok: ["#fff", "#1b6dff", "#1b6dff80"],
      bad: ["fff", "#ff", "#ggg"],
    },
    ulid: {
      ok: ["01ARZ3NDEKTSV4RRFFQ69G5FAV"],
      bad: ["81ARZ3NDEKTSV4RRFFQ69G5FAV", "01ARZ3"],
    },
    e164: {
      ok: ["+31201234567"],
      bad: ["0201234567", "+0123456789", "+31 20 123"],
    },
  };

  for (const [format, { ok, bad }] of Object.entries(cases)) {
    test(format, async () => {
      const f = await createTestLoraGraphQL({
        typeDefs: `type V @node @mutation { key: String! @key  value: String @format(as: "${format}") }`,
      });
      const accepted = async (value: string) =>
        (
          await f.lora.execute({
            source: `mutation ($v: String) { createVs(input: [{ key: "k", value: $v }]) { info { nodesCreated } } }`,
            variables: { v: value },
          })
        ).errors === undefined;
      for (const value of ok) {
        expect(await accepted(value), `${format} should accept ${value}`).toBe(
          true,
        );
        await f.db.execute("MATCH (v:V) DELETE v");
      }
      for (const value of bad) {
        expect(await accepted(value), `${format} should refuse ${value}`).toBe(
          false,
        );
      }
    });
  }
});

describe("at startup", () => {
  const problems = (fields: string): string[] => {
    try {
      buildModel(`type T @node @mutation { key: String! @key ${fields} }`);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test.each([
    ["n: Int @length(max: 3)", "@length applies to String and ID fields"],
    ["s: String @length", "@length needs min, max or both"],
    ["s: String @length(min: 5, max: 2)", "@length(min:) is greater than max"],
    ["s: String @length(max: -1)", "@length bounds cannot be negative"],
    [
      "s: String @range(min: 0)",
      "@range applies to Int, Float and BigInt fields",
    ],
    [
      "n: Int @range",
      "@range needs a bound: min, max, exclusiveMin or exclusiveMax",
    ],
    [
      "n: Int @range(min: 0, exclusiveMin: 0)",
      "@range takes min or exclusiveMin, not both",
    ],
    [
      "n: Int @range(min: 5, max: 1)",
      "@range admits no value: its lower bound is above its upper bound",
    ],
    [
      "s: String @multipleOf(value: 2)",
      "@multipleOf applies to Int and Float fields",
    ],
    [
      "n: Int @multipleOf(value: 0)",
      "@multipleOf(value:) must be greater than 0",
    ],
    [
      "n: Int @multipleOf(value: 0.5)",
      "@multipleOf(value:) on an Int field must be an integer",
    ],
    ["s: String @size(max: 3)", "@size applies to list fields"],
    ["l: [String!] @size", "@size needs min, max or unique"],
    ['n: Int @format(as: "email")', "@format applies to String and ID fields"],
    [
      "d: DateTime @timestamp @range(min: 0)",
      "@range: a @timestamp is set by the database, not validated",
    ],
  ])("%s", (fields, message) => {
    expect(problems(fields)).toContain(message);
  });

  test("an unknown format names the ones there are", () => {
    expect(problems('s: String @format(as: "phone")')[0]).toMatch(
      /^@format\(as: "phone"\) is not a known format \(email, /,
    );
  });

  test("a pattern needs a length bound, and must compile", () => {
    expect(problems('s: String @pattern(regex: "[a-z]+")')[0]).toMatch(
      /^@pattern needs @length\(max:\) on the same field/,
    );
    expect(
      problems('s: String @length(max: 10) @pattern(regex: "(")')[0],
    ).toMatch(/^@pattern\(regex:\) is not a valid expression/);
  });

  test("a pattern that backtracks catastrophically is refused", () => {
    for (const regex of ["(a+)+", "(\\\\w+\\\\s?)*", "(x+x+)+y"]) {
      expect(
        problems(`s: String @length(max: 200) @pattern(regex: "${regex}")`)[0],
        regex,
      ).toMatch(/^@pattern\(regex:\) takes too long on \d+ characters/);
    }
    // Ordinary patterns pass, repetition and all.
    expect(
      problems(
        's: String @length(max: 200) @pattern(regex: "[a-z0-9]+(?:-[a-z0-9]+)*")',
      ),
    ).toEqual([]);
  });

  test("a default the field's own rule refuses", () => {
    expect(
      problems('s: String @length(max: 3) @default(value: "four")'),
    ).toContain("@default(value:) must be at most 3 characters");
  });

  test("the rules are not offered where no value is stored", () => {
    const interfaceField = () =>
      buildModel(`interface I { key: String!  name: String @length(max: 3) }
        type T implements I @node { key: String! @key  name: String }`);
    expect(interfaceField).toThrow(
      /@length is not supported on a interface field|@length is not supported on an interface field/,
    );
  });
});

describe("@normalize", () => {
  const sdl = /* GraphQL */ `
    type Account @node @mutation {
      key: String! @key
      email: String!
        @normalize(trim: true, case: LOWER, unicode: NFC)
        @format(as: "email")
        @unique
      nickname: String @normalize(trim: true, emptyToNull: true) @length(min: 2)
      tags: [String!] @normalize(trim: true, case: UPPER) @length(max: 3)
      handle: String @populatedBy(callback: "handle") @normalize(case: LOWER)
      friends: [Account!]!
        @relationship(type: "KNOWS", direction: OUT, properties: "Knows")
    }
    type Knows @relationshipProperties {
      note: String @normalize(trim: true) @length(max: 3)
    }
  `;
  const make = () =>
    createTestLoraGraphQL({
      typeDefs: sdl,
      callbacks: { handle: () => "MiXeD" },
    });
  const stored = async (a: T, key: string) =>
    (
      (await a.db.execute(
        `MATCH (a:Account {key: '${key}'}) RETURN a.email AS email, a.nickname AS nickname, a.tags AS tags, a.handle AS handle`,
      )) as unknown as { rows: Array<Record<string, unknown>> }
    ).rows[0];

  test("the value is normalised, then validated, then stored", async () => {
    const a = await make();
    const r = await a.lora.execute({
      source: `mutation ($input: [AccountCreateInput!]!) { createAccounts(input: $input) { accounts { email tags } } }`,
      variables: {
        input: [
          {
            key: "a",
            // Trimmed and lower-cased before @format sees it; the
            // combining accent composed by NFC.
            email: "  Ada@Example.COM ",
            nickname: "   ",
            tags: [" ab ", "cd"],
          },
        ],
      },
    });
    expect(r.errors).toBeUndefined();
    expect(await stored(a, "a")).toEqual({
      email: "ada@example.com",
      nickname: null,
      tags: ["AB", "CD"],
      handle: "mixed",
    });
  });

  test("two spellings of one value meet the uniqueness constraint", async () => {
    const a = await make();
    const create = (key: string, email: string) =>
      a.lora.execute({
        source: `mutation ($key: String!, $email: String!) { createAccounts(input: [{ key: $key, email: $email }]) { info { nodesCreated } } }`,
        variables: { key, email },
      });
    expect((await create("a", "ada@example.com")).errors).toBeUndefined();
    expect(failure(await create("b", " ADA@example.com")).code).toBe(
      "CONSTRAINT_VIOLATION",
    );
  });

  test("the rules judge the normalised value", async () => {
    const a = await make();
    // Three characters once trimmed: within @length(max: 3).
    const ok = await a.lora.execute({
      source: `mutation { createAccounts(input: [{ key: "a", email: "a@b.co", tags: ["  abc  "] }]) { info { nodesCreated } } }`,
    });
    expect(ok.errors).toBeUndefined();
    // One character once trimmed: under @length(min: 2).
    const bad = await a.lora.execute({
      source: `mutation { createAccounts(input: [{ key: "b", email: "b@b.co", nickname: " x " }]) { info { nodesCreated } } }`,
    });
    expect(failure(bad).violations).toMatchObject([
      { path: ["input", 0, "nickname"], rule: "minLength" },
    ]);
  });

  test("on update, and on a relationship's properties", async () => {
    const a = await make();
    await a.lora.execute({
      source: `mutation { createAccounts(input: [{ key: "a", email: "a@b.co" }, { key: "b", email: "b@b.co" }]) { info { nodesCreated } } }`,
    });
    const r = await a.lora.execute({
      source: `mutation { updateAccount(key: "a", update: { email: " NEW@b.co ", friends: { connect: [{ key: "b", edge: { note: " hi " } }] } }) {
        account { email friendsConnection { edges { properties { note } } } } } }`,
    });
    expect(r.data).toEqual({
      updateAccount: {
        account: {
          email: "new@b.co",
          friendsConnection: { edges: [{ properties: { note: "hi" } }] },
        },
      },
    });
  });

  test("startup: where it applies, and a default in stored form", () => {
    const problems = (fields: string): string[] => {
      try {
        buildModel(`type T @node @mutation { key: String! @key ${fields} }`);
        return [];
      } catch (err) {
        if (err instanceof ModelError)
          return err.problems.map((p) => p.message);
        throw err;
      }
    };
    expect(problems("n: Int @normalize(trim: true)")).toContain(
      "@normalize applies to String and ID fields",
    );
    expect(problems("s: String @normalize")).toContain(
      "@normalize needs trim, case, unicode or emptyToNull",
    );
    expect(problems("l: [String!] @normalize(emptyToNull: true)")).toContain(
      "@normalize(emptyToNull:) applies to a single value, not to list items",
    );
    expect(
      problems('s: String @normalize(case: LOWER) @default(value: "Draft")'),
    ).toContain("@default(value:) is not in the form @normalize stores");
  });
});

describe("rules on a custom scalar", () => {
  const sdl = /* GraphQL */ `
    scalar Email
      @storedAs(type: STRING)
      @normalize(trim: true, case: LOWER)
      @format(as: "email")
      @length(max: 254)
    scalar Percent @storedAs(type: INT) @range(min: 0, max: 100)

    type Person @node @mutation {
      key: String! @key
      email: Email
      backup: Email @length(max: 12)
      progress: Percent
      knows: [Person!]!
        @relationship(type: "KNOWS", direction: OUT, properties: "Knows")
    }
    type Knows @relationshipProperties {
      trust: Percent
    }
  `;

  test("every field of the type has them, and a field may tighten one", async () => {
    const s = await createTestLoraGraphQL({ typeDefs: sdl });
    const r = await s.lora.execute({
      source: `mutation ($input: [PersonCreateInput!]!) { createPersons(input: $input) { info { nodesCreated } } }`,
      variables: {
        input: [
          {
            key: "a",
            email: "nope",
            backup: "long.address@example.com",
            progress: 101,
            knows: { connect: [{ key: "a", edge: { trust: -1 } }] },
          },
        ],
      },
    });
    expect(
      failure(r).violations.map((v) => [v.coordinate, v.rule, v.params]),
    ).toEqual([
      ["Person.email", "format", { format: "email" }],
      // The field's own @length replaces the scalar's 254.
      ["Person.backup", "maxLength", { limit: 12 }],
      ["Person.progress", "maximum", { limit: 100, comparison: "<=" }],
      ["Knows.trust", "minimum", { limit: 0, comparison: ">=" }],
    ]);
  });

  test("the scalar's @normalize applies before its rules", async () => {
    const s = await createTestLoraGraphQL({ typeDefs: sdl });
    const r = await s.lora.execute({
      source: `mutation ($e: Email) { createPersons(input: [{ key: "a", email: $e }]) { persons { email } } }`,
      variables: { e: " Ada@Example.com " },
    });
    expect(r.data).toEqual({
      createPersons: { persons: [{ email: "ada@example.com" }] },
    });
  });

  test("a rule the storage type cannot take is a model error", () => {
    expect(() =>
      buildModel(`scalar Code @storedAs(type: INT) @length(max: 3)
        type T @node { key: String! @key  code: Code }`),
    ).toThrow(/@length applies to String and ID fields/);
  });
});

describe("the validators option", () => {
  // The smallest Standard Schema: what zod, valibot, arktype and yup
  // schemas expose under "~standard".
  type Outcome =
    | { value: unknown }
    | {
        issues: Array<{
          message: string;
          path?: Array<string | number | { key: string }>;
        }>;
      };
  const schema = (
    validate: (value: Record<string, unknown>) => Outcome | Promise<Outcome>,
  ) => ({
    "~standard": {
      version: 1 as const,
      vendor: "test",
      validate: (value: unknown) => validate(value as Record<string, unknown>),
    },
  });
  const sdl = /* GraphQL */ `
    type Booking @node @mutation {
      key: String! @key
      startsOn: Date
      endsOn: Date
      note: String @length(max: 5)
      guests: [Guest!]! @relationship(type: "FOR", direction: IN)
    }
    type Guest @node @mutation {
      key: String! @key
      age: Int
    }
  `;
  const ordered = schema((b) => {
    // Dates arrive as the ISO strings the client sent.
    const start = b["startsOn"] as string | undefined;
    const end = b["endsOn"] as string | undefined;
    return start && end && end < start
      ? {
          issues: [
            { message: "must not be before startsOn", path: ["endsOn"] },
          ],
        }
      : { value: b };
  });

  test("a rule across two fields, reported like any other", async () => {
    const v = await createTestLoraGraphQL({
      typeDefs: sdl,
      validators: { Booking: ordered },
    });
    const r = await v.lora.execute({
      source: `mutation { createBookings(input: [
        { key: "ok", startsOn: "2027-01-01", endsOn: "2027-01-05" },
        { key: "bad", startsOn: "2027-01-05", endsOn: "2027-01-01" }
      ]) { info { nodesCreated } } }`,
    });
    expect(failure(r)).toMatchObject({
      code: "BAD_USER_INPUT",
      violations: [
        {
          path: ["input", 1, "endsOn"],
          coordinate: "Booking.endsOn",
          rule: "custom",
          params: {},
          message: "must not be before startsOn",
        },
      ],
    });
    expect(v.statements).toHaveLength(0);
  });

  test("dates are shown as strings, and stored as dates", async () => {
    const v = await createTestLoraGraphQL({
      typeDefs: sdl,
      validators: { Booking: ordered },
    });
    const r = await v.lora.execute({
      source: `mutation { createBookings(input: [{ key: "a", startsOn: "2027-01-01", endsOn: "2027-01-05" }]) { bookings { startsOn endsOn } } }`,
    });
    expect(r.data).toEqual({
      createBookings: {
        bookings: [{ startsOn: "2027-01-01", endsOn: "2027-01-05" }],
      },
    });
    expect(
      (
        (await v.db.execute(
          "MATCH (b:Booking) RETURN type.of(b.startsOn) AS t",
        )) as unknown as { rows: unknown[] }
      ).rows,
    ).toEqual([{ t: "DATE" }]);
  });

  test("it runs once the field rules hold, and may be asynchronous", async () => {
    let calls = 0;
    const v = await createTestLoraGraphQL({
      typeDefs: sdl,
      validators: {
        Booking: schema(async (b) => {
          calls++;
          await Promise.resolve();
          return { value: b };
        }),
      },
    });
    const bad = await v.lora.execute({
      source: `mutation { createBookings(input: [{ key: "a", note: "too long" }]) { info { nodesCreated } } }`,
    });
    expect(failure(bad).violations.map((x) => x.rule)).toEqual(["maxLength"]);
    expect(calls).toBe(0);
    const ok = await v.lora.execute({
      source: `mutation { createBookings(input: [{ key: "a", note: "fine" }]) { info { nodesCreated } } }`,
    });
    expect(ok.errors).toBeUndefined();
    expect(calls).toBe(1);
  });

  test("what it returns is what is written", async () => {
    const v = await createTestLoraGraphQL({
      typeDefs: sdl,
      validators: {
        Booking: schema((b) => ({
          value: { ...b, note: String(b["note"]).toUpperCase() },
        })),
      },
    });
    const r = await v.lora.execute({
      source: `mutation { createBookings(input: [{ key: "a", note: "hi" }]) { bookings { note } } }`,
    });
    expect(r.data).toEqual({ createBookings: { bookings: [{ note: "HI" }] } });
  });

  test("a schema per kind of input, and nested inputs", async () => {
    const adult = schema((g) =>
      typeof g["age"] === "number" && g["age"] < 18
        ? { issues: [{ message: "must be an adult", path: [{ key: "age" }] }] }
        : { value: g },
    );
    const v = await createTestLoraGraphQL({
      typeDefs: sdl,
      validators: { Guest: { create: adult } },
    });
    const nested = await v.lora.execute({
      source: `mutation { createBookings(input: [{ key: "b", guests: { create: [{ node: { key: "g", age: 12 } }] } }]) { info { nodesCreated } } }`,
    });
    expect(failure(nested).violations).toMatchObject([
      {
        path: ["input", 0, "guests", "create", 0, "node", "age"],
        coordinate: "Guest.age",
        message: "must be an adult",
      },
    ]);
    // An update has no validator here: only `create` was given.
    await v.db.execute("CREATE (:Guest {key: 'g1', age: 30})");
    const update = await v.lora.execute({
      source: `mutation { updateGuest(key: "g1", update: { age: 12 }) { guest { age } } }`,
    });
    expect(update.data).toEqual({ updateGuest: { guest: { age: 12 } } });
  });

  test("an unknown type or a non-schema is refused at construction", async () => {
    await expect(
      createTestLoraGraphQL({ typeDefs: sdl, validators: { Nope: ordered } }),
    ).rejects.toThrow(/validators.Nope: there is no @node type Nope/);
    await expect(
      createTestLoraGraphQL({
        typeDefs: sdl,
        validators: { Booking: { parse: () => 1 } as never },
      }),
    ).rejects.toThrow(/validators.Booking: not a Standard Schema validator/);
  });
});

// G-50: one input connects, creates, disconnects or updates at most so
// many relationships through a field, refused before any write.
describe("@size(max:) on a relationship field", () => {
  const sdl = /* GraphQL */ `
    type Claims @jwt {
      sub: String! @viewer(type: "P", field: "key")
    }
    interface Thing {
      key: String!
    }
    type Book implements Thing @node @mutation {
      key: String! @key
    }
    type Film implements Thing @node @mutation {
      key: String! @key
    }
    type P @node @mutation {
      key: String! @key
      follows: [P!]!
        @relationship(
          type: "F"
          direction: OUT
          properties: "Since"
          nestedOperations: [CONNECT, CREATE, DISCONNECT, UPDATE_EDGE]
        )
        @size(max: 2)
      likes: [Thing!]!
        @relationship(type: "LIKES", direction: OUT)
        @size(max: 1, message: "one at a time")
      best: P @relationship(type: "BEST", direction: OUT)
    }
    type Since @relationshipProperties {
      year: Int
    }
  `;
  const setup = () =>
    createTestLoraGraphQL({
      typeDefs: sdl,
      seed: [
        "UNWIND ['a','b','c','d'] AS k CREATE (:P {key: k})",
        "CREATE (:Book {key: 'b1'}), (:Book {key: 'b2'}), (:Film {key: 'f1'})",
      ],
    });
  const relationships = async (m: T) =>
    (
      (await m.db.execute(
        "MATCH ()-[r]->() RETURN count(r) AS c",
      )) as unknown as {
        rows: Array<Record<string, unknown>>;
      }
    ).rows[0]!["c"];
  const update = (m: T, input: string) =>
    m.lora.execute({
      source: `mutation { updateP(key: "a", update: { ${input} }) { p { key } } }`,
      context: { jwt: { sub: "a" } },
    });

  test("a connect list over the bound is refused, having connected nothing", async () => {
    const m = await setup();
    m.statements.length = 0;
    const r = await update(
      m,
      `follows: { connect: [{ key: "b" }, { key: "c" }, { key: "d" }] }`,
    );
    expect(failure(r)).toMatchObject({
      code: "BAD_USER_INPUT",
      violations: [
        {
          path: ["update", "follows", "connect"],
          coordinate: "P.follows",
          rule: "maxItems",
          params: { limit: 2 },
          message: "must have at most 2 items",
        },
      ],
    });
    expect(m.statements).toHaveLength(0);
    expect(await relationships(m)).toBe(0);
  });

  test("a list at the bound passes; each list has its own", async () => {
    const m = await setup();
    const r = await update(
      m,
      `follows: {
        connect: [{ key: "b" }, { key: "c" }]
        create: [{ node: { key: "e" } }, { node: { key: "f" } }]
      }`,
    );
    expect(r.errors).toBeUndefined();
    expect(await relationships(m)).toBe(4);
  });

  test("disconnect and update lists, and a create input", async () => {
    const m = await setup();
    expect(
      failure(
        await update(
          m,
          `follows: {
            disconnect: ["b", "c", "d"]
            update: [{ key: "b", edge: { year: 1 } }, { key: "c", edge: { year: 1 } }, { key: "d", edge: { year: 1 } }]
          }`,
        ),
      ).violations.map((v) => v.path),
    ).toEqual([
      ["update", "follows", "disconnect"],
      ["update", "follows", "update"],
    ]);
    const created = await m.lora.execute({
      source: `mutation { createPs(input: [{ key: "z", follows: { connect: [{ key: "a" }, { key: "b" }, { key: "c" }] } }]) { ps { key } } }`,
    });
    expect(failure(created).violations.map((v) => v.path)).toEqual([
      ["input", 0, "follows", "connect"],
    ]);
  });

  test("an interface target is bounded per member, with the field's message", async () => {
    const m = await setup();
    expect(
      (
        await update(
          m,
          `likes: { connect: { Book: [{ key: "b1" }], Film: [{ key: "f1" }] } }`,
        )
      ).errors,
    ).toBeUndefined();
    expect(
      failure(
        await update(
          m,
          `likes: { connect: { Book: [{ key: "b1" }, { key: "b2" }] } }`,
        ),
      ).violations,
    ).toMatchObject([
      {
        path: ["update", "likes", "connect", "Book"],
        coordinate: "P.likes",
        message: "one at a time",
      },
    ]);
  });

  test("the input says so", async () => {
    const m = await setup();
    const fields = m.schema.getType(
      "PFollowsUpdateRelationInput",
    ) as unknown as {
      getFields(): Record<string, { description?: string | null }>;
    };
    expect(fields.getFields()["connect"]!.description).toBe(
      "At most 2 per list.",
    );
    expect(fields.getFields()["disconnect"]!.description).toMatch(
      /At most 2 per list\.$/,
    );
  });

  test("at startup: max alone, on a list", () => {
    const problems = (field: string): string[] => {
      try {
        buildModel(`type P @node @mutation { key: String! @key  ${field} }`);
        return [];
      } catch (err) {
        return (err as ModelError).problems.map((p) => p.message);
      }
    };
    const rel = (rest: string, type = "[P!]!") =>
      `r: ${type} @relationship(type: "R", direction: OUT) ${rest}`;
    expect(problems(rel("@size(max: 5)"))).toEqual([]);
    expect(problems(rel("@size(max: 5)", "P"))).toContain(
      "@size only applies to list relationships",
    );
    expect(problems(rel("@size(min: 1, max: 5)"))).toContain(
      "@size on a relationship field takes max: it bounds each nested connect, create, disconnect and update list",
    );
    expect(problems(rel("@size(unique: true)"))).toContain(
      "@size on a relationship field needs max",
    );
    expect(problems(rel("@size(max: 0)"))).toContain(
      "@size(max:) must be at least 1",
    );
  });
});
