// Reads compiled for one request are reused by later ones only when the
// statement text would be the same: same field node (documents are
// cached), variables, claims and `$context` values.

import { createDatabase } from "@loradb/lora-node";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type StatementEvent } from "../src/index.js";

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
