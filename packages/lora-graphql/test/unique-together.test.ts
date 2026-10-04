// @uniqueTogether: a combination of scalar fields and relationship ends no
// two nodes of a type may share, enforced by every generated mutation.

import type { GraphQLError } from "graphql";
import {
  buildModel,
  loraDriver,
  ModelError,
  scanExpands,
} from "../src/index.js";
import { festivalHarness, type Harness } from "./harness.js";

const typeDefs = /* GraphQL */ `
  type Claims @jwt {
    sub: String
    roles: [String!]
  }
  extend schema
    @authorizationDefaults(bypass: { jwt: { roles: { includes: "admin" } } })

  enum ConversationKind {
    DIRECT
    GROUP
  }

  type Person @node @mutation {
    key: String! @key
    requestsSent: [ConnectionRequest!]!
      @relationship(type: "SENT", direction: OUT)
    requestsReceived: [ConnectionRequest!]!
      @relationship(type: "TO", direction: IN)
    conversations: [Conversation!]! @relationship(type: "IN", direction: OUT)
  }

  type ConnectionRequest
    @node
    @mutation
    @uniqueTogether(fields: ["from", "to"]) {
    key: String! @key(generate: true)
    note: String
    from: Person! @relationship(type: "SENT", direction: IN)
    to: Person! @relationship(type: "TO", direction: OUT)
  }

  type Conversation
    @node
    @mutation
    @uniqueTogether(fields: ["participants"], where: { kind: { eq: DIRECT } }) {
    key: String! @key
    kind: ConversationKind!
    participants: [Person!]! @relationship(type: "IN", direction: IN)
  }

  type Handle
    @node
    @mutation
    @uniqueTogether(fields: ["site", "owner"])
    @uniqueTogether(fields: ["site", "name"])
    @authorization(
      validate: [
        {
          operations: [CREATE, UPDATE]
          where: { jwt: { roles: { includes: "member" } } }
        }
      ]
    ) {
    key: String! @key
    site: String!
    name: String
    owner: Person @relationship(type: "OWNS", direction: IN)
  }
`;

const member = { jwt: { sub: "a", roles: ["member"] } };
const admin = { jwt: { sub: "root", roles: ["admin"] } };

let h: Harness;

beforeEach(async () => {
  h = await festivalHarness({
    typeDefs,
    seed: [
      `UNWIND ['a', 'b', 'c', 'd'] AS k CREATE (:Person {key: k})`,
      // A request a -> b, and the DIRECT conversation of a and b.
      `MATCH (a:Person {key: 'a'}), (b:Person {key: 'b'})
       CREATE (a)-[:SENT]->(:ConnectionRequest {key: 'r1'})-[:TO]->(b),
              (a)-[:IN]->(c:Conversation {key: 'ab', kind: 'DIRECT'})<-[:IN]-(b)`,
    ],
  });
});

/** The first error of a mutation, which must fail. */
async function failure(
  source: string,
  context: Record<string, unknown> = {},
): Promise<GraphQLError> {
  const r = await h.run(source, {}, context);
  expect(r.errors, JSON.stringify(r.data)).toBeDefined();
  return r.errors![0]!;
}

const count = async (cypher: string) =>
  Number((await h.db.execute(cypher)).rows[0]!["c"]);

describe("two single relationships", () => {
  test("a second request for the same pair is refused and rolled back", async () => {
    const err = await failure(`mutation {
      createConnectionRequests(input: [{
        from: { connect: { key: "a" } }, to: { connect: { key: "b" } }
      }]) { info { nodesCreated } }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(err.message).toMatch(
      /^ConnectionRequest must be unique by \(from, to\)/,
    );
    expect(err.extensions["fields"]).toEqual(["from", "to"]);
    expect(
      await count(`MATCH (r:ConnectionRequest) RETURN count(r) AS c`),
    ).toBe(1);
  });

  test("the reverse pair and other pairs are fine", async () => {
    await h.data(`mutation {
      createConnectionRequests(input: [
        { from: { connect: { key: "b" } }, to: { connect: { key: "a" } } }
        { from: { connect: { key: "a" } }, to: { connect: { key: "c" } } }
      ]) { info { nodesCreated } }
    }`);
    expect(
      await count(`MATCH (r:ConnectionRequest) RETURN count(r) AS c`),
    ).toBe(3);
  });

  test("two duplicates in one input are refused", async () => {
    const err = await failure(`mutation {
      createConnectionRequests(input: [
        { from: { connect: { key: "c" } }, to: { connect: { key: "d" } } }
        { from: { connect: { key: "c" } }, to: { connect: { key: "d" } } }
      ]) { info { nodesCreated } }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(
      await count(`MATCH (r:ConnectionRequest) RETURN count(r) AS c`),
    ).toBe(1);
  });

  test("a nested create from the other side is checked", async () => {
    const err = await failure(`mutation {
      updatePerson(key: "a", update: {
        requestsSent: { create: [{ node: { to: { connect: { key: "b" } } } }] }
      }) { info { nodesCreated } }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
  });

  test("re-pointing an end onto a taken pair is refused", async () => {
    await h.data(`mutation {
      createConnectionRequests(input: [{
        key: "r2", from: { connect: { key: "a" } }, to: { connect: { key: "c" } }
      }]) { info { nodesCreated } }
    }`);
    const err = await failure(`mutation {
      updateConnectionRequest(key: "r2", update: { to: { connect: { key: "b" } } }) {
        connectionRequest { key }
      }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    // Updating a request without touching its ends is fine.
    await h.data(`mutation {
      updateConnectionRequest(key: "r1", update: { note: "hi" }) { connectionRequest { key } }
    }`);
  });

  test("an upsert that creates a duplicate is refused", async () => {
    const err = await failure(`mutation {
      upsertConnectionRequests(input: [{
        key: "r9", from: { connect: { key: "a" } }, to: { connect: { key: "b" } }
      }]) { connectionRequests { key } }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
  });
});

describe("a set-valued list relationship under where", () => {
  test("a second DIRECT conversation of the same pair is refused, in any order", async () => {
    const err = await failure(`mutation {
      createConversations(input: [{
        key: "ba", kind: DIRECT
        participants: { connect: [{ key: "b" }, { key: "a" }] }
      }]) { info { nodesCreated } }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(err.message).toMatch(
      /^Conversation must be unique by \(participants\)/,
    );
  });

  test("GROUP conversations, subsets and supersets are fine", async () => {
    await h.data(`mutation {
      createConversations(input: [
        { key: "g1", kind: GROUP, participants: { connect: [{ key: "a" }, { key: "b" }] } }
        { key: "abc", kind: DIRECT, participants: { connect: [{ key: "a" }, { key: "b" }, { key: "c" }] } }
        { key: "a1", kind: DIRECT, participants: { connect: [{ key: "a" }] } }
      ]) { info { nodesCreated } }
    }`);
    expect(await count(`MATCH (c:Conversation) RETURN count(c) AS c`)).toBe(4);
  });

  test("an update that brings a node under where is checked", async () => {
    await h.data(`mutation {
      createConversations(input: [
        { key: "g1", kind: GROUP, participants: { connect: [{ key: "a" }, { key: "b" }] } }
      ]) { info { nodesCreated } }
    }`);
    const err = await failure(`mutation {
      updateConversation(key: "g1", update: { kind: DIRECT }) { conversation { key } }
    }`);
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
  });

  test("connect and disconnect from the other side change the set", async () => {
    await h.data(`mutation {
      createConversations(input: [
        { key: "abc", kind: DIRECT, participants: { connect: [{ key: "a" }, { key: "b" }, { key: "c" }] } }
        { key: "ac", kind: DIRECT, participants: { connect: [{ key: "a" }, { key: "c" }] } }
      ]) { info { nodesCreated } }
    }`);
    // c leaves abc: it becomes {a, b}, which ab holds.
    const left = await failure(`mutation {
      updatePerson(key: "c", update: { conversations: { disconnect: ["abc"] } }) {
        person { key }
      }
    }`);
    expect(left.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    // b joins ac: it becomes {a, b, c}, which abc holds.
    const joined = await failure(`mutation {
      updatePerson(key: "b", update: { conversations: { connect: [{ key: "ac" }] } }) {
        person { key }
      }
    }`);
    expect(joined.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    // d joins ac: {a, c, d} is new.
    await h.data(`mutation {
      updatePerson(key: "d", update: { conversations: { connect: [{ key: "ac" }] } }) {
        person { key }
      }
    }`);
  });

  test("deleting a participant that shrinks a set onto a taken one is refused", async () => {
    await h.data(`mutation {
      createConversations(input: [
        { key: "abd", kind: DIRECT, participants: { connect: [{ key: "a" }, { key: "b" }, { key: "d" }] } }
      ]) { info { nodesCreated } }
    }`);
    const err = await failure(
      `mutation { deletePerson(key: "d") { nodesDeleted } }`,
    );
    expect(err.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(
      await count(`MATCH (p:Person {key: 'd'}) RETURN count(p) AS c`),
    ).toBe(1);
  });
});

describe("scalar and relationship combinations", () => {
  test("site + owner and site + name, and the bypass is not exempt", async () => {
    await h.data(
      `mutation {
        createHandles(input: [
          { key: "h1", site: "x", name: "ann", owner: { connect: { key: "a" } } }
        ]) { info { nodesCreated } }
      }`,
      {},
      member,
    );
    // Same site and owner.
    const owner = await failure(
      `mutation {
        createHandles(input: [
          { key: "h2", site: "x", name: "bob", owner: { connect: { key: "a" } } }
        ]) { info { nodesCreated } }
      }`,
      admin,
    );
    expect(owner.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
    expect(owner.message).toMatch(/unique by \(site, owner\)/);
    // Same site and name, scalars only.
    const named = await failure(
      `mutation {
        createHandles(input: [{ key: "h3", site: "x", name: "ann" }]) { info { nodesCreated } }
      }`,
      admin,
    );
    expect(named.message).toMatch(/unique by \(site, name\)/);
    // No owner and no name: exempt, like a null in a unique index.
    await h.data(
      `mutation {
        createHandles(input: [{ key: "h4", site: "x" }, { key: "h5", site: "x" }]) {
          info { nodesCreated }
        }
      }`,
      {},
      member,
    );
    // Another site is fine.
    await h.data(
      `mutation {
        createHandles(input: [
          { key: "h6", site: "y", name: "ann", owner: { connect: { key: "a" } } }
        ]) { info { nodesCreated } }
      }`,
      {},
      member,
    );
    // Renaming onto a taken name.
    const renamed = await failure(
      `mutation {
        updateHandle(key: "h4", update: { name: "ann" }) { handle { key } }
      }`,
      member,
    );
    expect(renamed.extensions["code"]).toBe("CONSTRAINT_VIOLATION");
  });

  test("the checks seek the written nodes and expand from them, never from a scan", async () => {
    h.statements.length = 0;
    await h.data(`mutation {
      createConnectionRequests(input: [{
        from: { connect: { key: "c" } }, to: { connect: { key: "d" } }
      }]) { info { nodesCreated } }
    }`);
    await h.data(`mutation {
      createConversations(input: [
        { key: "cd", kind: DIRECT, participants: { connect: [{ key: "c" }, { key: "d" }] } }
      ]) { info { nodesCreated } }
    }`);
    await h.data(
      `mutation {
        createHandles(input: [{ key: "h1", site: "x", name: "ann", owner: { connect: { key: "a" } } }]) {
          info { nodesCreated }
        }
      }`,
      {},
      member,
    );
    const checks = h.statements
      .map((s) => s.statement)
      .filter((st) => /m\.`?key`? <> n\.`?key`?/.test(st.text));
    // ConnectionRequest, Conversation, and both of Handle's.
    expect(checks).toHaveLength(4);
    const driver = loraDriver(h.db);
    for (const st of checks) {
      expect(scanExpands(st, await driver.explain!(st))).toEqual([]);
    }
  });
});

describe("model errors", () => {
  const problems = (sdl: string): string[] => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };
  const base = (directive: string, extra = "") => `
    type P @node { key: String! @key }
    type T @node ${directive} {
      key: String! @key
      name: String
      tags: [String!]
      one: P @relationship(type: "ONE", direction: OUT)
      many: [P!]! @relationship(type: "MANY", direction: OUT)
      more: [P!]! @relationship(type: "MORE", direction: OUT)
      computed: Int @cypher(statement: "RETURN 1 AS x", columnName: "x")
      ${extra}
    }`;

  test("a valid declaration builds", () => {
    expect(
      problems(
        base(
          `@uniqueTogether(fields: ["name", "one", "many"], where: { name: { eq: "x" } })`,
        ),
      ),
    ).toEqual([]);
  });

  test.each([
    [`@uniqueTogether(fields: [])`, "@uniqueTogether: fields is empty"],
    [
      `@uniqueTogether(fields: ["nope"])`,
      "@uniqueTogether: T has no field nope",
    ],
    [
      `@uniqueTogether(fields: ["computed"])`,
      "@uniqueTogether: computed is a @cypher field; only stored fields and relationships can be unique",
    ],
    [
      `@uniqueTogether(fields: ["many", "more"])`,
      "@uniqueTogether: fields names 2 list relationships (many, more); at most one is compared as a set",
    ],
    [
      `@uniqueTogether(fields: ["tags"])`,
      "@uniqueTogether: tags is a list; list scalar fields cannot be unique",
    ],
    [
      `@uniqueTogether(fields: ["name", "name"])`,
      "@uniqueTogether: fields names a field twice",
    ],
    [
      `@uniqueTogether(fields: ["name"], where: { nope: { eq: 1 } })`,
      "@uniqueTogether: where.nope: T has no field nope",
    ],
  ])("%s", (directive, message) => {
    expect(problems(base(directive))).toContain(message);
  });

  test("a relationship to an interface is refused", () => {
    expect(
      problems(`
        interface I { key: String! }
        type P implements I @node { key: String! @key }
        type T @node @uniqueTogether(fields: ["i"]) {
          key: String! @key
          i: I @relationship(type: "I", direction: OUT)
        }`),
    ).toContain(
      "@uniqueTogether: i reaches an interface or union; only relationships to a @node type can be unique",
    );
  });

  test("not on a relationship properties type", () => {
    expect(
      problems(`
        type P @node { key: String! @key  f: [P!]! @relationship(type: "F", direction: OUT, properties: "E") }
        type E @relationshipProperties @uniqueTogether(fields: ["since"]) { since: Int }`),
    ).toContain(
      "@uniqueTogether is not supported on a relationship properties type",
    );
  });
});
