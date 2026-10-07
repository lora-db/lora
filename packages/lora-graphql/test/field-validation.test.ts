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
