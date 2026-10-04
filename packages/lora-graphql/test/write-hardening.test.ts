// Write-side hardening: @cypher mutations in shared transactions, batch
// limits, hidden nodes in write errors, mutation defaults per operation,
// and atomic operations on the getSchema() path.

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL } from "../src/testing.js";

type Result = {
  data?: unknown;
  errors?: ReadonlyArray<{ message: string; extensions?: unknown }>;
};
const codes = (r: Result) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );

/** Fails fast instead of hanging when a write waits on the writer lock. */
function within<T>(ms: number, promise: Promise<T>): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`still waiting after ${ms} ms`)), ms),
    ),
  ]);
}

const rows = async (
  t: Awaited<ReturnType<typeof createTestLoraGraphQL>>,
  query: string,
) =>
  (
    (await t.db.execute(query)) as {
      rows: Array<Record<string, unknown>>;
    }
  ).rows;

describe("@cypher mutations in a shared transaction", () => {
  const typeDefs = `type C @node @mutation { key: String! @key }
    type Mutation {
      makeC(key: String!): C
        @cypher(statement: "CREATE (c:C {key: $key}) RETURN c", columnName: "c")
    }`;
  const keys = async (t: Awaited<ReturnType<typeof createTestLoraGraphQL>>) =>
    (await rows(t, "MATCH (c:C) RETURN c.key AS k ORDER BY k")).map(
      (r) => r["k"],
    );

  test('runs in the operation\'s transaction with mutationTransaction: "operation"', async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      mutationTransaction: "operation",
    });
    const changes: Array<{ operation: string }> = [];
    t.lora.onWrite((c) => changes.push(c));
    const r = await within(
      3000,
      t.lora.execute({
        source: `mutation {
          a: createCs(input: [{ key: "c1" }]) { cs { key } }
          b: makeC(key: "c2") { key } }`,
      }),
    );
    expect(r.errors).toBeUndefined();
    expect(r.data).toEqual({ a: { cs: [{ key: "c1" }] }, b: { key: "c2" } });
    expect(await keys(t)).toEqual(["c1", "c2"]);
    expect(changes.map((c) => c.operation)).toEqual(["CREATE", "CYPHER"]);
    // Later writes still go through.
    const after = await within(
      3000,
      t.lora.execute({ source: `mutation { makeC(key: "c3") { key } }` }),
    );
    expect(after.errors).toBeUndefined();
  });

  test("a failing root field rolls the @cypher write back too", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs,
      mutationTransaction: "operation",
    });
    const changes: unknown[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const r = await within(
      3000,
      t.lora.execute({
        source: `mutation {
          b: makeC(key: "c2") { key }
          a: createCs(input: [{ key: "c2" }]) { cs { key } } }`,
      }),
    );
    expect(codes(r)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(await keys(t)).toEqual([]);
    expect(changes).toEqual([]);
  });

  test("runs in a caller-owned transaction; the event waits for the commit", async () => {
    const t = await createTestLoraGraphQL({ typeDefs });
    const changes: unknown[] = [];
    t.lora.onWrite((c) => changes.push(c));
    const tx = await t.lora.begin();
    const r = await within(
      3000,
      t.lora.execute({
        source: `mutation { makeC(key: "c1") { key } }`,
        context: { transaction: tx },
      }),
    );
    expect(r.errors).toBeUndefined();
    expect(changes).toEqual([]);
    await tx.commit();
    expect(changes).toHaveLength(1);
    expect(await keys(t)).toEqual(["c1"]);
  });

  test("waiting for the writer lock honours timeoutMs", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, timeoutMs: 200 });
    const tx = await t.lora.begin();
    try {
      const r = await within(
        3000,
        t.lora.execute({ source: `mutation { makeC(key: "c1") { key } }` }),
      );
      expect(codes(r)).toEqual(["DATABASE_ERROR"]);
      const generated = await within(
        3000,
        t.lora.execute({
          source: `mutation { createCs(input: [{ key: "c2" }]) { cs { key } } }`,
        }),
      );
      expect(codes(generated)).toEqual(["DATABASE_ERROR"]);
    } finally {
      await tx.rollback();
    }
    // The lock is free again, and the late transactions were released.
    const r = await within(
      3000,
      t.lora.execute({ source: `mutation { makeC(key: "c3") { key } }` }),
    );
    expect(r.errors).toBeUndefined();
    expect(await keys(t)).toEqual(["c3"]);
  });
});

describe("@uniqueTogether under @cypher mutations", () => {
  const typeDefs = `type Person @node { key: String! @key }
    type Pair @node @mutation @uniqueTogether(fields: ["a", "b"]) {
      key: String! @key  a: String  b: String
    }
    type Follow @node @uniqueTogether(fields: ["from", "to"]) {
      key: String! @key
      from: Person! @relationship(type: "FROM", direction: OUT)
      to: Person! @relationship(type: "TO", direction: OUT)
    }
    type Mutation {
      makePair(key: String!, a: String!, b: String!): Pair
        @cypher(statement: "CREATE (p:Pair {key: $key, a: $a, b: $b}) RETURN p", columnName: "p")
      setB(key: String!, b: String!): Pair
        @cypher(statement: "MATCH (p) WHERE p.key = $key SET p.b = $b RETURN p", columnName: "p")
      follow(key: String!, from: String!, to: String!): Follow
        @cypher(statement: """
          MATCH (x {key: $from}), (y {key: $to})
          CREATE (f:Follow {key: $key}), (f)-[:FROM]->(x), (f)-[:TO]->(y)
          RETURN f
        """, columnName: "f")
    }`;
  const seed = "CREATE (:Person {key: 'lou'}), (:Person {key: 'bo'})";

  test("a combination a @cypher mutation duplicates is refused and rolled back", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    expect(
      (await t.run(`mutation { makePair(key: "p1", a: "x", b: "y") { key } }`))
        .errors,
    ).toBeUndefined();
    const dup = await t.run(
      `mutation { makePair(key: "p2", a: "x", b: "y") { key } }`,
    );
    expect(codes(dup)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(dup.errors?.[0]?.message).toMatch(/Pair must be unique by \(a, b\)/);
    expect(await rows(t, "MATCH (p:Pair) RETURN p.key AS k")).toEqual([
      { k: "p1" },
    ]);
    // A write that names no label still reaches the constrained property.
    await t.data(`mutation { makePair(key: "p3", a: "x", b: "z") { key } }`);
    const set = await t.run(`mutation { setB(key: "p3", b: "y") { key } }`);
    expect(codes(set)).toEqual(["CONSTRAINT_VIOLATION"]);
  });

  test("relationship ends written by a @cypher mutation are checked", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    await t.data(
      `mutation { follow(key: "f1", from: "lou", to: "bo") { key } }`,
    );
    const dup = await t.run(
      `mutation { follow(key: "f2", from: "lou", to: "bo") { key } }`,
    );
    expect(codes(dup)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(
      (
        await t.run(
          `mutation { follow(key: "f3", from: "bo", to: "lou") { key } }`,
        )
      ).errors,
    ).toBeUndefined();
  });

  test("the model warns that such a mutation checks every node of the type", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const warnings = t.lora.model.warnings.filter((w) =>
      w.message.includes("@uniqueTogether"),
    );
    expect(warnings.map((w) => `${w.type}.${w.field}`).sort()).toEqual([
      "Mutation.follow",
      "Mutation.makePair",
      "Mutation.setB",
    ]);
  });
});

describe("maxBatch bounds every write of a mutation", () => {
  const typeDefs = `type F @node @mutation {
      key: String! @key  name: String
      stages: [S!]! @relationship(type: "HAS", direction: OUT)
    }
    type S @node @mutation { key: String! @key  size: Int }`;
  const seed = [
    "UNWIND range(1, 50) AS i CREATE (:F {key: 'f' + toString(i)})",
    "CREATE (f:F {key: 'hub'}) WITH f UNWIND range(1, 15) AS i CREATE (f)-[:HAS]->(:S {key: 's' + toString(i)})",
  ];
  const setup = () => createTestLoraGraphQL({ typeDefs, seed, maxBatch: 2 });

  test("an upsert counts the inputs that update, not only those that create", async () => {
    const t = await setup();
    const input = Array.from(
      { length: 50 },
      (_, i) => `{ key: "f${i + 1}", name: "n" }`,
    ).join(" ");
    t.statements.length = 0;
    const r = await t.run(
      `mutation { upsertFs(input: [${input}]) { info { nodesUpdated } } }`,
    );
    expect(codes(r)).toEqual(["LIMIT_EXCEEDED"]);
    // Refused before any write.
    expect(t.statements.some((s) => /\bSET\b/.test(s.statement.text))).toBe(
      false,
    );
    expect(
      await rows(t, "MATCH (f:F) WHERE f.name IS NOT NULL RETURN f"),
    ).toEqual([]);
  });

  test("nested update entries count as nodes", async () => {
    const t = await setup();
    const update = Array.from(
      { length: 15 },
      (_, i) => `{ key: "s${i + 1}", node: { size: 1 } }`,
    ).join(" ");
    t.statements.length = 0;
    const r = await t.run(
      `mutation { updateF(key: "hub", update: { stages: { update: [${update}] } }) { info { nodesUpdated } } }`,
    );
    expect(codes(r)).toEqual(["LIMIT_EXCEEDED"]);
    expect(t.statements.length).toBeLessThan(10);
  });

  test("disconnect lists count as relationships", async () => {
    const t = await setup();
    const keys = Array.from({ length: 100_000 }, (_, i) => `s${i}`);
    const r = await t.run(
      `mutation($keys: [String!]!) { updateF(key: "hub", update: { stages: { disconnect: $keys } }) { info { relationshipsDeleted } } }`,
      { keys },
    );
    expect(codes(r)).toEqual(["LIMIT_EXCEEDED"]);
    expect(
      await rows(t, "MATCH (:F {key: 'hub'})-[r:HAS]->() RETURN count(r) AS n"),
    ).toEqual([{ n: 15 }]);
  });

  test("writes within the limit still go through", async () => {
    const t = await setup();
    const r = await t.run(`mutation {
      upsertFs(input: [{ key: "f1", name: "a" }, { key: "new" }]) { info { nodesCreated nodesUpdated } } }`);
    expect(r.errors).toBeUndefined();
    const u = await t.run(
      `mutation { updateF(key: "hub", update: { stages: { update: [{ key: "s1", node: { size: 3 } }], disconnect: ["s2", "s3"] } }) { info { nodesUpdated relationshipsDeleted } } }`,
    );
    expect(u.errors).toBeUndefined();
  });
});

describe("write errors do not reveal nodes the caller cannot read", () => {
  const typeDefs = `type Claims @jwt { sub: String }
    type User @node @mutation { key: String! @key }
    type Secret @node @mutation
      @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
      key: String! @key
      owner: String!
      holder: User! @relationship(type: "HOLDS", direction: IN)
    }
    type Org @node @mutation {
      key: String! @key
      docs: [Doc!]! @relationship(type: "OWNS", direction: OUT, onDelete: RESTRICT)
    }
    type Doc @node @mutation
      @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
      key: String! @key
      owner: String!
    }
    type F @node @mutation {
      key: String! @key
      genre: G @relationship(type: "IS", direction: OUT)
    }
    type G @node @mutation
      @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
      key: String! @key
      owner: String!
    }
    type Note @node @mutation
      @authorization(
        filter: [{ operations: [READ], where: { node: { owner: { eq: "$jwt.sub" } } } }]
        validate: [{ operations: [UPDATE, DELETE], where: { node: { owner: { eq: "$jwt.sub" } } } }]
      ) {
      key: String! @key
      owner: String!
      text: String
    }`;
  const seed = [
    "CREATE (bo:User {key: 'bo'})-[:HOLDS]->(:Secret {key: 'bo-secret-plan', owner: 'bo'})",
    "CREATE (:Org {key: 'o'})-[:OWNS]->(:Doc {key: 'd', owner: 'bo'})",
    "CREATE (:F {key: 'f'})-[:IS]->(:G {key: 'hidden', owner: 'bo'}), (:G {key: 'jazz', owner: 'lou'})",
    "CREATE (:Note {key: 'n', owner: 'bo', text: 'x'})",
  ];
  const lou = { jwt: { sub: "lou" } };
  const bo = { jwt: { sub: "bo" } };

  test("a required relationship names a hidden node's key only to those who can read it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const hidden = await t.run(
      `mutation { deleteUser(key: "bo") { nodesDeleted } }`,
      {},
      lou,
    );
    expect(codes(hidden)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(hidden.errors?.[0]?.message).toBe(
      "a Secret the caller can't read requires a User (Secret.holder)",
    );
    const seen = await t.run(
      `mutation { deleteUser(key: "bo") { nodesDeleted } }`,
      {},
      bo,
    );
    expect(seen.errors?.[0]?.message).toBe(
      'Secret "bo-secret-plan" requires a User (Secret.holder)',
    );
  });

  test("RESTRICT blocked by a hidden node gives a generic message", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const hidden = await t.run(
      `mutation { deleteOrg(key: "o") { nodesDeleted } }`,
      {},
      lou,
    );
    expect(codes(hidden)).toEqual(["CONSTRAINT_VIOLATION"]);
    expect(hidden.errors?.[0]?.message).toBe(
      'Org "o" cannot be deleted: Org.docs has onDelete: RESTRICT',
    );
    const seen = await t.run(
      `mutation { deleteOrg(key: "o") { nodesDeleted } }`,
      {},
      bo,
    );
    expect(seen.errors?.[0]?.message).toBe(
      'Org "o" still has docs (onDelete: RESTRICT); remove them first',
    );
  });

  test("replacing a single relationship whose current target is hidden is refused generically", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      `mutation { updateF(key: "f", update: { genre: { connect: { key: "jazz" } } }) { f { key } } }`,
      {},
      lou,
    );
    expect(codes(r)).toEqual(["FORBIDDEN"]);
    expect(r.errors?.[0]?.message).toBe("not allowed to replace F.genre");
    expect(
      await rows(t, "MATCH (:F {key: 'f'})-[:IS]->(g) RETURN g.key AS k"),
    ).toEqual([{ k: "hidden" }]);
  });

  test("update and delete treat a hidden key as a missing one", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    for (const key of ["n", "missing"]) {
      const u = await t.run(
        `mutation($key: String!) { updateNote(key: $key, update: { text: "y" }) { note { key } } }`,
        { key },
        lou,
      );
      expect(u.errors).toBeUndefined();
      expect(u.data).toEqual({ updateNote: { note: null } });
      const d = await t.run(
        `mutation($key: String!) { deleteNote(key: $key) { nodesDeleted } }`,
        { key },
        lou,
      );
      expect(d.errors).toBeUndefined();
      expect(d.data).toEqual({ deleteNote: { nodesDeleted: 0 } });
    }
    expect(await rows(t, "MATCH (n:Note) RETURN n.text AS t")).toEqual([
      { t: "x" },
    ]);
  });
});

describe("@authorizationDefaults(mutations:) per operation", () => {
  const typeDefs = `type Claims @jwt { sub: String  roles: [String!] }
    extend schema @authorizationDefaults(mutations: { jwt: { roles: { includes: "editor" } } })
    type Note @node @mutation
      @authorization(filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]) {
      key: String! @key
      owner: String!
    }
    type Tag @node @mutation
      @authorization(validate: [{ operations: [UPDATE], where: { jwt: { roles: { includes: "tagger" } } } }]) {
      key: String! @key
      name: String
    }`;
  const seed = "CREATE (:Tag {key: 't', name: 'a'})";
  const user = { jwt: { sub: "lou", roles: [] } };
  const editor = { jwt: { sub: "lou", roles: ["editor"] } };

  test("a filter rule does not cover CREATE: the default still guards it", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const denied = await t.run(
      `mutation { createNotes(input: [{ key: "n1", owner: "lou" }]) { notes { key } } }`,
      {},
      user,
    );
    expect(codes(denied)).toEqual(["FORBIDDEN"]);
    const allowed = await t.run(
      `mutation { createNotes(input: [{ key: "n1", owner: "lou" }]) { notes { key } } }`,
      {},
      editor,
    );
    expect(allowed.errors).toBeUndefined();
    // UPDATE is the type's own (its filter covers it): no editor needed.
    expect(
      (
        await t.run(
          `mutation { updateNote(key: "n1", update: { owner: "lou" }) { note { key } } }`,
          {},
          user,
        )
      ).errors,
    ).toBeUndefined();
  });

  test("a validate rule covers only its own operations", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const tagger = { jwt: { sub: "x", roles: ["tagger"] } };
    expect(
      (
        await t.run(
          `mutation { updateTag(key: "t", update: { name: "b" }) { tag { key } } }`,
          {},
          tagger,
        )
      ).errors,
    ).toBeUndefined();
    const create = await t.run(
      `mutation { createTags(input: [{ key: "t2" }]) { tags { key } } }`,
      {},
      tagger,
    );
    expect(codes(create)).toEqual(["FORBIDDEN"]);
    const del = await t.run(
      `mutation { deleteTag(key: "t") { nodesDeleted } }`,
      {},
      tagger,
    );
    expect(codes(del)).toEqual(["FORBIDDEN"]);
  });
});
