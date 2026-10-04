// Subscriptions reveal no more than the subscriber may read: deletions,
// previous values and relationship events are checked like any read, and
// a subscription's own cost is bounded.

import { parse, subscribe, type ExecutionResult } from "graphql";
import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL, type TestLoraGraphQL } from "../src/testing.js";

const settle = () => new Promise((r) => setTimeout(r, 30));

async function listen(
  t: TestLoraGraphQL,
  query: string,
  context: Record<string, unknown> = {},
) {
  const controller = new AbortController();
  const result = await subscribe({
    schema: t.schema,
    document: parse(query),
    contextValue: { ...context, signal: controller.signal },
  });
  if (!(Symbol.asyncIterator in result)) throw result.errors?.[0];
  const events: unknown[] = [];
  const done = (async () => {
    for await (const r of result as AsyncIterable<ExecutionResult>) {
      events.push(
        r.errors
          ? { data: r.data, errors: r.errors.map((e) => e.message) }
          : r.data,
      );
    }
  })();
  return {
    events,
    stop: async () => {
      controller.abort();
      await (result as AsyncGenerator).return?.(undefined);
      await done;
    },
  };
}

const claims = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}`;
const lou = { jwt: { sub: "lou" } };
const bo = { jwt: { sub: "bo" } };
const admin = { jwt: { sub: "admin", roles: ["admin"] } };

describe("deletions", () => {
  const typeDefs = `${claims}
  type Person @node { key: String! @key }
  type Secret @node @mutation
    @subscription(previousState: true)
    @authorization(filter: [{
      operations: [READ, SUBSCRIBE]
      where: { node: { owner: { isViewer: true } } }
    }]) {
    key: String! @key
    body: String!
    owner: Person! @relationship(type: "OWNS", direction: IN)
  }`;
  const seed =
    "CREATE (:Person {key: 'bo'})-[:OWNS]->(:Secret {key: 's1', body: 'hush'}), (:Person {key: 'lou'}), (:Person {key: 'admin'})";
  const follow = `subscription { secretChanged(key: "s1") { operation key previousState { body } } }`;

  test("reach a follower of the key only if they could read the node", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const asLou = await listen(t, follow, lou);
    const asBo = await listen(t, follow, bo);
    await t.data(
      `mutation { deleteSecret(key: "s1") { nodesDeleted } }`,
      {},
      admin,
    );
    await settle();
    await asLou.stop();
    await asBo.stop();
    expect(asLou.events).toEqual([]);
    expect(asBo.events).toEqual([
      {
        secretChanged: {
          operation: "DELETE",
          key: "s1",
          previousState: { body: "hush" },
        },
      },
    ]);
  });
  test("reach a follower the claims alone let read, without a check", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs.replace(
        "where: { node: { owner: { isViewer: true } } }",
        `where: { OR: [
          { node: { owner: { isViewer: true } } }
          { jwt: { roles: { includes: "admin" } } }
        ] }`,
      ),
      seed,
    });
    const asAdmin = await listen(t, follow, admin);
    const asLou = await listen(t, follow, lou);
    await t.data(
      `mutation { deleteSecret(key: "s1") { nodesDeleted } }`,
      {},
      bo,
    );
    await settle();
    await asAdmin.stop();
    await asLou.stop();
    expect(asAdmin.events).toHaveLength(1);
    expect(asLou.events).toEqual([]);
  });
});

describe("previousState", () => {
  const typeDefs = `${claims}
  type Person @node { key: String! @key }
  type Card @node @mutation @subscription(previousState: true) {
    key: String! @key
    title: String!
    pin: String
      @authorization(mask: [{ unless: { jwt: { roles: { includes: "admin" } } }, value: "****" }])
    hint: String
      @authorization(mask: [{ unless: { node: { owner: { isViewer: true } } }, value: "hidden" }])
    note: String @authentication(operations: [READ])
    owner: Person @relationship(type: "OWNS", direction: IN)
  }`;
  const seed =
    "CREATE (:Person {key: 'bo'})-[:OWNS]->(:Card {key: 'c1', title: 'A', pin: '1234', hint: 'red', note: 'n'})";
  const follow = `subscription { cardChanged(key: "c1") { operation previousState { title pin hint note } } }`;

  test("masks and field authentication apply to the values before the write", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const anonymous = await listen(t, follow);
    const asAdmin = await listen(t, follow, admin);
    await t.data(
      `mutation { updateCard(key: "c1", update: { title: "B" }) { info { nodesUpdated } } }`,
    );
    await settle();
    await anonymous.stop();
    await asAdmin.stop();
    expect(anonymous.events).toEqual([
      {
        data: {
          cardChanged: {
            operation: "UPDATE",
            // A mask that depends on the node reads as its value: the
            // node as it was cannot be tested after the write.
            previousState: {
              title: "A",
              pin: "****",
              hint: "hidden",
              note: null,
            },
          },
        },
        errors: ["Card.note needs an authenticated request"],
      },
    ]);
    expect(asAdmin.events).toEqual([
      {
        cardChanged: {
          operation: "UPDATE",
          previousState: { title: "A", pin: "1234", hint: "hidden", note: "n" },
        },
      },
    ]);
  });
});
