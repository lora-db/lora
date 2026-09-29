import { parse, subscribe, type ExecutionResult } from "graphql";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Post
    @node
    @mutation
    @subscription
    @authorization(
      filter: [
        {
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
  type Tag @node @mutation {
    key: String! @key
  }
`;

let h: Harness;
beforeEach(async () => {
  h = await festivalHarness({ typeDefs, seed: [] });
});

async function listen(query: string, context: Record<string, unknown> = {}) {
  const controller = new AbortController();
  const result = await subscribe({
    schema: h.lora.getSchema(),
    document: parse(query),
    contextValue: { ...context, signal: controller.signal },
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
}

const settle = () => new Promise((r) => setTimeout(r, 20));

test("create, update and delete events carry the node as it is now", async () => {
  const sub = await listen(
    `subscription { postChanged(key: "p1") { operation key node { title } } }`,
  );
  await h.data(
    `mutation { createPosts(input: [{ key: "p1", title: "Hi", published: true }]) { info { nodesCreated } } }`,
  );
  await settle();
  await h.data(
    `mutation { updatePost(key: "p1", update: { title: "Hello" }) { info { nodesUpdated } } }`,
  );
  await settle();
  await h.data(`mutation { deletePost(key: "p1") { nodesDeleted } }`);
  await settle();
  await sub.stop();
  expect(sub.events).toEqual([
    { postChanged: { operation: "CREATE", key: "p1", node: { title: "Hi" } } },
    {
      postChanged: { operation: "UPDATE", key: "p1", node: { title: "Hello" } },
    },
    { postChanged: { operation: "DELETE", key: "p1", node: null } },
  ]);
});

test("deletions of rule-protected types go only to followers of the key", async () => {
  await h.data(
    `mutation { createPosts(input: [{ key: "p1", title: "Hi", published: true }]) { info { nodesCreated } } }`,
  );
  const sub = await listen(`subscription { postChanged { operation key } }`);
  await h.data(`mutation { deletePost(key: "p1") { nodesDeleted } }`);
  await settle();
  await sub.stop();
  expect(sub.events).toEqual([]);
});

test("filters by key and operation; relationship changes are updates", async () => {
  await h.data(`mutation {
    createPosts(input: [{ key: "p1", title: "A", published: true }, { key: "p2", title: "B", published: true }]) { info { nodesCreated } }
    createTags(input: [{ key: "t" }]) { info { nodesCreated } }
  }`);
  const sub = await listen(
    `subscription { postChanged(key: "p2", operations: [UPDATE]) { operation key } }`,
  );
  await h.data(
    `mutation { updatePost(key: "p1", update: { title: "A2" }) { info { nodesUpdated } } }`,
  );
  await h.data(
    `mutation { updatePost(key: "p2", update: { tags: { connect: [{ key: "t" }] } }) { info { relationshipsCreated } } }`,
  );
  await settle();
  await sub.stop();
  expect(sub.events).toEqual([
    { postChanged: { operation: "UPDATE", key: "p2" } },
  ]);
});

test("changes to nodes the subscriber cannot read are not sent", async () => {
  const sub = await listen(`subscription { postChanged { operation key } }`);
  await h.data(
    `mutation { createPosts(input: [{ key: "draft", title: "D", published: false }]) { info { nodesCreated } } }`,
  );
  await h.data(
    `mutation { createPosts(input: [{ key: "pub", title: "P", published: true }]) { info { nodesCreated } } }`,
  );
  await h.data(`mutation { deletePost(key: "draft") { nodesDeleted } }`);
  await settle();
  await sub.stop();
  expect(sub.events).toEqual([
    { postChanged: { operation: "CREATE", key: "pub" } },
  ]);
});
