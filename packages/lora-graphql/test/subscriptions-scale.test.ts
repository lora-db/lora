// Phase 18: shared checks across subscribers, richer events.

import { createDatabase } from "@loradb/lora-node";
import { parse, subscribe, type ExecutionResult } from "graphql";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type LoraDriver } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)
  type Post
    @node
    @mutation
    @subscription(relationships: true, previousState: true)
    @authorization(
      filter: [
        {
          operations: [READ, SUBSCRIBE]
          where: { node: { published: { eq: true } } }
        }
      ]
    ) {
    key: String! @key
    title: String! @filterable
    published: Boolean!
    secret: String @private
    tags: [Tag!]! @relationship(type: "TAGGED", direction: OUT)
  }
  type Tag @node @mutation {
    key: String! @key
  }
`;

async function setup() {
  const db = await createDatabase();
  const base = loraDriver(db);
  let reads = 0;
  const driver: LoraDriver = {
    ...base,
    run: (statements, options) => {
      if (options.mode === "read") reads++;
      return base.run(statements, options);
    },
  };
  const lora = new LoraGraphQL({ typeDefs, driver });
  await lora.assertSchema({ create: true });
  const mutate = async (source: string) => {
    const r = await lora.execute({ source });
    if (r.errors) throw r.errors[0];
  };
  const listen = async (query: string) => {
    const controller = new AbortController();
    const result = await subscribe({
      schema: lora.getSchema(),
      document: parse(query),
      contextValue: { signal: controller.signal },
    });
    if (!(Symbol.asyncIterator in result)) throw result.errors?.[0];
    const events: unknown[] = [];
    const done = (async () => {
      for await (const r of result as AsyncIterable<ExecutionResult>) {
        events.push(
          r.errors ? { errors: r.errors.map((e) => e.message) } : r.data,
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
  };
  return { lora, mutate, listen, reads: () => reads };
}

const settle = () => new Promise((r) => setTimeout(r, 30));

test("subscribers with the same checks share one read per change", async () => {
  const { mutate, listen, reads } = await setup();
  const query = `subscription { postChanged(where: { title: { eq: "Hi" } }) { operation node { title } } }`;
  const subs = await Promise.all(
    Array.from({ length: 20 }, () => listen(query)),
  );
  const before = reads();
  await mutate(
    `mutation { createPosts(input: [{ key: "p1", title: "Hi", published: true }]) { info { nodesCreated } } }`,
  );
  await settle();
  // One visibility query and one node read, for twenty subscribers.
  expect(reads() - before).toBe(2);
  for (const s of subs) {
    expect(s.events).toEqual([
      { postChanged: { operation: "CREATE", node: { title: "Hi" } } },
    ]);
    await s.stop();
  }
});

test("timestamp, previous state, and relationship events", async () => {
  const { mutate, listen } = await setup();
  await mutate(
    `mutation { createTags(input: [{ key: "t1" }]) { info { nodesCreated } } }`,
  );
  await mutate(
    `mutation { createPosts(input: [{ key: "p1", title: "Hi", published: true }]) { info { nodesCreated } } }`,
  );
  const sub = await listen(`subscription { postChanged(key: "p1") {
    operation timestamp previousState { title published } relationship { field type relatedType relatedKey }
  } }`);
  const t0 = Date.now();
  await mutate(
    `mutation { updatePost(key: "p1", update: { title: "Hello", tags: { connect: [{ key: "t1" }] } }) { info { nodesUpdated } } }`,
  );
  await settle();
  await mutate(`mutation { deletePost(key: "p1") { nodesDeleted } }`);
  await settle();
  await sub.stop();
  const events = sub.events as Array<{ postChanged: Record<string, unknown> }>;
  expect(events.map((e) => e.postChanged["operation"])).toEqual([
    "UPDATE",
    "CONNECT",
    "DELETE",
  ]);
  const [update, connect, del] = events.map((e) => e.postChanged);
  expect(update).toMatchObject({
    previousState: { title: "Hi", published: true },
    relationship: null,
  });
  expect(Date.parse(update!["timestamp"] as string)).toBeGreaterThanOrEqual(
    t0 - 1000,
  );
  expect(connect).toMatchObject({
    relationship: {
      field: "Post.tags",
      type: "TAGGED",
      relatedType: "Tag",
      relatedKey: "t1",
    },
  });
  expect(del).toMatchObject({ previousState: { title: "Hello" } });
});

test("previousState leaves out private fields", async () => {
  const { lora } = await setup();
  const sdl = lora.printPublicSchema();
  const block = sdl.match(/type PostPreviousState \{[^}]*\}/)![0];
  expect(block).toContain("title: String");
  expect(block).not.toContain("secret");
});
