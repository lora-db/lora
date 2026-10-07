// How scalar values arrive: a BigInt is stored as an integer whatever its
// size, and a value a scalar refuses is BAD_USER_INPUT whether it came as
// a variable or as a literal in the document.

import { graphql, GraphQLError, GraphQLScalarType, Kind } from "graphql";
import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

const rows = async (
  db: { execute(query: string): Promise<unknown> },
  cypher: string,
) =>
  ((await db.execute(cypher)) as { rows: Array<Record<string, unknown>> }).rows;

describe("BigInt", () => {
  const typeDefs = /* GraphQL */ `
    type Counter @node @mutation {
      id: BigInt! @key
      total: BigInt @filterable(byValue: [EQ, GTE]) @sortable
    }
  `;

  // The Node binding types a JavaScript number as an INTEGER only within
  // 32 bits; between there and 2^53 a BigInt used to be stored as a FLOAT.
  test("is stored as an INTEGER at every size", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const values = [
      "3",
      "-2147483649",
      "4294967296",
      "5000000000",
      "9007199254740993",
    ];
    const created = await t.data<{ createCounters: { counters: unknown[] } }>(
      `mutation ($input: [CounterCreateInput!]!) { createCounters(input: $input) { counters { id total } } }`,
      { input: values.map((v, i) => ({ id: String(i + 1), total: v })) },
    );
    expect(created.createCounters.counters).toEqual(
      values.map((v, i) => ({ id: String(i + 1), total: v })),
    );
    expect(
      await rows(
        t.db,
        "MATCH (c:Counter) RETURN DISTINCT type.of(c.total) AS t",
      ),
    ).toEqual([{ t: "INTEGER" }]);
  });

  test("a key beyond 32 bits is found again, updated and filtered", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    await t.data(
      `mutation { createCounters(input: [{ id: "5000000000", total: "1" }]) { info { nodesCreated } } }`,
    );
    expect(
      await t.data(
        `mutation { updateCounter(id: "5000000000", adjust: { total: { add: "6000000000" } }) { counter { id total } } }`,
      ),
    ).toEqual({
      updateCounter: { counter: { id: "5000000000", total: "6000000001" } },
    });
    expect(
      await t.data(
        `{ counter(id: "5000000000") { total } counters(where: { total: { gte: "6000000000" } }) { id } }`,
      ),
    ).toEqual({
      counter: { total: "6000000001" },
      counters: [{ id: "5000000000" }],
    });
  });
});

describe("a value a scalar refuses", () => {
  const check = (value: unknown) => {
    if (typeof value !== "string" || !value.includes("@")) {
      throw new GraphQLError("Email must contain @");
    }
    return value.toLowerCase();
  };
  const Email = new GraphQLScalarType({
    name: "Email",
    serialize: (value) => value,
    parseValue: check,
    parseLiteral: (ast) => check(ast.kind === Kind.STRING ? ast.value : null),
  });
  const typeDefs = /* GraphQL */ `
    scalar Email @storedAs(type: STRING)
    type Person @node @mutation {
      key: String! @key
      email: Email
      born: Date
      visits: BigInt
    }
  `;
  const literal = (field: string) =>
    `mutation { createPersons(input: [{ key: "a", ${field} }]) { info { nodesCreated } } }`;

  test.each([
    ["a custom scalar", literal(`email: "nope"`), "Email must contain @"],
    ["a temporal", literal(`born: 5`), "Date must be an ISO-8601 string"],
    ["a BigInt", literal(`visits: 1.5`), "BigInt must be an integer"],
  ])("as a literal: %s", async (_, source, message) => {
    const t = await createTestLoraGraphQL({ typeDefs, scalars: { Email } });
    // Through execute(), and through plain graphql-js on getSchema().
    for (const result of [
      await t.lora.execute({ source }),
      await graphql({ schema: t.schema, source }),
    ]) {
      expect(result.errors?.[0]?.message).toContain(message);
      expect(result.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
    }
  });

  test("as a variable", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, scalars: { Email } });
    const result = await t.lora.execute({
      source: `mutation ($email: Email) { createPersons(input: [{ key: "a", email: $email }]) { info { nodesCreated } } }`,
      variables: { email: "nope" },
    });
    expect(result.errors?.[0]?.extensions?.["code"]).toBe("BAD_USER_INPUT");
  });

  test("a value it accepts is stored as the scalar returns it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, scalars: { Email } });
    await t.data(literal(`email: "Ada@Example.com"`));
    expect(await rows(t.db, "MATCH (p:Person) RETURN p.email AS e")).toEqual([
      { e: "ada@example.com" },
    ]);
  });
});
