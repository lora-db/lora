// @trigger: a statement run inside the transaction of the mutation that
// wrote a node of the type, before the commit.

import { createDatabase } from "@loradb/lora-node";
import { GraphQLError } from "graphql";
import { describe, expect, test } from "vitest";
import {
  LoraGraphQL,
  ModelError,
  loraDriver,
  type LoraDriver,
  type Statement,
  type WriteChange,
} from "../src/index.js";

const typeDefs = /* GraphQL */ `
  type User @node @mutation {
    id: ID! @key
    posts: [Post!]! @relationship(type: "WROTE", direction: OUT)
  }

  type Post @node @mutation {
    id: ID! @key
    title: String!
    comments: Int
    author: User @relationship(type: "WROTE", direction: IN)
    replies: [Comment!]! @relationship(type: "ON", direction: IN)
  }

  type Comment
    @node
    @mutation
    @trigger(
      on: [CREATE]
      statement: """
      MATCH (this)-[:ON]->(post:Post)<-[:WROTE]-(author:User)
      WHERE author.id <> $jwt.sub
      CREATE (n:Notification {
        id: this.id + ':' + author.id, kind: "COMMENT",
        recipientId: author.id, by: $jwt.sub, operation: $operation
      })
      RETURN n AS created
      """
    )
    @trigger(
      on: [CREATE, DELETE]
      statement: """
      MATCH (this)-[:ON]->(post:Post)
      SET post.comments = coalesce(post.comments, 0)
        + CASE $operation WHEN "DELETE" THEN -1 ELSE 1 END
      RETURN post AS updated
      """
    )
    @trigger(
      on: [UPDATE]
      fields: ["text"]
      statement: "SET this.edited = true RETURN this AS updated"
    ) {
    id: ID! @key
    text: String!
    pinned: Boolean
    edited: Boolean
    post: Post @relationship(type: "ON", direction: OUT)
  }

  type Notification @node @subscription(operations: [CREATE]) {
    id: ID! @key
    kind: String!
    recipientId: ID!
    by: ID
    operation: String
  }
`;

async function setup(extra: { calls?: number[] } = {}) {
  const db = await createDatabase();
  const base = loraDriver(db);
  const driver: LoraDriver = extra.calls
    ? {
        ...base,
        begin: async (options) => {
          const tx = await base.begin!(options);
          return {
            execute: (s: Statement) => (extra.calls!.push(1), tx.execute(s)),
            executeMany: (s: Statement[]) => (
              extra.calls!.push(s.length),
              tx.executeMany!(s)
            ),
            commit: () => tx.commit(),
            rollback: () => tx.rollback(),
            get isOpen() {
              return tx.isOpen;
            },
          };
        },
      }
    : base;
  const lora = new LoraGraphQL({ typeDefs, driver });
  await lora.assertSchema({ create: true });
  const changes: WriteChange[] = [];
  lora.onWrite((change) => changes.push(change));
  const run = async (source: string, sub = "u2") => {
    const r = await lora.execute({ source, context: { jwt: { sub } } });
    if (r.errors) throw r.errors[0];
    return r.data;
  };
  await run(`mutation {
    createUsers(input: [{ id: "u1" }, { id: "u2" }]) { users { id } }
    createPosts(input: [{ id: "p1", title: "Hello", author: { connect: { id: "u1" } } }]) { posts { id } }
  }`);
  changes.length = 0;
  return { lora, db, run, changes };
}

const COMMENT = `mutation {
  createComments(input: [{ id: "c1", text: "Nice", post: { connect: { id: "p1" } } }]) {
    comments { id post { comments } }
  }
}`;

describe("CREATE triggers", () => {
  test("write in the mutation's transaction, and the payload shows it", async () => {
    const { run } = await setup();
    expect(await run(COMMENT)).toEqual({
      createComments: { comments: [{ id: "c1", post: { comments: 1 } }] },
    });
    expect(
      await run(`{ notifications { id kind recipientId by operation } }`),
    ).toEqual({
      notifications: [
        {
          id: "c1:u1",
          kind: "COMMENT",
          recipientId: "u1",
          by: "u2",
          operation: "CREATE",
        },
      ],
    });
  });

  test("the statement's WHERE is honoured: no notification to oneself", async () => {
    const { run } = await setup();
    await run(COMMENT, "u1");
    expect(await run(`{ notifications { id } }`)).toEqual({
      notifications: [],
    });
  });

  test("nodes a statement returns join the write-set", async () => {
    const { run, changes } = await setup();
    await run(COMMENT);
    expect(changes).toHaveLength(1);
    const [change] = changes;
    expect(change!.broad).toBe(false);
    expect(change!.created).toEqual([
      { type: "Comment", key: "c1" },
      { type: "Notification", key: "c1:u1" },
    ]);
    expect(change!.updated).toEqual([{ type: "Post", key: "p1" }]);
    expect(change!.types).toEqual(["Comment", "Notification", "Post"]);
  });

  test("a subscription on the created type hears it without changeFeed", async () => {
    const { lora, run } = await setup();
    const events = (await lora.subscribe({
      source: `subscription { notificationChanged { operation id node { recipientId } } }`,
      context: { jwt: { sub: "u1" } },
    })) as AsyncIterableIterator<{ data?: unknown }>;
    const next = events.next();
    await run(COMMENT);
    expect((await next).value.data).toEqual({
      notificationChanged: {
        operation: "CREATE",
        id: "c1:u1",
        node: { recipientId: "u1" },
      },
    });
    await events.return?.();
  });

  test("fire for a node created by a nested create", async () => {
    const { run } = await setup();
    await run(`mutation {
      updatePost(id: "p1", update: { replies: { create: [{ node: { id: "c7", text: "Nested" } }] } }) { post { id } }
    }`);
    expect(await run(`{ notifications { id } posts { comments } }`)).toEqual({
      notifications: [{ id: "c7:u1" }],
      posts: [{ comments: 1 }],
    });
  });

  test("one statement per trigger, whatever the number of nodes", async () => {
    const calls: number[] = [];
    const { run } = await setup({ calls });
    calls.length = 0;
    await run(`mutation {
      createComments(input: [
        { id: "a", text: "1", post: { connect: { id: "p1" } } },
        { id: "b", text: "2", post: { connect: { id: "p1" } } },
        { id: "c", text: "3", post: { connect: { id: "p1" } } }
      ]) { comments { id } }
    }`);
    // create + link, then the two CREATE triggers with the payload read.
    expect(calls).toEqual([2, 3]);
    expect(await run(`{ notifications { id } posts { comments } }`)).toEqual({
      notifications: [{ id: "a:u1" }, { id: "b:u1" }, { id: "c:u1" }],
      posts: [{ comments: 3 }],
    });
  });

  test("a failing statement rolls the mutation back", async () => {
    const { run } = await setup();
    await run(COMMENT);
    await run(`mutation { deleteComment(id: "c1") { nodesDeleted } }`);
    // The notification c1:u1 is still there, so creating the comment again
    // makes the trigger's CREATE break the key of Notification.
    await expect(run(COMMENT)).rejects.toMatchObject({
      extensions: { code: "CONSTRAINT_VIOLATION" },
    });
    expect(await run(`{ comments { id } posts { comments } }`)).toEqual({
      comments: [],
      posts: [{ comments: 0 }],
    });
  });

  test("nothing is written when the mutation itself is rolled back", async () => {
    const { lora, run } = await setup();
    const tx = await lora.begin();
    const r = await lora.execute({
      source: COMMENT,
      context: { jwt: { sub: "u2" }, transaction: tx },
    });
    expect(r.errors).toBeUndefined();
    await tx.rollback();
    expect(
      await run(`{ comments { id } notifications { id } posts { comments } }`),
    ).toEqual({
      comments: [],
      notifications: [],
      posts: [{ comments: null }],
    });
  });
});

describe("UPDATE triggers", () => {
  test("fire only when one of fields was set", async () => {
    const { run } = await setup();
    await run(COMMENT);
    expect(
      await run(
        `mutation { updateComment(id: "c1", update: { pinned: true }) { comment { edited } } }`,
      ),
    ).toEqual({ updateComment: { comment: { edited: null } } });
    expect(
      await run(
        `mutation { updateComment(id: "c1", update: { text: "Nicer" }) { comment { edited } } }`,
      ),
    ).toEqual({ updateComment: { comment: { edited: true } } });
  });

  test("a node created and updated in one mutation fires CREATE only", async () => {
    const { run, changes } = await setup();
    await run(COMMENT);
    expect(changes[0]!.updated).not.toContainEqual({
      type: "Comment",
      key: "c1",
    });
    expect(await run(`{ comments { edited } }`)).toEqual({
      comments: [{ edited: null }],
    });
  });
});

describe("DELETE triggers", () => {
  test("run while the node still exists", async () => {
    const { run, changes } = await setup();
    await run(COMMENT);
    changes.length = 0;
    await run(`mutation { deleteComment(id: "c1") { nodesDeleted } }`);
    expect(await run(`{ posts { comments } comments { id } }`)).toEqual({
      posts: [{ comments: 0 }],
      comments: [],
    });
    expect(changes[0]!.deleted).toEqual([{ type: "Comment", key: "c1" }]);
    expect(changes[0]!.updated).toEqual([{ type: "Post", key: "p1" }]);
  });
});

describe("no cascades", () => {
  test("what a trigger writes fires no trigger", async () => {
    const db = await createDatabase();
    const lora = new LoraGraphQL({
      driver: loraDriver(db),
      typeDefs: /* GraphQL */ `
        type A
          @node
          @mutation
          @trigger(
            on: [CREATE]
            statement: "CREATE (b:B {id: this.id}) RETURN b AS created"
          ) {
          id: ID! @key
        }
        type B
          @node
          @mutation
          @trigger(
            on: [CREATE]
            statement: "CREATE (c:C {id: this.id}) RETURN c AS created"
          ) {
          id: ID! @key
        }
        type C @node {
          id: ID! @key
        }
      `,
    });
    await lora.assertSchema({ create: true });
    const r = await lora.execute({
      source: `mutation { createAs(input: [{ id: "x" }]) { as { id } } }`,
    });
    expect(r.errors).toBeUndefined();
    const read = await lora.execute({ source: `{ bs { id } cs { id } }` });
    expect(read.data).toEqual({ bs: [{ id: "x" }], cs: [] });
  });
});

describe("a statement that writes without reporting", () => {
  test("makes the change broad", async () => {
    const db = await createDatabase();
    const lora = new LoraGraphQL({
      driver: loraDriver(db),
      typeDefs: /* GraphQL */ `
        type A
          @node
          @mutation
          @trigger(on: [CREATE], statement: "CREATE (:Audit {of: this.id})") {
          id: ID! @key
        }
      `,
    });
    await lora.assertSchema({ create: true });
    const changes: WriteChange[] = [];
    lora.onWrite((c) => changes.push(c));
    await lora.execute({
      source: `mutation { createAs(input: [{ id: "x" }]) { as { id } } }`,
    });
    expect(changes[0]!.broad).toBe(true);
  });
});

describe("startup checks", () => {
  const model = (directive: string, fields = "id: ID! @key\n title: String") =>
    new LoraGraphQL({
      typeDefs: `type T @node @mutation ${directive} { ${fields} }`,
      driver: undefined as never,
    });
  const problems = (build: () => unknown): string[] => {
    try {
      build();
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
    return [];
  };

  test("an unknown parameter", () => {
    expect(
      problems(() =>
        model(`@trigger(on: [CREATE], statement: "SET this.x = $nope")`),
      ),
    ).toEqual([
      "@trigger: the statement uses $nope; a trigger statement has $jwt, $viewer and $operation",
    ]);
  });

  test("$viewer without a @viewer claim", () => {
    expect(
      problems(() =>
        model(`@trigger(on: [CREATE], statement: "SET this.x = $viewer")`),
      ),
    ).toEqual([
      "@trigger: the statement uses $viewer, but no @jwt claim has @viewer",
    ]);
  });

  test("fields without UPDATE, and a field the type lacks", () => {
    expect(
      problems(() =>
        model(
          `@trigger(on: [CREATE], fields: ["title"], statement: "SET this.x = 1")`,
        ),
      ),
    ).toEqual(["@trigger: fields narrows UPDATE, which on does not list"]);
    expect(
      problems(() =>
        model(
          `@trigger(on: [UPDATE], fields: ["nope"], statement: "SET this.x = 1")`,
        ),
      ),
    ).toEqual(["@trigger: T has no field nope"]);
  });

  test("an operation the type has no mutation for", () => {
    expect(
      problems(
        () =>
          new LoraGraphQL({
            typeDefs: `type T @node @mutation(operations: [CREATE]) @trigger(on: [DELETE], statement: "SET this.x = 1") { id: ID! @key }`,
            driver: undefined as never,
          }),
      ),
    ).toEqual([
      "@trigger: on lists DELETE, but T has no generated DELETE mutation (@mutation(operations:)) to fire it",
    ]);
  });

  test("on a type without @node", () => {
    expect(
      problems(
        () =>
          new LoraGraphQL({
            typeDefs: `
              type T @node { id: ID! @key }
              type Shape @trigger(on: [CREATE], statement: "RETURN 1") { n: Int }
              type Query { shape: Shape @cypher(statement: "RETURN {n: 1} AS s") }
            `,
            driver: undefined as never,
          }),
      ).length,
    ).toBeGreaterThan(0);
  });
});

describe("check()", () => {
  const instance = async (statement: string) => {
    const db = await createDatabase();
    const lora = new LoraGraphQL({
      driver: loraDriver(db),
      typeDefs: `type A @node @mutation @trigger(on: [CREATE], statement: ${JSON.stringify(statement)}) { id: ID! @key }`,
    });
    await lora.assertSchema({ create: true });
    return lora;
  };

  test("plans a trigger statement and reports one that does not parse", async () => {
    const good = await (
      await instance("SET this.seen = true RETURN this AS updated")
    ).check();
    expect(good.cypher).toEqual([]);
    const bad = await (await instance("SET this.seen = RETURN")).check();
    expect(bad.ok).toBe(false);
    expect(bad.cypher).toHaveLength(1);
    expect(bad.cypher[0]).toMatchObject({ type: "A", field: "@trigger 1" });
  });

  test("lints a statement that writes without reporting", async () => {
    const report = await (
      await instance("CREATE (:Audit {of: this.id})")
    ).check();
    expect(report.lint.filter((l) => l.field === "@trigger 1")).toHaveLength(1);
    const quiet = await (
      await instance("SET this.seen = true RETURN this AS updated")
    ).check();
    expect(quiet.lint.filter((l) => l.field === "@trigger 1")).toEqual([]);
  });
});

describe("trigger functions", () => {
  const sdl = /* GraphQL */ `
    type Order @node @mutation {
      id: ID! @key
      total: Int!
      status: String
      lines: [Line!]! @relationship(type: "HAS", direction: OUT)
    }
    type Line @node @mutation {
      id: ID! @key
    }
    type Audit @node @mutation {
      id: ID! @key
      of: ID!
    }
  `;
  const build = async (
    triggers: NonNullable<
      ConstructorParameters<typeof LoraGraphQL>[0]["triggers"]
    >,
    triggerTimeoutMs?: number,
  ) => {
    const db = await createDatabase();
    const lora: LoraGraphQL = new LoraGraphQL({
      typeDefs: sdl,
      driver: loraDriver(db),
      triggers,
      ...(triggerTimeoutMs !== undefined ? { triggerTimeoutMs } : {}),
    });
    await lora.assertSchema({ create: true });
    const changes: WriteChange[] = [];
    lora.onWrite((c) => changes.push(c));
    return { lora, changes };
  };
  const ORDER = (id: string, total: number) =>
    `mutation { createOrders(input: [{ id: "${id}", total: ${total} }]) { orders { id status } } }`;

  test("write through the transaction, and the payload shows it", async () => {
    const { lora } = await build({
      Order: {
        create: async ({ keys, transaction, operation, type }) => {
          expect([operation, type]).toEqual(["CREATE", "Order"]);
          await transaction.execute(
            "UNWIND $keys AS k MATCH (o:Order {id: k}) SET o.status = 'NEW'",
            { keys },
          );
        },
      },
    });
    const r = await lora.execute({ source: ORDER("o1", 5) });
    expect(r.data).toEqual({
      createOrders: { orders: [{ id: "o1", status: "NEW" }] },
    });
  });

  test("a mutation run in the transaction joins the write-set and fires nothing", async () => {
    let auditCalls = 0;
    const holder: { lora?: LoraGraphQL } = {};
    const { lora, changes } = await build({
      Order: {
        create: async ({ keys, transaction, context }) => {
          const r = await holder.lora!.execute({
            source: `mutation ($input: [AuditCreateInput!]!) { createAudits(input: $input) { audits { id } } }`,
            variables: {
              input: keys.map((k) => ({ id: `a-${String(k)}`, of: k })),
            },
            context: { ...(context as object), transaction },
          });
          if (r.errors) throw r.errors[0];
        },
      },
      Audit: { create: () => void auditCalls++ },
    });
    holder.lora = lora;
    const r = await lora.execute({ source: ORDER("o1", 5) });
    expect(r.errors).toBeUndefined();
    expect(auditCalls).toBe(0);
    expect(changes).toHaveLength(1);
    expect(changes[0]!.created).toEqual([
      { type: "Order", key: "o1" },
      { type: "Audit", key: "a-o1" },
    ]);
    const read = await lora.execute({ source: `{ audits { id of } }` });
    expect(read.data).toEqual({ audits: [{ id: "a-o1", of: "o1" }] });
  });

  test("throwing a GraphQLError vetoes the write with that error", async () => {
    const { lora, changes } = await build({
      Order: {
        create: ({ change }) => {
          expect(change.created).toEqual([{ type: "Order", key: "o1" }]);
          throw new GraphQLError("orders are closed", {
            extensions: { code: "FORBIDDEN" },
          });
        },
      },
    });
    const r = await lora.execute({ source: ORDER("o1", 5) });
    expect(r.errors?.[0]?.message).toBe("orders are closed");
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    expect(changes).toEqual([]);
    const read = await lora.execute({ source: `{ orders { id } }` });
    expect(read.data).toEqual({ orders: [] });
  });

  test("any other error rolls back too", async () => {
    const { lora } = await build({
      Order: {
        create: () => {
          throw new Error("boom");
        },
      },
    });
    const r = await lora.execute({ source: ORDER("o1", 5) });
    expect(r.errors).toHaveLength(1);
    const read = await lora.execute({ source: `{ orders { id } }` });
    expect(read.data).toEqual({ orders: [] });
  });

  test("a trigger past triggerTimeoutMs fails with TIMEOUT", async () => {
    const { lora } = await build(
      {
        Order: {
          create: () => new Promise((resolve) => setTimeout(resolve, 200)),
        },
      },
      20,
    );
    const r = await lora.execute({ source: ORDER("o1", 5) });
    expect(r.errors?.[0]?.extensions).toMatchObject({
      code: "TIMEOUT",
      triggerTimeoutMs: 20,
    });
    const read = await lora.execute({ source: `{ orders { id } }` });
    expect(read.data).toEqual({ orders: [] });
  });

  test("update and delete functions get the keys written", async () => {
    const seen: Array<[string, unknown[]]> = [];
    const { lora } = await build({
      Order: {
        update: ({ operation, keys }) => void seen.push([operation, keys]),
        delete: async ({ operation, keys, transaction }) => {
          // The nodes still exist.
          const { rows } = await transaction.execute(
            "UNWIND $keys AS k MATCH (o:Order {id: k}) RETURN count(o) AS n",
            { keys },
          );
          seen.push([operation, [Number(rows[0]!["n"])]]);
        },
      },
    });
    await lora.execute({ source: ORDER("o1", 5) });
    await lora.execute({
      source: `mutation { updateOrder(id: "o1", update: { total: 6 }) { order { id } } }`,
    });
    await lora.execute({
      source: `mutation { deleteOrder(id: "o1") { nodesDeleted } }`,
    });
    expect(seen).toEqual([
      ["UPDATE", ["o1"]],
      ["DELETE", [1]],
    ]);
  });

  test("a trigger cannot commit or roll back its transaction", async () => {
    let message = "";
    const { lora } = await build({
      Order: {
        create: async ({ transaction }) => {
          await transaction.commit().catch((err: Error) => {
            message = err.message;
          });
        },
      },
    });
    const r = await lora.execute({ source: ORDER("o1", 5) });
    expect(r.errors).toBeUndefined();
    expect(message).toContain("a trigger cannot commit its transaction");
  });

  test("startup refuses an unknown type, operation or mutation", () => {
    const refuse = (triggers: Record<string, Record<string, unknown>>) => {
      try {
        new LoraGraphQL({
          typeDefs: `type T @node @mutation(operations: [CREATE]) { id: ID! @key }`,
          driver: undefined as never,
          triggers: triggers as never,
        });
      } catch (err) {
        return (err as ModelError).problems.map((p) => p.message);
      }
      return [];
    };
    expect(refuse({ Nope: { create: () => 1 } })).toEqual([
      "triggers.Nope: no @node type Nope",
    ]);
    expect(refuse({ T: { upsert: () => 1 } })).toEqual([
      "triggers.T.upsert: not an operation (create, update, delete)",
    ]);
    expect(refuse({ T: { delete: () => 1 } })).toEqual([
      "triggers.T.delete: T has no generated DELETE mutation (@mutation(operations:)) to fire it",
    ]);
  });
});
