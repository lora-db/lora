// Triggers and workers on the paths the main suites do not walk: upserts,
// bulk updates, cascading deletes, operation-level transactions, $viewer,
// the engine change feed, and failures inside a batch.

import { createDatabase } from "@loradb/lora-node";
import { describe, expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type WriteChange } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type Claims @jwt {
    sub: String! @viewer(type: "Person", field: "key")
  }

  type Person @node @mutation {
    key: String! @key
  }

  type Folder @node @mutation {
    key: String! @key
    files: [File!]!
      @relationship(type: "HOLDS", direction: OUT, onDelete: CASCADE)
  }

  type File
    @node
    @mutation
    @trigger(
      on: [CREATE, UPDATE, DELETE]
      statement: """
      CREATE (l:Log {key: $operation + ':' + this.key, by: $viewer, operation: $operation})
      RETURN l AS created
      """
    ) {
    key: String! @key
    name: String @unique
    size: Int
    folder: Folder @relationship(type: "HOLDS", direction: IN)
  }

  type Log @node {
    key: String! @key
    by: String
    operation: String!
  }
`;

async function setup(options: { changeFeed?: boolean; atomic?: boolean } = {}) {
  const db = await createDatabase();
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    ...(options.changeFeed ? { changeFeed: true } : {}),
    ...(options.atomic ? { mutationTransaction: "operation" as const } : {}),
  });
  await lora.assertSchema({ create: true });
  await db.execute("CREATE (:Person {key: 'ada'})");
  const changes: WriteChange[] = [];
  lora.onWrite((c) => changes.push(c));
  const run = async (source: string) => {
    const r = await lora.execute({ source, context: { jwt: { sub: "ada" } } });
    if (r.errors) throw r.errors[0];
    return r.data;
  };
  const logs = async () =>
    (
      (await run(`{ logs(sort: [{ key: ASC }]) { key by } }`)) as {
        logs: Array<{ key: string; by: string }>;
      }
    ).logs.map((l) => l.key);
  return { db, lora, run, logs, changes };
}

describe("which operation fires", () => {
  test("an upsert fires CREATE for a new key and UPDATE for an existing one", async () => {
    const { run, logs } = await setup();
    await run(
      `mutation { upsertFiles(input: [{ key: "a", size: 1 }]) { files { key } } }`,
    );
    expect(await logs()).toEqual(["CREATE:a"]);
    await run(
      `mutation { upsertFiles(input: [{ key: "a", size: 2 }, { key: "b", size: 1 }]) { files { key } } }`,
    );
    expect(await logs()).toEqual(["CREATE:a", "CREATE:b", "UPDATE:a"]);
  });

  test("a bulk update fires once for every node it changed", async () => {
    const { run, logs } = await setup();
    await run(
      `mutation { createFiles(input: [{ key: "a", size: 1 }, { key: "b", size: 1 }, { key: "c", size: 9 }]) { files { key } } }`,
    );
    await run(
      `mutation { updateFiles(where: { key: { in: ["a", "b"] } }, update: { size: 5 }) { files { key } } }`,
    );
    expect(await logs()).toEqual([
      "CREATE:a",
      "CREATE:b",
      "CREATE:c",
      "UPDATE:a",
      "UPDATE:b",
    ]);
  });

  test("a cascading delete fires DELETE for the nodes it reaches", async () => {
    const { run, logs, changes } = await setup();
    await run(`mutation {
      createFolders(input: [{ key: "f", files: { create: [{ node: { key: "a" } }, { node: { key: "b" } }] } }]) { folders { key } }
    }`);
    changes.length = 0;
    await run(`mutation { deleteFolder(key: "f") { nodesDeleted } }`);
    expect(await logs()).toEqual([
      "CREATE:a",
      "CREATE:b",
      "DELETE:a",
      "DELETE:b",
    ]);
    expect(changes[0]!.created).toEqual([
      { type: "Log", key: "DELETE:a" },
      { type: "Log", key: "DELETE:b" },
    ]);
    expect(changes[0]!.deleted).toHaveLength(3);
  });

  test("$viewer is the caller's node key", async () => {
    const { run } = await setup();
    await run(
      `mutation { createFiles(input: [{ key: "a" }]) { files { key } } }`,
    );
    expect(await run(`{ logs { by operation } }`)).toEqual({
      logs: [{ by: "ada", operation: "CREATE" }],
    });
  });
});

describe("transactions", () => {
  test("under mutationTransaction: operation, a later failing field rolls the triggers back", async () => {
    const { lora, logs } = await setup({ atomic: true });
    const r = await lora.execute({
      source: `mutation {
        one: createFiles(input: [{ key: "a", name: "same" }]) { files { key } }
        two: createFiles(input: [{ key: "b", name: "same" }]) { files { key } }
      }`,
      context: { jwt: { sub: "ada" } },
    });
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(r.data).toBeNull();
    expect(await logs()).toEqual([]);
  });

  test("a constraint violation in an update's write is reported and nothing changes", async () => {
    const { run, lora, logs } = await setup();
    await run(
      `mutation { createFiles(input: [{ key: "a", name: "one" }, { key: "b", name: "two" }]) { files { key } } }`,
    );
    // The SET shares a round trip with the trigger and the payload read:
    // its failure must surface as the mutation's error.
    const r = await lora.execute({
      source: `mutation { updateFile(key: "b", update: { name: "one" }) { file { key name } } }`,
      context: { jwt: { sub: "ada" } },
    });
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(await logs()).toEqual(["CREATE:a", "CREATE:b"]);
    expect(await run(`{ file(key: "b") { name } }`)).toEqual({
      file: { name: "two" },
    });
  });
});

describe("with the engine change feed", () => {
  const outbox = /* GraphQL */ `
    type Job @node @mutation @outbox @subscription {
      key: String! @key
      note: String
    }
  `;

  test("a worker hears a node created by raw Cypher once it is stamped, and its state writes are not events", async () => {
    const db = await createDatabase();
    const lora = new LoraGraphQL({
      typeDefs: outbox,
      driver: loraDriver(db),
      changeFeed: true,
    });
    await lora.assertSchema({ create: true });
    const events: string[] = [];
    const controller = new AbortController();
    const stream = (await lora.subscribe({
      source: `subscription { jobChanged { operation key } }`,
      context: { signal: controller.signal },
    })) as AsyncIterableIterator<{
      data?: { jobChanged: { operation: string; key: string } };
    }>;
    void (async () => {
      for await (const e of stream) {
        events.push(
          `${e.data!.jobChanged.operation}:${e.data!.jobChanged.key}`,
        );
      }
    })().catch(() => undefined);
    const seen: unknown[] = [];
    const worker = lora.worker("Job", {
      handle: (node) => void seen.push(node["key"]),
    });
    // The feed starts with its first consumer; give it a turn.
    await new Promise((resolve) => setTimeout(resolve, 20));
    await lora.execute({
      source: `mutation { createJobs(input: [{ key: "api" }]) { jobs { key } } }`,
    });
    await db.execute("CREATE (:Job {key: 'raw', _outbox: 'PENDING'})");
    await expect
      .poll(() => seen.slice().sort(), { timeout: 3000 })
      .toEqual(["api", "raw"]);
    await worker.idle();
    await new Promise((resolve) => setTimeout(resolve, 50));
    // One CREATE each: claiming and finishing a job is not an UPDATE event.
    expect(events.sort()).toEqual(["CREATE:api", "CREATE:raw"]);
    controller.abort();
    lora.close();
  });
});
