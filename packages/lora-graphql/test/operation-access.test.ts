// `lora.operationAccess()`: who may run an operation, per root field and
// kind of caller, from the rules the access matrix reads.

import { createDatabase } from "@loradb/lora-node";
import { parse } from "graphql";
import { describe, expect, test } from "vitest";
import { LoraGraphQL, loraDriver } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node @mutation { key: String! @key  name: String }
type Article @node @mutation @subscription
  @authentication(operations: [CREATE, UPDATE, DELETE], jwt: { roles: { includes: "admin" } }) {
  key: String! @key @relayId
  title: String
}
type Draft @node @mutation
  @authorization(filter: [{ operations: [READ, UPDATE], where: { node: { author: { eq: "\${jwt.sub}" } } } }]) {
  key: String! @key @relayId
  author: String!
}
union Item = Article | Draft
type Mutation {
  wipe: Int
    @authorization(validate: [{ where: { jwt: { roles: { includes: "admin" } } } }])
    @cypher(statement: "MATCH (a:Article) DETACH DELETE a RETURN 0 AS n")
}`;

const lora = async () =>
  new LoraGraphQL({ typeDefs, driver: loraDriver(await createDatabase()) });

describe("operationAccess", () => {
  test("an admin-only mutation: every root field denied to everyone but admins", async () => {
    const l = await lora();
    const access =
      l.operationAccess(`mutation Seed($a: [ArticleUpsertInput!]!) {
      upsertArticles(input: $a) { articles { key } }
      gone: deleteArticle(key: "x") { deleted }
      wipe
    }`);
    expect(access.operation).toBe("mutation");
    expect(access.name).toBe("Seed");
    expect(access.principals).toEqual([
      "anonymous",
      "authenticated",
      "roles:admin",
    ]);
    expect(
      access.fields.map((f) => [f.field, f.alias, f.type, f.operations]),
    ).toEqual([
      ["upsertArticles", undefined, "Article", ["CREATE", "UPDATE"]],
      ["deleteArticle", "gone", "Article", ["DELETE"]],
      ["wipe", undefined, "Mutation", ["EXECUTE"]],
    ]);
    expect(access.verdicts).toEqual({
      anonymous: "unauthenticated",
      authenticated: "denied",
      "roles:admin": "allowed",
    });
    expect(access.fields[0]!.access["anonymous"]).toEqual({
      verdict: "unauthenticated",
      by: ["CREATE: @authentication", "UPDATE: @authentication"],
    });
  });

  test("queries, fragments, unions, node(id:) and subscriptions", async () => {
    const l = await lora();
    const q = l.operationAccess(
      parse(`query Feed { ...Root items { __typename } __typename }
        fragment Root on Query { articles { key } drafts { key } }`),
    );
    expect(q.fields.map((f) => f.field)).toEqual([
      "articles",
      "drafts",
      "items",
    ]);
    expect(q.fields[0]!.access["anonymous"]!.verdict).toBe("allowed");
    expect(q.fields[1]!.access["authenticated"]!.verdict).toBe("filtered");
    expect(q.fields[2]!.type).toBe("Item");
    expect(q.fields[2]!.access["authenticated"]).toEqual({
      verdict: "filtered",
      by: ["Article: no rule", "Draft: filter[0]"],
    });
    expect(q.verdicts["authenticated"]).toBe("filtered");

    const node = l.operationAccess(
      `{ node(id: "x") { id } nodes(ids: []) { id } }`,
    );
    expect(node.fields.map((f) => f.type)).toEqual(["Node", "Node"]);

    const sub = l.operationAccess(`subscription { articleChanged { key } }`);
    expect(sub.fields[0]!.operations).toEqual(["SUBSCRIBE"]);
    expect(sub.verdicts["anonymous"]).toBe("allowed");
  });

  test("a persisted id, operationName, and unknown fields", async () => {
    const l = await lora();
    l.persist({ abc: `query A { articles { key } } mutation B { wipe }` });
    expect(l.operationAccess("abc", "B").fields[0]!.field).toBe("wipe");
    expect(() => l.operationAccess("abc")).toThrow(/pass operationName/);
    expect(() => l.operationAccess(`{ nope }`)).toThrow(/no root field nope/);
  });

  // What a mutation's inputs write through relationship fields counts
  // towards its root field (Festimap G-43).
  const rooms = `type Claims @jwt { sub: String!  roles: [String!] }
extend schema @authorizationDefaults(requireAuthentication: false)
type Room @node @mutation(operations: [CREATE, UPDATE]) @authorization(public: [CREATE, UPDATE]) {
  key: String! @key
  kind: String!
  chan: Chan @relationship(type: "IN", direction: OUT, nestedOperations: [CONNECT, DISCONNECT, CREATE])
    @authentication(operations: [CREATE_RELATIONSHIP])
    @authorization(validate: [{ operations: [CONNECT], where: { source: { kind: { eq: "CHANNEL" } } } }])
  tags: [Tag!]! @relationship(type: "TAGGED", direction: OUT, nestedOperations: [CONNECT, DISCONNECT])
    @authentication(operations: [DELETE_RELATIONSHIP])
}
type Chan @node @mutation(operations: [CREATE])
  @authorization(validate: [{ operations: [CREATE], where: { jwt: { roles: { includes: "admin" } } } }]) {
  key: String! @key
}
type Tag @node { key: String! @key }`;

  // The first error's code, or its message when it has none (a request
  // graphql-js rejects), so a malformed test input does not read as success.
  const firstError = (r: {
    errors?: ReadonlyArray<{
      message: string;
      extensions?: Record<string, unknown>;
    }>;
  }) => {
    const e = r.errors?.[0];
    return e && ((e.extensions?.["code"] as string | undefined) ?? e.message);
  };

  test("a nested connect answers to the relationship field's rows", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: rooms });
    await t.db.execute("CREATE (:Chan {key: 'c'}), (:Tag {key: 't'})");
    const create = `mutation { createRooms(input: [{ key: "r", kind: "CHANNEL", chan: { connect: { key: "c" } } }]) { rooms { key } } }`;
    const plain = `mutation { createRooms(input: [{ key: "p", kind: "CHANNEL" }]) { rooms { key } } }`;
    const code = async (source: string, jwt?: Record<string, unknown>) =>
      firstError(await t.run(source, {}, jwt ? { jwt } : {}));
    // Enforcement first: what the verdicts have to agree with.
    expect(await code(plain)).toBeUndefined();
    expect(await code(create)).toBe("UNAUTHENTICATED");
    expect(await code(create, { sub: "u" })).toBeUndefined();

    expect(t.lora.operationAccess(plain).verdicts).toMatchObject({
      anonymous: "allowed",
      authenticated: "allowed",
    });
    const access = t.lora.operationAccess(create);
    expect(access.verdicts).toMatchObject({
      anonymous: "unauthenticated",
      authenticated: "validated",
    });
    expect(access.fields[0]!.access["anonymous"]).toEqual({
      verdict: "unauthenticated",
      by: [
        "public",
        "Room.chan CONNECT: @authentication",
        "Room.chan CONNECT: validate[0]",
      ],
    });
    expect(access.fields[0]!.unresolved).toBeUndefined();
  });

  test("inputs given as variables are read when their values are passed", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: rooms });
    const source = `mutation Make($input: [RoomCreateInput!]!) { createRooms(input: $input) { rooms { key } } }`;
    // Without a value the write is unknown, and said to be.
    const blind = t.lora.operationAccess(source);
    expect(blind.verdicts["anonymous"]).toBe("allowed");
    expect(blind.fields[0]!.unresolved).toEqual(["input"]);

    const read = t.lora.operationAccess(source, undefined, {
      input: [{ key: "r", kind: "CHANNEL", chan: { connect: { key: "c" } } }],
    });
    expect(read.verdicts["anonymous"]).toBe("unauthenticated");
    expect(read.fields[0]!.unresolved).toBeUndefined();
  });

  test("nested creates, disconnects and re-pointed single relationships", async () => {
    const t = await createTestLoraGraphQL({ typeDefs: rooms });
    await t.db.execute(
      "CREATE (r:Room {key: 'r', kind: 'CHANNEL'})-[:TAGGED]->(:Tag {key: 't'}), (:Chan {key: 'c'})",
    );
    const by = (source: string, principal: string) =>
      t.lora.operationAccess(source).fields[0]!.access[principal]!;
    const code = async (source: string, jwt?: Record<string, unknown>) =>
      firstError(await t.run(source, {}, jwt ? { jwt } : {}));

    // A nested create is a CONNECT and a CREATE of the target.
    const nested = `mutation { createRooms(input: [{ key: "n", kind: "CHANNEL", chan: { create: { node: { key: "c2" } } } }]) { rooms { key } } }`;
    expect(by(nested, "authenticated")).toEqual({
      verdict: "denied",
      by: [
        "public",
        "Room.chan CONNECT: @authentication",
        "Room.chan CONNECT: validate[0]",
        "Chan CREATE: validate[0]",
      ],
    });
    expect(by(nested, "roles:admin").verdict).toBe("validated");
    expect(await code(nested, { sub: "u" })).toBe("FORBIDDEN");
    expect(await code(nested, { sub: "a", roles: ["admin"] })).toBeUndefined();

    // A disconnect answers to DELETE_RELATIONSHIP on the field.
    const drop = `mutation { updateRoom(key: "r", update: { tags: { disconnect: ["t"] } }) { room { key } } }`;
    expect(by(drop, "anonymous")).toEqual({
      verdict: "unauthenticated",
      by: ["public", "Room.tags DISCONNECT: @authentication"],
    });
    expect(by(drop, "authenticated").verdict).toBe("allowed");
    expect(await code(drop)).toBe("UNAUTHENTICATED");
    expect(await code(drop, { sub: "u" })).toBeUndefined();

    // Connecting through a single relationship in an update replaces the
    // one it had: a CONNECT and a DISCONNECT.
    const move = `mutation { updateRoom(key: "r", update: { chan: { connect: { key: "c" } } }) { room { key } } }`;
    expect(by(move, "anonymous").by).toEqual([
      "public",
      "Room.chan CONNECT: @authentication",
      "Room.chan CONNECT: validate[0]",
    ]);
    expect(by(move, "anonymous").verdict).toBe("unauthenticated");
    expect(await code(move)).toBe("UNAUTHENTICATED");
  });
});
