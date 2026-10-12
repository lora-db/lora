// Defects an independent review found in the first cut of triggers, the
// write queue and batching, each held here by the scenario that showed it.

import { createDatabase } from "@loradb/lora-node";
import { describe, expect, test } from "vitest";
import {
  LoraGraphQL,
  ModelError,
  loraDriver,
  type LoraDriver,
  type QueryResult,
  type RunOptions,
  type Statement,
  type WriteChange,
} from "../src/index.js";

type TriggerOption = NonNullable<
  ConstructorParameters<typeof LoraGraphQL>[0]["triggers"]
>;

async function build(typeDefs: string, triggers?: TriggerOption) {
  const db = await createDatabase();
  const lora: LoraGraphQL = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    ...(triggers ? { triggers } : {}),
  });
  await lora.assertSchema({ create: true });
  const changes: WriteChange[] = [];
  lora.onWrite((c) => changes.push(c));
  const exec = (source: string, jwt?: Record<string, unknown>) =>
    lora.execute({ source, context: jwt ? { jwt } : {} });
  return { db, lora, exec, changes };
}

describe("a denied write fails with its own error, whatever a trigger would hit", () => {
  const sdl = /* GraphQL */ `
    extend schema @authorizationDefaults(requireAuthentication: false)
    type Doc
      @node
      @mutation
      @authorization(
        validate: [
          {
            operations: [CREATE, UPDATE]
            where: { jwt: { role: { eq: "writer" } } }
          }
        ]
      )
      @trigger(
        on: [CREATE, UPDATE]
        statement: "CREATE (a:Audit {id: 'a-' + this.id}) RETURN a AS created"
      ) {
      id: ID! @key
      title: String
    }
    type Audit @node {
      id: ID! @key
    }
  `;

  test("create: FORBIDDEN whether or not the trigger's node exists", async () => {
    const calls: string[] = [];
    const { db, exec } = await build(sdl, {
      Doc: { create: ({ keys }) => void calls.push(String(keys[0])) },
    });
    await db.execute("CREATE (:Audit {id: 'a-taken'})");
    const codes = [];
    for (const id of ["free", "taken"]) {
      const r = await exec(
        `mutation { createDocs(input: [{ id: "${id}" }]) { docs { id } } }`,
        { role: "reader" },
      );
      codes.push(r.errors?.[0]?.extensions?.["code"]);
    }
    expect(codes).toEqual(["FORBIDDEN", "FORBIDDEN"]);
    // And no trigger function ran for a write that was never allowed.
    expect(calls).toEqual([]);
  });

  test("update: the same", async () => {
    const { db, exec } = await build(sdl);
    await db.execute(
      "CREATE (:Doc {id: 'free'}), (:Doc {id: 'taken'}), (:Audit {id: 'a-taken'})",
    );
    const codes = [];
    for (const id of ["free", "taken"]) {
      const r = await exec(
        `mutation { updateDoc(id: "${id}", update: { title: "x" }) { doc { id } } }`,
        { role: "reader" },
      );
      codes.push(r.errors?.[0]?.extensions?.["code"]);
    }
    expect(codes).toEqual(["FORBIDDEN", "FORBIDDEN"]);
  });

  test("a trigger function is not called for a create the key refuses", async () => {
    const calls: unknown[] = [];
    const { exec } = await build(`type T @node @mutation { id: ID! @key }`, {
      T: { create: ({ keys }) => void calls.push(keys) },
    });
    await exec(`mutation { createTs(input: [{ id: "d1" }]) { ts { id } } }`);
    calls.length = 0;
    const r = await exec(
      `mutation { createTs(input: [{ id: "d1" }]) { ts { id } } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(calls).toEqual([]);
  });
});

describe("what a DELETE trigger writes fires no trigger", () => {
  test("neither statements nor functions, also for a nested delete", async () => {
    const fired: string[] = [];
    const { db, exec } = await build(
      /* GraphQL */ `
        type Post
          @node
          @mutation
          @trigger(
            on: [UPDATE]
            statement: "SET this.touched = true RETURN this AS updated"
          ) {
          id: ID! @key
          n: Int
          touched: Boolean
          comments: [Comment!]! @relationship(type: "ON", direction: IN)
        }
        type Comment
          @node
          @mutation
          @trigger(
            on: [DELETE]
            statement: """
            MATCH (this)-[:ON]->(p:Post)
            SET p.n = coalesce(p.n, 0) + 1
            CREATE (a:Audit {id: 'a-' + this.id})
            RETURN p AS updated, a AS created
            """
          ) {
          id: ID! @key
          post: Post @relationship(type: "ON", direction: OUT)
        }
        type Audit
          @node
          @mutation
          @trigger(
            on: [CREATE]
            statement: "SET this.fired = true RETURN this AS updated"
          ) {
          id: ID! @key
          fired: Boolean
        }
      `,
      {
        Audit: { create: () => void fired.push("Audit.create") },
        Post: { update: () => void fired.push("Post.update") },
      },
    );
    await db.execute(
      "CREATE (p:Post {id: 'p'}), (:Comment {id: 'c1'})-[:ON]->(p), (:Comment {id: 'c2'})-[:ON]->(p)",
    );
    const one = await exec(
      `mutation { deleteComment(id: "c1") { nodesDeleted } }`,
    );
    expect(one.errors).toBeUndefined();
    const nested = await exec(
      `mutation { updatePost(id: "p", update: { comments: { delete: { limit: 5 } } }) { post { n } } }`,
    );
    expect(nested.errors).toBeUndefined();
    // The nested delete's own update of Post is the mutation's, so Post's
    // UPDATE trigger fires once, for that; nothing fires for the Audits.
    expect(fired).toEqual(["Post.update"]);
    const read = await exec(`{ audits { id fired } posts { n } }`);
    expect(read.data).toEqual({
      audits: [
        { id: "a-c1", fired: null },
        { id: "a-c2", fired: null },
      ],
      posts: [{ n: 2 }],
    });
  });
});

describe("the write-set", () => {
  test("a node a trigger function deleted is reported deleted, not updated", async () => {
    const holder: { lora?: LoraGraphQL } = {};
    const { lora, exec, changes } = await build(
      `type Task @node @mutation { id: ID! @key  done: Boolean }`,
      {
        Task: {
          update: async ({ keys, transaction, context }) => {
            const r = await holder.lora!.execute({
              source: `mutation ($id: ID!) { deleteTask(id: $id) { nodesDeleted } }`,
              variables: { id: keys[0] },
              context: { ...(context as object), transaction },
            });
            if (r.errors) throw r.errors[0];
          },
        },
      },
    );
    holder.lora = lora;
    await exec(
      `mutation { createTasks(input: [{ id: "t1" }]) { tasks { id } } }`,
    );
    changes.length = 0;
    const r = await exec(
      `mutation { updateTask(id: "t1", update: { done: true }) { info { nodesUpdated } } }`,
    );
    expect(r.errors).toBeUndefined();
    expect(changes[0]!.updated).toEqual([]);
    expect(changes[0]!.deleted).toEqual([{ type: "Task", key: "t1" }]);
  });

  test("a reporting statement that matches no row does not make the change broad", async () => {
    const { exec, changes } = await build(/* GraphQL */ `
      type Child
        @node
        @mutation
        @trigger(
          on: [CREATE]
          statement: "MATCH (p:Parent {id: this.parent}) SET p.n = 1 RETURN p AS updated"
        ) {
        id: ID! @key
        parent: ID
      }
      type Parent @node {
        id: ID! @key
        n: Int
      }
    `);
    await exec(
      `mutation { createChilds(input: [{ id: "c", parent: "nobody" }]) { childs { id } } }`,
    );
    expect(changes).toHaveLength(1);
    expect(changes[0]!.broad).toBe(false);
  });
});

describe("the triggers option", () => {
  test("an operation key must be spelled as it is looked up", () => {
    const problems = (() => {
      try {
        new LoraGraphQL({
          typeDefs: `type T @node @mutation { id: ID! @key }`,
          driver: undefined as never,
          triggers: { T: { Create: () => 1 } } as never,
        });
      } catch (err) {
        return (err as ModelError).problems.map((p) => p.message);
      }
      return [];
    })();
    expect(problems).toEqual([
      "triggers.T.Create: not an operation (create, update, delete)",
    ]);
  });

  test("a trigger that swallows a failed statement does not let the mutation report success", async () => {
    const { lora, exec } = await build(
      `type T @node @mutation { id: ID! @key }`,
      {
        T: {
          create: async ({ transaction }) => {
            await transaction
              .execute("THIS IS NOT CYPHER")
              .catch(() => undefined);
          },
        },
      },
    );
    const own = await exec(
      `mutation { createTs(input: [{ id: "a" }]) { info { nodesCreated } } }`,
    );
    expect(own.errors).toHaveLength(1);
    // In a caller-owned transaction nothing else would have said so until
    // the caller's commit.
    const tx = await lora.begin();
    const r = await lora.execute({
      source: `mutation { createTs(input: [{ id: "b" }]) { info { nodesCreated } } }`,
      context: { transaction: tx },
    });
    expect(r.errors).toHaveLength(1);
    expect(tx.isOpen).toBe(false);
    const read = await exec(`{ ts { id } }`);
    expect(read.data).toEqual({ ts: [] });
  });
});

describe("a driver that is a class instance", () => {
  test("keeps its methods behind the write queue", async () => {
    const db = await createDatabase();
    const inner = loraDriver(db);
    class Recording implements LoraDriver {
      readonly #inner = inner;
      calls: string[] = [];
      run(
        statements: Statement[],
        options: RunOptions,
      ): Promise<QueryResult[]> {
        this.calls.push("run");
        return this.#inner.run(statements, options);
      }
      begin(options: RunOptions) {
        this.calls.push("begin");
        return this.#inner.begin!(options);
      }
      explain(statement: Statement) {
        this.calls.push("explain");
        return this.#inner.explain!(statement);
      }
    }
    const driver = new Recording();
    const lora = new LoraGraphQL({
      typeDefs: `type T @node @mutation { id: ID! @key }`,
      driver,
    });
    await lora.assertSchema({ create: true });
    const created = await lora.execute({
      source: `mutation { createTs(input: [{ id: "a" }]) { ts { id } } }`,
    });
    expect(created.errors).toBeUndefined();
    const read = await lora.execute({ source: `{ ts { id } }` });
    expect(read.data).toEqual({ ts: [{ id: "a" }] });
    expect((await lora.check()).cypher).toEqual([]);
    // A delete runs in a transaction: begin() is the class's own too.
    const deleted = await lora.execute({
      source: `mutation { deleteT(id: "a") { nodesDeleted } }`,
    });
    expect(deleted.errors).toBeUndefined();
    expect(driver.calls).toContain("begin");
    expect(driver.calls).toContain("run");
  });

  test("a failing statement in the middle of a batch, without executeMany", async () => {
    const db = await createDatabase();
    const base = loraDriver(db);
    const driver: LoraDriver = {
      ...base,
      begin: async (options) => {
        const tx = await base.begin!(options);
        return {
          execute: (s: Statement) => tx.execute(s),
          commit: () => tx.commit(),
          rollback: () => tx.rollback(),
          get isOpen() {
            return tx.isOpen;
          },
        };
      },
    };
    const lora = new LoraGraphQL({
      typeDefs: `type T @node @mutation { id: ID! @key  name: String @unique }`,
      driver,
    });
    await lora.assertSchema({ create: true });
    const run = (source: string) => lora.execute({ source });
    await run(
      `mutation { createTs(input: [{ id: "a", name: "n" }]) { ts { id } } }`,
    );
    const r = await run(
      `mutation { createTs(input: [{ id: "b", name: "n" }]) { ts { id } } }`,
    );
    expect(r.errors?.[0]?.extensions?.["code"]).toBe("CONSTRAINT_VIOLATION");
    expect((await run(`{ ts { id } }`)).data).toEqual({ ts: [{ id: "a" }] });
  });
});

describe("second review", () => {
  test("no trigger runs for a create that a hidden key refuses", async () => {
    const calls: unknown[] = [];
    const { db, exec } = await build(
      /* GraphQL */ `
        type Num
          @node
          @mutation
          @authorization(
            filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
          )
          @trigger(
            on: [CREATE]
            statement: "CREATE (a:Seen {id: toString(this.id)}) RETURN a AS created"
          ) {
          id: Int! @key
          owner: String
        }
        type Seen @node {
          id: ID! @key
        }
      `,
      { Num: { create: ({ keys }) => void calls.push(keys) } },
    );
    await db.execute("CREATE (:Num {id: 7, owner: 'someone-else'})");
    const hidden = await exec(
      `mutation { createNums(input: [{ id: 7, owner: "m" }]) { nums { id } } }`,
      { sub: "m" },
    );
    expect(hidden.errors?.[0]?.extensions?.["code"]).toBe("FORBIDDEN");
    expect(calls).toEqual([]);
    const free = await exec(
      `mutation { createNums(input: [{ id: 8, owner: "m" }]) { nums { id } } }`,
      { sub: "m" },
    );
    expect(free.errors).toBeUndefined();
    expect(calls).toEqual([[8]]);
    const seen = await db.execute("MATCH (s:Seen) RETURN s.id AS id");
    expect(seen.rows.map((r) => r["id"])).toEqual(["8"]);
  });

  test.each([
    [
      "an alias in an earlier WITH is not a report",
      "MATCH (o:Other) WITH o, 1 AS updated SET o.n = updated",
    ],
    [
      "a column in another case is not a report",
      "MATCH (o:Other) SET o.n = 1 RETURN o AS Updated",
    ],
    [
      "a value that is not a node is not a report",
      "MATCH (o:Other) SET o.n = 1 RETURN o.id AS updated",
    ],
  ])("%s: the change is broad", async (_name, statement) => {
    const { db, exec, changes } = await build(`
      type T @node @mutation @trigger(on: [CREATE], statement: ${JSON.stringify(statement)}) { id: ID! @key }
      type Other @node { id: ID! @key  n: Int }
    `);
    await db.execute("CREATE (:Other {id: 'o'})");
    const r = await exec(
      `mutation { createTs(input: [{ id: "t" }]) { ts { id } } }`,
    );
    expect(r.errors).toBeUndefined();
    expect(changes[0]!.broad).toBe(true);
  });

  test("a quoted column name reports, and a longer name does not", async () => {
    const run = async (statement: string) => {
      const { db, exec, changes } = await build(`
        type T @node @mutation @trigger(on: [CREATE], statement: ${JSON.stringify(statement)}) { id: ID! @key }
        type Other @node { id: ID! @key  n: Int }
      `);
      await db.execute("CREATE (:Other {id: 'o'})");
      await exec(`mutation { createTs(input: [{ id: "t" }]) { ts { id } } }`);
      return changes[0]!;
    };
    const quoted = await run(
      "MATCH (o:Other) SET o.n = 1 RETURN o AS `updated`",
    );
    expect(quoted.broad).toBe(false);
    expect(quoted.updated).toEqual([{ type: "Other", key: "o" }]);
    const longer = await run(
      "MATCH (o:Other) SET o.n = 1 RETURN o AS updatedAt",
    );
    expect(longer.broad).toBe(true);
  });
});
