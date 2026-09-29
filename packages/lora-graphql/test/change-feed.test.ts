// Phase 18.3: subscriptions fed by the engine's committed change feed.

import { createDatabase } from "@loradb/lora-node";
import { parse, subscribe, type ExecutionResult } from "graphql";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Post
    @node
    @mutation
    @subscription(relationships: true)
    @authorization(
      filter: [
        {
          operations: [READ, SUBSCRIBE]
          where: { node: { published: { eq: true } } }
          requireAuthentication: false
        }
      ]
    ) {
    key: String! @key
    title: String!
    published: Boolean!
    tags: [Tag!]! @relationship(type: "TAGGED", direction: OUT)
  }
  type Tag @node {
    key: String! @key
  }
`;

async function setup() {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    changeFeed: true,
  });
  await lora.assertSchema({ create: true });
  const listen = async (query: string) => {
    const controller = new AbortController();
    const result = await subscribe({
      schema: lora.getSchema(),
      document: parse(query),
      contextValue: { signal: controller.signal },
    });
    if (!(Symbol.asyncIterator in result)) throw result.errors?.[0];
    const events: unknown[] = [];
    const iterator = (result as AsyncIterableIterator<ExecutionResult>)[
      Symbol.asyncIterator
    ]();
    // Start the subscription (and the feed) before any write.
    const first = iterator.next();
    const done = (async () => {
      let r = await first;
      while (!r.done) {
        events.push(
          r.value.errors
            ? { errors: r.value.errors.map((e) => e.message) }
            : r.value.data,
        );
        r = await iterator.next();
      }
    })();
    return {
      events,
      stop: async () => {
        controller.abort();
        await iterator.return?.(undefined);
        await done.catch(() => undefined);
      },
    };
  };
  return { db, lora, listen };
}

const settle = () => new Promise((r) => setTimeout(r, 100));

test("writes made outside the library reach subscribers, under their rules", async () => {
  const { db, lora, listen } = await setup();
  const sub = await listen(
    `subscription { postChanged { operation key node { title } } }`,
  );
  await settle();
  await db.execute("CREATE (:Post {key: 'p1', title: 'Raw', published: true})");
  await db.execute(
    "CREATE (:Post {key: 'p2', title: 'Hidden', published: false})",
  );
  await settle();
  await sub.stop();
  lora.close();
  expect(sub.events).toEqual([
    { postChanged: { operation: "CREATE", key: "p1", node: { title: "Raw" } } },
  ]);
});

test("relationships created in Cypher are CONNECT events with their field", async () => {
  const { db, lora, listen } = await setup();
  await db.execute(
    "CREATE (:Post {key: 'p1', title: 'T', published: true}), (:Tag {key: 't1'})",
  );
  const sub = await listen(
    `subscription { postChanged(operations: [CONNECT]) { operation key relationship { field relatedType relatedKey } } }`,
  );
  await settle();
  await db.execute(
    "MATCH (p:Post {key: 'p1'}), (t:Tag {key: 't1'}) CREATE (p)-[:TAGGED]->(t)",
  );
  await settle();
  await sub.stop();
  lora.close();
  expect(sub.events).toEqual([
    {
      postChanged: {
        operation: "CONNECT",
        key: "p1",
        relationship: {
          field: "Post.tags",
          relatedType: "Tag",
          relatedKey: "t1",
        },
      },
    },
  ]);
});

test("changeFeed needs a driver with changes()", () => {
  expect(
    () =>
      new LoraGraphQL({
        typeDefs,
        driver: { run: () => Promise.reject(new Error("x")) },
        changeFeed: true,
      }),
  ).toThrow("changeFeed needs a driver with changes()");
});
