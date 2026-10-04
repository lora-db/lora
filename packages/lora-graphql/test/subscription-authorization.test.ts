// Subscriptions reveal no more than the subscriber may read: deletions,
// previous values and relationship events are checked like any read, and
// a subscription's own cost is bounded.

import { createDatabase } from "@loradb/lora-node";
import { execute, parse, subscribe, type ExecutionResult } from "graphql";
import { describe, expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type LoraDriver } from "../src/index.js";
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
    // Deleted by its owner: a node the deleter can't read is never deleted.
    const r = await t.data<{ deleteSecret: { nodesDeleted: number } }>(
      `mutation { deleteSecret(key: "s1") { nodesDeleted } }`,
      {},
      bo,
    );
    expect(r.deleteSecret.nodesDeleted).toBe(1);
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

describe("relationship events", () => {
  const typeDefs = `${claims}
  type Person @node { key: String! @key }
  type Board @node @mutation @subscription(relationships: true) {
    key: String! @key
    items: [Item!]! @relationship(type: "HAS", direction: OUT)
    notes: [Note!]! @relationship(type: "PINNED", direction: OUT)
      @authorization(validate: [{
        operations: [READ]
        where: { jwt: { roles: { includes: "admin" } } }
      }])
  }
  type Item @node @mutation
    @authorization(filter: [{
      operations: [READ]
      where: { node: { owner: { isViewer: true } } }
    }]) {
    key: String! @key
    owner: Person @relationship(type: "OWNS", direction: IN)
  }
  type Note @node @mutation { key: String! @key }`;
  const seed =
    "CREATE (:Board {key: 'b1'}), (:Person {key: 'bo'})-[:OWNS]->(:Item {key: 'i1'}), (:Note {key: 'n1'})";
  const follow = `subscription { boardChanged(key: "b1", operations: [CONNECT, DISCONNECT]) {
    operation relationship { field relatedType relatedKey }
  } }`;

  test("name a related node only to subscribers who may read it and the field", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const asLou = await listen(t, follow, lou);
    const asBo = await listen(t, follow, bo);
    const asAdmin = await listen(t, follow, admin);
    await t.data(
      `mutation { updateBoard(key: "b1", update: { items: { connect: [{ key: "i1" }] } }) { info { relationshipsCreated } } }`,
      {},
      bo,
    );
    await t.data(
      `mutation { updateBoard(key: "b1", update: { notes: { connect: [{ key: "n1" }] } }) { info { relationshipsCreated } } }`,
      {},
      admin,
    );
    await settle();
    // A deleted related node cannot be checked: the event is dropped.
    await t.data(`mutation { deleteItem(key: "i1") { nodesDeleted } }`, {}, bo);
    await settle();
    await asLou.stop();
    await asBo.stop();
    await asAdmin.stop();
    const item = {
      boardChanged: {
        operation: "CONNECT",
        relationship: {
          field: "Board.items",
          relatedType: "Item",
          relatedKey: "i1",
        },
      },
    };
    const note = {
      boardChanged: {
        operation: "CONNECT",
        relationship: {
          field: "Board.notes",
          relatedType: "Note",
          relatedKey: "n1",
        },
      },
    };
    expect(asLou.events).toEqual([]);
    expect(asBo.events).toEqual([item]);
    expect(asAdmin.events).toEqual([note]);
  });
});

describe("limits", () => {
  const typeDefs = `
  type Board @node @mutation @subscription {
    key: String! @key
    title: String @filterable
    items: [Item!]! @relationship(type: "HAS", direction: OUT) @filterable
  }
  type Item @node @mutation {
    key: String! @key
    label: String @filterable
    parts: [Item!]! @relationship(type: "PART", direction: OUT) @filterable
  }`;
  const code = (r: unknown) =>
    (r as ExecutionResult).errors?.[0]?.extensions?.["code"];

  test("a context holds at most maxSubscriptions subscriptions", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, maxSubscriptions: 2 });
    const context = {};
    const source = "subscription { boardChanged { key } }";
    const a = await t.lora.subscribe({ source, context });
    const b = await t.lora.subscribe({ source, context });
    expect(Symbol.asyncIterator in a && Symbol.asyncIterator in b).toBe(true);
    const c = await t.lora.subscribe({ source, context });
    expect(code(c)).toBe("LIMIT_EXCEEDED");
    // Another connection has its own allowance.
    const other = await t.lora.subscribe({ source, context: {} });
    expect(Symbol.asyncIterator in other).toBe(true);
    // Ending one frees its place.
    await (a as AsyncIterableIterator<ExecutionResult>).return!();
    const d = await t.lora.subscribe({ source, context });
    expect(Symbol.asyncIterator in d).toBe(true);
    for (const it of [b, other, d]) {
      await (it as AsyncIterableIterator<ExecutionResult>).return!();
    }
  });

  test("where is charged at subscribe time and nests one relationship deep", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, maxCost: 50 });
    const ok = await t.lora.subscribe({
      source: `subscription { boardChanged(where: { title: { eq: "x" } }) { key } }`,
    });
    expect(Symbol.asyncIterator in ok).toBe(true);
    await (ok as AsyncIterableIterator<ExecutionResult>).return!();
    // Every Board change would scan up to a page of items.
    const costly = await t.lora.subscribe({
      source: `subscription { boardChanged(where: { items: { some: { label: { eq: "x" } } } }) { key } }`,
    });
    expect(code(costly)).toBe("COST_EXCEEDED");
    const t2 = await createTestLoraGraphQL({ typeDefs });
    const nested = await t2.lora.subscribe({
      source: `subscription { boardChanged(where: { items: { some: { parts: { some: { label: { eq: "x" } } } } } }) { key } }`,
    });
    expect(code(nested)).toBe("LIMIT_EXCEEDED");
    const t3 = await createTestLoraGraphQL({
      typeDefs,
      maxSubscriptionFilterDepth: 2,
    });
    const allowed = await t3.lora.subscribe({
      source: `subscription { boardChanged(where: { items: { some: { parts: { some: { label: { eq: "x" } } } } } }) { key } }`,
    });
    expect(Symbol.asyncIterator in allowed).toBe(true);
    await (allowed as AsyncIterableIterator<ExecutionResult>).return!();
  });

  test("visibility checks run under subscriptionTimeoutMs", async () => {
    const db = await createDatabase();
    const base = loraDriver(db);
    const timeouts: Array<number | undefined> = [];
    const driver: LoraDriver = {
      ...base,
      run: (statements, options) => {
        if (statements[0]!.text.startsWith("UNWIND")) {
          timeouts.push(options.timeoutMs);
        }
        return base.run(statements, options);
      },
    };
    const lora = new LoraGraphQL({
      typeDefs,
      driver,
      subscriptionTimeoutMs: 250,
    });
    await lora.assertSchema({ create: true });
    const sub = await lora.subscribe({
      source: `subscription { boardChanged(where: { title: { eq: "x" } }) { key } }`,
    });
    const it = sub as AsyncIterableIterator<ExecutionResult>;
    const next = it.next();
    await new Promise((r) => setTimeout(r, 10));
    await lora.execute({
      source: `mutation { createBoards(input: [{ key: "b", title: "x" }]) { info { nodesCreated } } }`,
    });
    expect((await next).value).toEqual({
      data: { boardChanged: { key: "b" } },
    });
    await it.return!();
    expect(timeouts).toEqual([250]);
  });
});

describe("cost per request", () => {
  test("a reused context is charged per execution, not across them", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: `type Board @node { key: String! @key }`,
      seed: "UNWIND range(1, 5) AS i CREATE (:Board {key: toString(i)})",
      maxCost: 50,
    });
    const context = {};
    const source = "{ boards(limit: 20) { key } }";
    const costs: unknown[] = [];
    for (let i = 0; i < 6; i++) {
      const r = await t.lora.execute({ source, context });
      expect(r.errors).toBeUndefined();
      costs.push(r.extensions?.["cost"]);
    }
    expect(new Set(costs).size).toBe(1);
    // A server that caches the document and reuses the context.
    const document = parse(source);
    for (let i = 0; i < 6; i++) {
      const r = await execute({
        schema: t.schema,
        document,
        contextValue: context,
      });
      expect(r.errors).toBeUndefined();
    }
  });
});
