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
