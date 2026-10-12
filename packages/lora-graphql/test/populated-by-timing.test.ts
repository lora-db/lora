// @populatedBy callbacks of a mutation's own inputs start before the
// transaction opens, together: user code does not hold the writer lock.

import { createDatabase } from "@loradb/lora-node";
import { expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type LoraDriver } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Post @node @mutation {
    key: String! @key
    title: String
    slug: String @populatedBy(callback: "slug", operations: [CREATE, UPDATE])
    owner: String @populatedBy(callback: "owner", operations: [CREATE])
    comments: [Comment!]! @relationship(type: "ON", direction: IN)
  }
  type Comment @node @mutation {
    key: String! @key
    stamp: String @populatedBy(callback: "stamp", operations: [CREATE])
    post: Post @relationship(type: "ON", direction: OUT)
  }
`;

async function setup() {
  const db = await createDatabase();
  const events: string[] = [];
  const base = loraDriver(db);
  const driver: LoraDriver = {
    ...base,
    begin: async (options) => {
      events.push("begin");
      return base.begin!(options);
    },
    // A single-call create has no begin(): its write batch is the moment
    // the lock is taken.
    run: (statements, options) => {
      if (options.mode === "write") events.push("write");
      return base.run(statements, options);
    },
  };
  let running = 0;
  let overlap = 0;
  const callback = (label: string) => async () => {
    events.push(label);
    running++;
    overlap = Math.max(overlap, running);
    await new Promise((resolve) => setTimeout(resolve, 5));
    running--;
    return label;
  };
  const lora = new LoraGraphQL({
    typeDefs,
    driver,
    callbacks: {
      slug: callback("slug"),
      owner: callback("owner"),
      stamp: callback("stamp"),
    },
  });
  await lora.assertSchema({ create: true });
  events.length = 0;
  return { lora, events, overlap: () => overlap };
}

test("a create's callbacks run before begin(), at once", async () => {
  const { lora, events, overlap } = await setup();
  const r = await lora.execute({
    source: `mutation { createPosts(input: [{ key: "p1" }, { key: "p2" }]) { posts { key slug owner } } }`,
  });
  expect(r.errors).toBeUndefined();
  expect(r.data).toEqual({
    createPosts: {
      posts: [
        { key: "p1", slug: "slug", owner: "owner" },
        { key: "p2", slug: "slug", owner: "owner" },
      ],
    },
  });
  // Each callback once per input, all started before the transaction.
  expect(events).toEqual(["slug", "owner", "slug", "owner", "write"]);
  expect(overlap()).toBe(4);
});

test("an update's callback runs before the write, once", async () => {
  const { lora, events } = await setup();
  await lora.execute({
    source: `mutation { createPosts(input: [{ key: "p1" }]) { posts { key } } }`,
  });
  events.length = 0;
  const r = await lora.execute({
    source: `mutation { updatePost(key: "p1", update: { title: "T" }) { post { slug title } } }`,
  });
  expect(r.errors).toBeUndefined();
  expect(r.data).toEqual({
    updatePost: { post: { slug: "slug", title: "T" } },
  });
  expect(events).toEqual(["slug", "write"]);
});

test("a nested create's callback runs before its transaction opens too", async () => {
  const { lora, events } = await setup();
  const r = await lora.execute({
    source: `mutation { createPosts(input: [{ key: "p1", comments: { create: [{ node: { key: "c1" } }] } }]) { posts { comments { stamp } } } }`,
  });
  expect(r.errors).toBeUndefined();
  expect(r.data).toEqual({
    createPosts: { posts: [{ comments: [{ stamp: "stamp" }] }] },
  });
  // The plan is complete before a create decides how to reach the
  // database, so every callback has run by then.
  expect(events).toEqual(["slug", "owner", "stamp", "begin"]);
});

test("a failing callback fails the mutation with its own error", async () => {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    callbacks: {
      slug: () => {
        throw new Error("no slug today");
      },
      owner: () => "o",
      stamp: () => "s",
    },
  });
  const r = await lora.execute({
    source: `mutation { createPosts(input: [{ key: "p1" }]) { posts { key } } }`,
  });
  expect(r.errors?.[0]?.message).toContain("no slug today");
  const read = await lora.execute({ source: `{ posts { key } }` });
  expect(read.data).toEqual({ posts: [] });
});
