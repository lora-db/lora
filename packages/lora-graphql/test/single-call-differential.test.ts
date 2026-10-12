// The single call must be invisible: the same mutation answers the same
// and leaves the same graph whether it went to the database as one call
// or through a transaction. Every case here runs both ways.

import { createDatabase } from "@loradb/lora-node";
import { describe, expect, test } from "vitest";
import { LoraGraphQL, loraDriver, type WriteChange } from "../src/index.js";

const typeDefs = /* GraphQL */ `
  extend schema @authorizationDefaults(requireAuthentication: false)

  type Doc
    @node
    @mutation
    @subscription(previousState: true)
    @authorization(
      filter: [{ where: { node: { owner: { eq: "$jwt.sub" } } } }]
      validate: [
        { operations: [CREATE], where: { node: { owner: { eq: "$jwt.sub" } } } }
        {
          operations: [UPDATE]
          when: [BEFORE]
          where: { node: { locked: { eq: false } } }
        }
        {
          operations: [UPDATE]
          when: [AFTER]
          where: { node: { open: { eq: true } } }
        }
      ]
    ) {
    key: String! @key
    owner: String
    title: String
    slug: String @unique
    locked: Boolean
    open: Boolean
    n: Int
    count: Int
      @authorization(
        validate: [
          { operations: [UPDATE], where: { jwt: { role: { eq: "admin" } } } }
        ]
      )
    boom: Int @cypher(statement: "RETURN 1 / this.n AS v")
    note: String
    stamp: String @populatedBy(callback: "stamp", operations: [CREATE, UPDATE])
    secret: String
      @authorization(
        validate: [
          { operations: [UPDATE], where: { jwt: { role: { eq: "admin" } } } }
        ]
      )
    tags: [Tag!]! @relationship(type: "HAS", direction: OUT)
  }

  type Tag @node @mutation {
    key: String! @key
  }
`;

const seed = [
  "CREATE (:Doc {key: 'mine', owner: 'me', title: 'One', slug: 'one', note: 'n', locked: false, open: true, n: 0})",
  "CREATE (:Doc {key: 'locked', owner: 'me', title: 'Two', locked: true, open: true})",
  "CREATE (:Doc {key: 'theirs', owner: 'other', title: 'Three', slug: 'three', locked: false, open: true})",
  "CREATE (:Tag {key: 't1'})",
];

interface Case {
  name: string;
  source: string;
  jwt?: Record<string, unknown>;
}

const me = { sub: "me" };

const cases: Case[] = [
  // Found in review.
  {
    name: "a field named in adjust with null still answers to its rule",
    source: `mutation { updateDoc(key: "mine", update: { title: "a" }, adjust: { count: null }) { doc { key title } } }`,
    jwt: me,
  },
  {
    name: "an update left out, on a missing node",
    source: `mutation { updateDoc(key: "nope") { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "an update left out, on a node",
    source: `mutation { updateDoc(key: "mine") { doc { key stamp } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "an update left out, refused before",
    source: `mutation { updateDoc(key: "locked") { doc { key } } }`,
    jwt: me,
  },
  {
    name: "a rule refuses after the write and the payload read would fail too",
    source: `mutation { updateDoc(key: "mine", update: { open: false }) { doc { key boom } } }`,
    jwt: me,
  },
  {
    name: "a payload read that fails",
    source: `mutation { updateDoc(key: "mine", update: { title: "b" }) { doc { key boom } } }`,
    jwt: me,
  },
  // Creates.
  {
    name: "create",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", title: "N" }]) { docs { key title stamp } info { nodesCreated } } }`,
    jwt: me,
  },
  {
    name: "create two shapes",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me" }, { key: "m", owner: "me", title: "M" }]) { docs { key title } } }`,
    jwt: me,
  },
  {
    name: "create refused by a rule",
    source: `mutation { createDocs(input: [{ key: "n", owner: "other" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create two shapes, one refused",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me" }, { key: "m", owner: "other", title: "M" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create under a hidden key",
    source: `mutation { createDocs(input: [{ key: "theirs", owner: "me" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create under a visible key",
    source: `mutation { createDocs(input: [{ key: "mine", owner: "me" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create with a taken unique value",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", slug: "three" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create with a visible taken unique value",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", slug: "one" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create under a hidden key that a rule also refuses",
    source: `mutation { createDocs(input: [{ key: "theirs", owner: "other" }]) { docs { key } } }`,
    jwt: me,
  },
  {
    name: "create anonymous",
    source: `mutation { createDocs(input: [{ key: "n" }]) { docs { key } } }`,
  },
  {
    name: "create nothing",
    source: `mutation { createDocs(input: []) { docs { key } info { nodesCreated } } }`,
    jwt: me,
  },
  {
    name: "create with a relationship",
    source: `mutation { createDocs(input: [{ key: "n", owner: "me", tags: { connect: [{ key: "t1" }] } }]) { docs { key tags { key } } } }`,
    jwt: me,
  },
  // Updates.
  {
    name: "update",
    source: `mutation { updateDoc(key: "mine", update: { title: "New" }) { doc { key title stamp } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update nothing",
    source: `mutation { updateDoc(key: "mine", update: {}) { doc { key title } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update to null",
    source: `mutation { updateDoc(key: "mine", update: { note: null }) { doc { key note } } }`,
    jwt: me,
  },
  {
    name: "update a missing node",
    source: `mutation { updateDoc(key: "nope", update: { title: "x" }) { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update a node hidden from the caller",
    source: `mutation { updateDoc(key: "theirs", update: { title: "x" }) { doc { key } info { nodesUpdated } } }`,
    jwt: me,
  },
  {
    name: "update refused before",
    source: `mutation { updateDoc(key: "locked", update: { title: "x" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update refused after",
    source: `mutation { updateDoc(key: "mine", update: { open: false }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update a field the claims refuse",
    source: `mutation { updateDoc(key: "mine", update: { secret: "s" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update that field as admin",
    source: `mutation { updateDoc(key: "mine", update: { secret: "s" }) { doc { key secret } } }`,
    jwt: { sub: "me", role: "admin" },
  },
  {
    name: "update that field on a missing node",
    source: `mutation { updateDoc(key: "nope", update: { secret: "s" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update to a taken unique value",
    source: `mutation { updateDoc(key: "mine", update: { slug: "three" }) { doc { key } } }`,
    jwt: me,
  },
  {
    name: "update with a relationship",
    source: `mutation { updateDoc(key: "mine", update: { title: "R", tags: { connect: [{ key: "t1" }] } }) { doc { title tags { key } } } }`,
    jwt: me,
  },
  {
    name: "update anonymous",
    source: `mutation { updateDoc(key: "mine", update: { title: "x" }) { doc { key } } }`,
  },
  {
    name: "update with only info",
    source: `mutation { updateDoc(key: "mine", update: { title: "I" }) { info { nodesUpdated } } }`,
    jwt: me,
  },
];

async function outcome(single: boolean, c: Case) {
  let calls = 0;
  const db = await createDatabase();
  for (const statement of seed) await db.execute(statement);
  const lora = new LoraGraphQL({
    typeDefs,
    driver: loraDriver(db),
    singleCallMutations: single,
    maskErrors: false,
    callbacks: {
      stamp: ({ operation }) => {
        calls++;
        return `by-${operation}`;
      },
    },
  });
  await lora.assertSchema({ create: true });
  const changes: WriteChange[] = [];
  lora.onWrite((change) => changes.push(change));
  const result = await lora.execute({
    source: c.source,
    context: c.jwt ? { jwt: c.jwt } : {},
  });
  const graph = await db.execute(
    "MATCH (n) OPTIONAL MATCH (n)-[r]->(m) RETURN labels(n) AS labels, properties(n) AS props, type(r) AS rel, m.key AS target ORDER BY n.key, target",
  );
  return {
    data: result.data ?? null,
    errors: (result.errors ?? []).map((e) => {
      // A database error carries a random correlation id.
      const { id: _id, ...extensions } = e.extensions;
      return { message: e.message, extensions };
    }),
    graph: graph.rows,
    // How often a @populatedBy callback ran.
    calls,
    changes: changes.map(({ timestamp: _timestamp, ...change }) => change),
  };
}

describe("one call and a transaction answer alike", () => {
  test.each(cases)("$name", async (c) => {
    const [single, transaction] = await Promise.all([
      outcome(true, c),
      outcome(false, c),
    ]);
    expect(single).toEqual(transaction);
  });
});
