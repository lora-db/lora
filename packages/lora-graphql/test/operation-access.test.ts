// `lora.operationAccess()`: who may run an operation, per root field and
// kind of caller, from the rules the access matrix reads.

import { createDatabase } from "@loradb/lora-node";
import { parse } from "graphql";
import { describe, expect, test } from "vitest";
import { LoraGraphQL, loraDriver } from "../src/index.js";

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
});
