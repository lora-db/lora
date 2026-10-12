// A create, with the relationships it connects, is one write batch that
// commits as it returns: no begin(), no hop to JavaScript while the writer
// lock is held. Its rule checks are assert() statements of the batch.
// Everything else keeps a transaction.

import { createDatabase } from "@loradb/lora-node";
import { describe, expect, test } from "vitest";
import {
  LoraGraphQL,
  loraDriver,
  type LoraDriver,
  type Statement,
} from "../src/index.js";

type Options = Partial<ConstructorParameters<typeof LoraGraphQL>[0]>;

async function setup(
  typeDefs: string,
  options: Options = {},
  wrap?: (d: LoraDriver) => LoraDriver,
) {
  const db = await createDatabase();
  const base = loraDriver(db);
  /** How each mutation reached the database: "begin", or the batch's texts. */
  const paths: Array<"begin" | string[]> = [];
  const counted: LoraDriver = {
    ...base,
    run: (statements: Statement[], runOptions) => {
      if (runOptions.mode === "write")
        paths.push(statements.map((s) => s.text));
      return base.run(statements, runOptions);
    },
    begin: (beginOptions) => {
      if (beginOptions.mode === "write") paths.push("begin");
      return base.begin!(beginOptions);
    },
  };
  const lora = new LoraGraphQL({
    typeDefs,
    driver: wrap ? wrap(counted) : counted,
    ...options,
  });
  await lora.assertSchema({ create: true });
  paths.length = 0;
  const exec = (source: string, jwt?: Record<string, unknown>) =>
    lora.execute({ source, context: jwt ? { jwt } : {} });
  const keys = async (label: string) =>
    (
      await db.execute(`MATCH (n:${label}) RETURN n.key AS key ORDER BY key`)
    ).rows.map((r) => r["key"]);
  return { db, lora, exec, paths, keys };
}

const PLAIN = `type Item @node @mutation { key: String! @key  name: String @unique }`;
const CREATE = (key: string, name = key) =>
  `mutation { createItems(input: [{ key: "${key}", name: "${name}" }]) { items { key name } } }`;

describe("the single call", () => {
  test("a create with a nested create and a connect is one write batch", async () => {
    const { exec, paths, db } = await setup(
      `type Post @node @mutation { key: String! @key  tags: [Tag!]! @relationship(type: "HAS", direction: OUT) }
       type Tag @node @mutation { key: String! @key }`,
    );
    await db.execute("CREATE (:Tag {key: 'old'})");
    const r = await exec(
      `mutation { createPosts(input: [{ key: "p", tags: { create: [{ node: { key: "new" } }], connect: [{ key: "old" }] } }]) { posts { key tags(sort: [{ key: ASC }]) { key } } info { nodesCreated relationshipsCreated } } }`,
    );
    expect(r.errors).toBeUndefined();
    expect(r.data).toEqual({
      createPosts: {
        posts: [{ key: "p", tags: [{ key: "new" }, { key: "old" }] }],
        info: { nodesCreated: 2, relationshipsCreated: 2 },
      },
    });
    expect(paths).toHaveLength(1);
    expect(paths[0]).not.toBe("begin");
  });

  test("@uniqueTogether is checked in the batch", async () => {
    const { exec, paths, keys } = await setup(
      `type Item @node @mutation @uniqueTogether(fields: ["name", "kind"]) { key: String! @key  name: String  kind: String }`,
    );
    const create = (key: string) =>
      `mutation { createItems(input: [{ key: "${key}", name: "n", kind: "k" }]) { items { key } } }`;
    expect((await exec(create("a"))).errors).toBeUndefined();
    const r = await exec(create("b"));
    expect(r.errors?.[0]?.message).toBe(
      'Item must be unique by (name, kind); Item "b" would share it with another',
    );
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
    });
    expect(paths).toHaveLength(2);
    expect(paths).not.toContain("begin");
    expect(await keys("Item")).toEqual(["a"]);
  });

  test("a connect to a node that is not there writes nothing", async () => {
    const { exec, paths, keys } = await setup(
      `type Post @node @mutation { key: String! @key  tags: [Tag!]! @relationship(type: "HAS", direction: OUT) }
       type Tag @node @mutation { key: String! @key }`,
    );
    const r = await exec(
      `mutation { createPosts(input: [{ key: "p", tags: { connect: [{ key: "a" }, { key: "b" }] } }]) { posts { key } } }`,
    );
    expect(r.errors?.[0]?.message).toBe('Post.tags: no Tag with key "a", "b"');
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "NOT_FOUND",
      keys: ["a", "b"],
    });
    expect(paths).toHaveLength(1);
    expect(paths[0]).not.toBe("begin");
    expect(await keys("Post")).toEqual([]);
  });

  test("a create is one write batch and no transaction", async () => {
    const { exec, paths, keys } = await setup(PLAIN);
    const r = await exec(CREATE("a"));
    expect(r.data).toEqual({
      createItems: { items: [{ key: "a", name: "a" }] },
    });
    expect(paths).toHaveLength(1);
    expect(paths[0]).not.toBe("begin");
    expect(paths[0]).toHaveLength(2);
    expect(await keys("Item")).toEqual(["a"]);
  });

  test("a constraint the create breaks rolls the batch back, with the usual error", async () => {
    const { exec, keys } = await setup(PLAIN);
    await exec(CREATE("a", "same"));
    const r = await exec(CREATE("b", "same"));
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      type: "Item",
      field: "name",
    });
    expect(await keys("Item")).toEqual(["a"]);
  });

  const RULED = /* GraphQL */ `
    extend schema @authorizationDefaults(requireAuthentication: false)
    type Doc
      @node
      @mutation
      @authorization(
        filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
        validate: [
          {
            operations: [CREATE]
            where: { node: { owner: { eq: "$jwt.sub" } } }
          }
        ]
      ) {
      key: String! @key
      owner: String
      slug: String @unique
    }
  `;

  test("a rule on the stored node refuses inside the batch", async () => {
    const { exec, paths, keys } = await setup(RULED);
    const denied = await exec(
      `mutation { createDocs(input: [{ key: "d", owner: "someone-else" }]) { docs { key } } }`,
      { sub: "me" },
    );
    expect(denied.errors?.[0]).toMatchObject({
      message: "not allowed to create this Doc",
      extensions: { code: "FORBIDDEN" },
    });
    expect(paths).toHaveLength(1);
    expect(
      (paths[0] as string[]).some((text) => text.includes("assert(")),
    ).toBe(true);
    expect(await keys("Doc")).toEqual([]);
    const allowed = await exec(
      `mutation { createDocs(input: [{ key: "d", owner: "me" }]) { docs { key } } }`,
      { sub: "me" },
    );
    expect(allowed.errors).toBeUndefined();
    expect(await keys("Doc")).toEqual(["d"]);
  });

  test("a key held by a node the caller cannot see is refused, and that node is untouched", async () => {
    const { db, exec, paths } = await setup(RULED);
    await db.execute(
      "CREATE (:Doc {key: 'taken', owner: 'someone-else', slug: 's'})",
    );
    const r = await exec(
      `mutation { createDocs(input: [{ key: "taken", owner: "me" }]) { docs { key } } }`,
      { sub: "me" },
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    expect(paths[0]).not.toBe("begin");
    const { rows } = await db.execute(
      "MATCH (d:Doc) RETURN d.key AS key, d.owner AS owner",
    );
    expect(rows).toEqual([{ key: "taken", owner: "someone-else" }]);
  });

  test("a unique value another node holds is CONSTRAINT_VIOLATION, and that node keeps it", async () => {
    const { db, exec } = await setup(RULED);
    await db.execute(
      "CREATE (:Doc {key: 'other', owner: 'someone-else', slug: 'held'})",
    );
    const r = await exec(
      `mutation { createDocs(input: [{ key: "mine", owner: "me", slug: "held" }]) { docs { key } } }`,
      { sub: "me" },
    );
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      type: "Doc",
      field: "slug",
    });
    const { rows } = await db.execute(
      "MATCH (d:Doc) RETURN d.key AS key, d.slug AS slug",
    );
    expect(rows).toEqual([{ key: "other", slug: "held" }]);
  });

  test("a rule the claims alone settle refuses before anything is sent", async () => {
    const { exec, paths, keys } = await setup(/* GraphQL */ `
      extend schema @authorizationDefaults(requireAuthentication: false)
      type User @node @mutation {
        key: String! @key
        verified: Boolean
          @authorization(
            validate: [
              {
                operations: [CREATE]
                where: { jwt: { role: { eq: "admin" } } }
              }
            ]
          )
      }
    `);
    for (let i = 0; i < 2; i++) {
      const r = await exec(
        `mutation { createUsers(input: [{ key: "u", verified: true }]) { users { key } } }`,
        { role: "member" },
      );
      expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    }
    expect(paths).toEqual([]);
    expect(await keys("User")).toEqual([]);
  });

  test("it is bounded by maxQueuedWrites and by timeoutMs like a transaction", async () => {
    const { lora, exec, keys } = await setup(PLAIN, {
      maxQueuedWrites: 1,
      timeoutMs: 80,
    });
    const holder = await lora.begin();
    // A transaction holds the lock, so every batch counted is waiting.
    const waiting = exec(CREATE("a"));
    const refused = await exec(CREATE("b"));
    expect(refused.errors?.[0]?.extensions?.["code"]).toBe("OVERLOADED");
    // The one that waits gives up at timeoutMs.
    expect((await waiting).errors).toHaveLength(1);
    await holder.commit();
    expect(await keys("Item")).toEqual([]);
    expect((await exec(CREATE("c"))).errors).toBeUndefined();
    expect(await keys("Item")).toEqual(["c"]);
  });
});

describe("what keeps a transaction", () => {
  const usesBegin = async (
    typeDefs: string,
    source: string,
    options: Options = {},
    wrap?: (d: LoraDriver) => LoraDriver,
  ) => {
    const { exec, paths } = await setup(typeDefs, options, wrap);
    const r = await exec(source);
    expect(r.errors).toBeUndefined();
    return paths;
  };

  test("singleCallMutations: false", async () => {
    expect(
      await usesBegin(PLAIN, CREATE("a"), { singleCallMutations: false }),
    ).toEqual(["begin"]);
  });

  const TAGGED = `type Post @node @mutation { key: String! @key  tags: [Tag!]! @relationship(type: "HAS", direction: OUT) }
       type Tag @node @mutation { key: String! @key  posts: [Post!]! @relationship(type: "HAS", direction: IN) }`;

  test("a create that names one relationship from both sides", async () => {
    // The second link finds the first one's relationship: a re-connect.
    const paths = await usesBegin(
      TAGGED,
      `mutation { createPosts(input: [{ key: "p", tags: { create: [{ node: { key: "t", posts: { connect: [{ key: "p" }] } } }] } }]) { posts { key } } }`,
    );
    expect(paths).toEqual(["begin"]);
  });

  test("a schema with a trigger", async () => {
    const paths = await usesBegin(
      `type Item @node @mutation @trigger(on: [CREATE], statement: "SET this.seen = true RETURN this AS updated") { key: String! @key  name: String  seen: Boolean }`,
      CREATE("a"),
    );
    expect(paths).toEqual(["begin"]);
  });

  test("a schema where a read can be refused", async () => {
    const paths = await usesBegin(
      `extend schema @authorizationDefaults(requireAuthentication: false)
       type Item @node @mutation { key: String! @key  name: String }
       type Secret @node @authorization(validate: [{ operations: [READ], where: { jwt: { role: { eq: "admin" } } } }]) { key: String! @key }`,
      CREATE("a"),
    );
    expect(paths).toEqual(["begin"]);
  });

  test("a database without assert()", async () => {
    const paths = await usesBegin(PLAIN, CREATE("a"), {}, (driver) => ({
      ...driver,
      run: (statements, options) =>
        statements[0]!.text.startsWith("RETURN assert(")
          ? Promise.reject(new Error("unknown function `assert`"))
          : driver.run(statements, options),
    }));
    expect(paths).toEqual(["begin"]);
  });

  test("a caller-owned transaction, and a delete", async () => {
    const { lora, exec, paths } = await setup(PLAIN);
    const tx = await lora.begin();
    paths.length = 0;
    const r = await lora.execute({
      source: CREATE("a"),
      context: { transaction: tx },
    });
    expect(r.errors).toBeUndefined();
    expect(paths).toEqual([]);
    await tx.commit();
    await exec(`mutation { deleteItem(key: "a") { nodesDeleted } }`);
    expect(paths).toEqual(["begin"]);
  });
});

describe("review regressions", () => {
  const RULED = /* GraphQL */ `
    extend schema @authorizationDefaults(requireAuthentication: false)
    type Doc
      @node
      @mutation
      @authorization(
        validate: [
          {
            operations: [CREATE]
            where: { node: { owner: { eq: "$jwt.sub" } } }
          }
        ]
      ) {
      key: String! @key
      owner: String
      a: String @unique
      b: String @unique
    }
  `;

  // A batch fails as a whole. With several statements issued together
  // (one create per shape of input), every one of them rejects: all must
  // be handled, or Node ends the process on the unhandled rejection.
  // vitest fails the run on one, so these pass only when none is left.
  test("a refused create with several shapes leaves no unhandled rejection", async () => {
    const { exec, keys } = await setup(RULED);
    const r = await exec(
      `mutation { createDocs(input: [{ key: "x", owner: "me" }, { key: "y", owner: "other", a: "1" }]) { docs { key } } }`,
      { sub: "me" },
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(await keys("Doc")).toEqual([]);
  });

  test("a taken unique value among several leaves none either", async () => {
    const { db, exec, keys } = await setup(RULED);
    await db.execute("CREATE (:Doc {key: 'held', owner: 'other', a: 'taken'})");
    const r = await exec(
      `mutation { createDocs(input: [{ key: "x", owner: "me", a: "taken", b: "free" }]) { docs { key } } }`,
      { sub: "me" },
    );
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "CONSTRAINT_VIOLATION",
      field: "a",
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(await keys("Doc")).toEqual(["held"]);
  });

  test("a duplicate key among several shapes, in a transaction too", async () => {
    const { exec, keys } = await setup(PLAIN, { singleCallMutations: false });
    await exec(CREATE("a"));
    const r = await exec(
      `mutation { createItems(input: [{ key: "a" }, { key: "b", name: "n" }]) { items { key } } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("CONSTRAINT_VIOLATION");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(await keys("Item")).toEqual(["a"]);
  });

  test.each([true, false])(
    "a tracer that throws does not fail a write that ran (single call: %s)",
    async (singleCallMutations) => {
      const { exec, keys } = await setup(PLAIN, {
        singleCallMutations,
        tracer: {
          startActiveSpan: (_name, fn) =>
            fn({
              setAttribute: () => undefined,
              setStatus: () => undefined,
              recordException: () => undefined,
              end: () => {
                throw new Error("span exporter down");
              },
            }),
        },
      });
      const r = await exec(CREATE("a"));
      expect(r.errors).toBeUndefined();
      expect(await keys("Item")).toEqual(["a"]);
    },
  );

  test("with the lock free, one create runs and maxQueuedWrites more may wait", async () => {
    // A write batch is counted until it returns: the one running must
    // not use up the queue's only place.
    const slow = (driver: LoraDriver): LoraDriver => ({
      ...driver,
      run: async (statements, options) => {
        if (options.mode === "write") {
          await new Promise((resolve) => setTimeout(resolve, 30));
        }
        return driver.run(statements, options);
      },
    });
    const { exec, keys } = await setup(PLAIN, { maxQueuedWrites: 1 }, slow);
    const first = exec(CREATE("a"));
    await new Promise((resolve) => setTimeout(resolve, 5));
    const second = exec(CREATE("b"));
    const third = await exec(CREATE("c"));
    expect(third.errors?.[0]?.extensions?.["code"]).toBe("OVERLOADED");
    expect((await first).errors).toBeUndefined();
    expect((await second).errors).toBeUndefined();
    expect(await keys("Item")).toEqual(["a", "b"]);
  });

  test("a worker's own writes are never refused by the write queue", async () => {
    const db = await createDatabase();
    const errors: string[] = [];
    const lora = new LoraGraphQL({
      typeDefs: `type Job @node @mutation @outbox { key: String! @key }`,
      driver: loraDriver(db),
      maxQueuedWrites: 1,
      onError: (e) => errors.push(e.message),
    });
    await lora.assertSchema({ create: true });
    let handled = 0;
    const worker = lora.worker("Job", { handle: () => void handled++ });
    await lora.execute({
      source: `mutation { createJobs(input: [{ key: "j" }]) { jobs { key } } }`,
    });
    await worker.idle();
    // Hold the lock with the queue full, as a slow writer would.
    const holder = await lora.begin();
    const waiting = lora.execute({
      source: `mutation { createJobs(input: [{ key: "k" }]) { jobs { key } } }`,
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    await holder.commit();
    await waiting;
    await worker.idle();
    expect(handled).toBe(2);
    expect(errors).toEqual([]);
    const { rows } = await db.execute(
      "MATCH (j:Job) RETURN j.key AS key, j._outbox AS state ORDER BY key",
    );
    expect(rows).toEqual([
      { key: "j", state: "DONE" },
      { key: "k", state: "DONE" },
    ]);
    lora.close();
  });

  test("a developer's own assert in a transaction keeps its message", async () => {
    const { exec } = await setup(
      `type Item @node @mutation @trigger(on: [CREATE], statement: "RETURN assert(false, 'lora-graphql:forbidden:Item:CREATE') AS ok") { key: String! @key  name: String }`,
      { maskErrors: false },
    );
    const r = await exec(CREATE("a"));
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("DATABASE_ERROR");
    expect(r.errors?.[0]?.message).toContain("assertion failed");
  });
});

describe("second review", () => {
  test.each([true, false])(
    "an aborted request reports the abort (single call: %s)",
    async (singleCallMutations) => {
      const { lora, keys } = await setup(PLAIN, {
        singleCallMutations,
        maskErrors: false,
      });
      const controller = new AbortController();
      const pending = lora.execute({
        source: CREATE("a"),
        context: { signal: controller.signal },
      });
      controller.abort();
      const r = await pending;
      // Either it finished first or it was aborted: never a TypeError
      // from rewriting the abort's read-only message.
      for (const error of r.errors ?? []) {
        expect(error.message).not.toContain("only a getter");
      }
      const stored = await keys("Item");
      expect(stored).toEqual(r.errors ? [] : ["a"]);
    },
  );
});
