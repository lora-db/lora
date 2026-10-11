// lora.worker(): nodes of an @outbox type are handed to a handler after
// the commit, at least once, with retries, and survive a restart.

import { createDatabase } from "@loradb/lora-node";
import { describe, expect, test } from "vitest";
import {
  LoraGraphQL,
  loraDriver,
  type DatabaseErrorEvent,
  type WriteChange,
} from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Post
    @node
    @mutation
    @trigger(
      on: [CREATE]
      statement: """
      CREATE (n:Notification {id: 'n-' + this.id, kind: 'POST', recipientId: this.id})
      RETURN n AS created
      """
    ) {
    id: ID! @key
  }

  type Notification @node @mutation @outbox @subscription {
    id: ID! @key
    kind: String!
    recipientId: ID!
  }
`;

type Db = Awaited<ReturnType<typeof createDatabase>>;

async function instance(db?: Db) {
  db ??= await createDatabase();
  const errors: DatabaseErrorEvent[] = [];
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    onError: (e) => errors.push(e),
  });
  await lora.assertSchema({ create: true });
  const changes: WriteChange[] = [];
  lora.onWrite((c) => changes.push(c));
  const run = async (source: string) => {
    const r = await lora.execute({ source });
    if (r.errors) throw r.errors[0];
    return r.data;
  };
  const states = async () =>
    (
      await db!.execute(
        "MATCH (n:Notification) RETURN n.id AS id, n._outbox AS state, n._outboxAttempts AS attempts ORDER BY id",
      )
    ).rows.map((r) => [r["id"], r["state"], r["attempts"] ?? null]);
  return { db, lora, run, errors, changes, states };
}

const NOTE = (id: string) =>
  `mutation { createNotifications(input: [{ id: "${id}", kind: "DIRECT", recipientId: "u1" }]) { notifications { id } } }`;
const POST = (id: string) =>
  `mutation { createPosts(input: [{ id: "${id}" }]) { posts { id } } }`;

describe("delivery", () => {
  test("a node from a generated create is handled once and marked done", async () => {
    const { lora, run, states, changes } = await instance();
    const seen: unknown[] = [];
    const worker = lora.worker("Notification", {
      handle: (node, info) => void seen.push([node, info.key, info.attempt]),
    });
    await run(NOTE("a"));
    changes.length = 0;
    await worker.idle();
    expect(seen).toEqual([
      [{ id: "a", kind: "DIRECT", recipientId: "u1" }, "a", 1],
    ]);
    expect(await states()).toEqual([["a", "DONE", null]]);
    // Delivery state is not a write a client can see.
    expect(changes).toEqual([]);
    lora.close();
  });

  test("a node a @trigger statement created is handled too", async () => {
    const { lora, run, states } = await instance();
    const seen: unknown[] = [];
    const worker = lora.worker("Notification", {
      handle: (node) => void seen.push(node["id"]),
    });
    await run(POST("p1"));
    await worker.idle();
    expect(seen).toEqual(["n-p1"]);
    expect(await states()).toEqual([["n-p1", "DONE", null]]);
    lora.close();
  });

  test("the delivery state is not part of the API", async () => {
    const { lora, run } = await instance();
    await run(NOTE("a"));
    const r = await lora.execute({ source: `{ notifications { _outbox } }` });
    expect(r.errors?.[0]?.message).toContain("_outbox");
    lora.close();
  });

  test("a rolled-back mutation leaves no work", async () => {
    const { lora, states } = await instance();
    const seen: unknown[] = [];
    const worker = lora.worker("Notification", {
      handle: (node) => void seen.push(node["id"]),
    });
    const tx = await lora.begin();
    await lora.execute({ source: NOTE("a"), context: { transaction: tx } });
    await tx.rollback();
    await worker.idle();
    expect(seen).toEqual([]);
    expect(await states()).toEqual([]);
    lora.close();
  });

  test("onDone: delete removes the node and reports it", async () => {
    const { lora, run, states, changes } = await instance();
    const worker = lora.worker("Notification", {
      handle: () => undefined,
      onDone: "delete",
    });
    await run(NOTE("a"));
    changes.length = 0;
    await worker.idle();
    expect(await states()).toEqual([]);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toMatchObject({
      operation: "DELETE",
      deleted: [{ type: "Notification", key: "a" }],
    });
    lora.close();
  });
});

describe("failures", () => {
  test("a failing handler is retried with backoff, then succeeds", async () => {
    const { lora, run, states } = await instance();
    const attempts: number[] = [];
    const worker = lora.worker("Notification", {
      handle: (_node, { attempt }) => {
        attempts.push(attempt);
        if (attempt < 3) throw new Error(`not yet ${attempt}`);
      },
      backoffMs: () => 5,
      sweepMs: 10,
    });
    await run(NOTE("a"));
    await expect
      .poll(() => states(), { timeout: 3000 })
      .toEqual([["a", "DONE", 2]]);
    expect(attempts).toEqual([1, 2, 3]);
    await worker.stop();
  });

  test("after maxAttempts the node is FAILED and reported", async () => {
    const { lora, run, states, errors } = await instance();
    const worker = lora.worker("Notification", {
      handle: () => {
        throw new Error("gateway down");
      },
      maxAttempts: 2,
      backoffMs: () => 5,
      sweepMs: 10,
    });
    await run(NOTE("a"));
    await expect
      .poll(() => states(), { timeout: 3000 })
      .toEqual([["a", "FAILED", 2]]);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatchObject({
      field: "worker:Notification",
      message: "gateway down",
    });
    // A failed node is not taken again.
    await worker.idle();
    expect(await states()).toEqual([["a", "FAILED", 2]]);
    await worker.stop();
  });
});

describe("restarts and several workers", () => {
  test("work committed before the worker started is picked up", async () => {
    const first = await instance();
    await first.run(NOTE("a"));
    await first.run(POST("p1"));
    first.lora.close();
    // A new instance on the same database, as after a restart.
    const second = await instance(first.db);
    const seen: unknown[] = [];
    const worker = second.lora.worker("Notification", {
      handle: (node) => void seen.push(node["id"]),
    });
    await worker.idle();
    expect(seen.sort()).toEqual(["a", "n-p1"]);
    second.lora.close();
  });

  test("a claim whose lease ran out is taken again", async () => {
    const { db, lora, run, states } = await instance();
    await run(NOTE("a"));
    // A worker that died mid-handler: claimed, lease in the past.
    await db.execute(
      "MATCH (n:Notification {id: 'a'}) SET n._outbox = 'CLAIMED', n._outboxLease = 1",
    );
    const seen: unknown[] = [];
    const worker = lora.worker("Notification", {
      handle: (node) => void seen.push(node["id"]),
    });
    await worker.idle();
    expect(seen).toEqual(["a"]);
    expect(await states()).toEqual([["a", "DONE", null]]);
    lora.close();
  });

  test("two workers on one database handle each node once", async () => {
    const one = await instance();
    const two = await instance(one.db);
    const seen: unknown[] = [];
    const slow = (node: Record<string, unknown>) =>
      new Promise<void>((resolve) => {
        seen.push(node["id"]);
        setTimeout(resolve, 5);
      });
    const a = one.lora.worker("Notification", { handle: slow, sweepMs: 10 });
    const b = two.lora.worker("Notification", { handle: slow, sweepMs: 10 });
    for (let i = 0; i < 20; i++) await one.run(NOTE(`k${i}`));
    await Promise.all([a.idle(), b.idle()]);
    expect(seen).toHaveLength(20);
    expect(new Set(seen).size).toBe(20);
    one.lora.close();
    two.lora.close();
  });
});

describe("worker()", () => {
  test("refuses a type without @outbox, and an unknown type", async () => {
    const { lora } = await instance();
    expect(() => lora.worker("Post", { handle: () => undefined })).toThrow(
      'worker("Post"): Post has no @outbox',
    );
    expect(() => lora.worker("Nope", { handle: () => undefined })).toThrow(
      'worker("Nope"): no @node type Nope',
    );
  });

  test("stop() ends it: later work stays pending", async () => {
    const { lora, run, states } = await instance();
    const seen: unknown[] = [];
    const worker = lora.worker("Notification", {
      handle: (node) => void seen.push(node["id"]),
    });
    await worker.stop();
    await run(NOTE("a"));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(seen).toEqual([]);
    expect(await states()).toEqual([["a", "PENDING", null]]);
  });
});
