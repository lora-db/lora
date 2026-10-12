// A create without relationship writes is one write batch that commits as
// it returns: no begin(), no hop to JavaScript while the writer lock is
// held. Its rule checks are assert() statements of the batch. Everything
// else keeps a transaction.

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
    const waiting = exec(CREATE("a"));
    const refused = await exec(CREATE("b"));
    expect(refused.errors?.[0]?.extensions?.["code"]).toBe("OVERLOADED");
    // The one that waits gives up at timeoutMs.
    const timedOut = await waiting;
    expect(timedOut.errors).toHaveLength(1);
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

  test("a create with a relationship write", async () => {
    const paths = await usesBegin(
      `type Post @node @mutation { key: String! @key  tags: [Tag!]! @relationship(type: "HAS", direction: OUT) }
       type Tag @node @mutation { key: String! @key }`,
      `mutation { createPosts(input: [{ key: "p", tags: { create: [{ node: { key: "t" } }] } }]) { posts { key } } }`,
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

  test("a type with @uniqueTogether", async () => {
    const paths = await usesBegin(
      `type Item @node @mutation @uniqueTogether(fields: ["name", "kind"]) { key: String! @key  name: String  kind: String }`,
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

  test("a caller-owned transaction, an update and a delete", async () => {
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
    await exec(
      `mutation { updateItem(key: "a", update: { name: "b" }) { item { key } } }`,
    );
    await exec(`mutation { deleteItem(key: "a") { nodesDeleted } }`);
    expect(paths).toEqual(["begin", "begin"]);
  });
});
