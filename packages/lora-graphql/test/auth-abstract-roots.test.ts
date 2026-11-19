// `@authentication` and `@authorization` on an interface or union guard its
// generated root fields (`<plural>`, `<plural>Connection`) and nothing
// else: each implementation keeps its own rules for its rows, its own
// roots and every relationship that reaches it through the interface.

import type { ExecutionResult } from "graphql";
import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = (guard: string) => `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
type Person @node { key: String! @key  staff: Boolean }
interface MediaFile ${guard} {
  key: String!
  path: String! @filterable
}
type Image implements MediaFile @node
  @authorization(filter: [{ operations: [READ], where: { node: { public: { eq: true } } } }]) {
  key: String! @key
  path: String! @filterable
  public: Boolean!
}
type Video implements MediaFile @node {
  key: String! @key
  path: String! @filterable
}
type Event @node {
  key: String! @key
  media: [MediaFile!]! @relationship(type: "SHOWS", direction: OUT)
}`;
const adminOnly =
  '@authorization(validate: [{ where: { jwt: { roles: { includes: "admin" } } } }])';
const seed = [
  "CREATE (:Person {key: 'ann', staff: true}), (:Person {key: 'bob', staff: false})",
  "CREATE (e:Event {key: 'e1'})",
  "MATCH (e:Event) CREATE (e)-[:SHOWS]->(:Image {key: 'i1', path: '/a.jpg', public: true}), (e)-[:SHOWS]->(:Image {key: 'i2', path: '/b.jpg', public: false}), (e)-[:SHOWS]->(:Video {key: 'v1', path: '/c.mp4'})",
];
const as = (sub: string, roles: string[] = []) => ({ jwt: { sub, roles } });
const admin = as("ann", ["admin"]);
const codes = (r: ExecutionResult) =>
  r.errors?.map((e) => e.extensions?.["code"]);
const keys = (rows: Array<{ key: string }>) => rows.map((r) => r.key).sort();

describe("@authorization on an interface", () => {
  test("guards the interface's roots: an admin reads them, others are refused", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(adminOnly),
      seed,
    });
    const d = await t.data<{
      mediaFiles: Array<{ key: string }>;
      mediaFilesConnection: { totalCount: number };
    }>("{ mediaFiles { key } mediaFilesConnection { totalCount } }", {}, admin);
    // Image's own filter still holds under the interface's rule.
    expect(keys(d.mediaFiles)).toEqual(["i1", "v1"]);
    expect(d.mediaFilesConnection.totalCount).toBe(2);

    t.statements.length = 0;
    const user = await t.run("{ mediaFiles { key } }", {}, as("bob"));
    expect(codes(user)).toEqual(["FORBIDDEN"]);
    expect(user.errors![0]!.message).toBe(
      "not allowed to run Query.mediaFiles",
    );
    const anon = await t.run("{ mediaFilesConnection { totalCount } }");
    expect(codes(anon)).toEqual(["UNAUTHENTICATED"]);
    // No statement ran for a refused root.
    expect(t.statements).toEqual([]);
  });

  test("leaves each implementation's roots and the relationships to the interface alone", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(adminOnly),
      seed,
    });
    const d = await t.data<{
      images: Array<{ key: string }>;
      videos: Array<{ key: string }>;
      event: { media: Array<{ key: string }> };
    }>(
      '{ images { key } videos { key } event(key: "e1") { media { key } } }',
      {},
      as("bob"),
    );
    expect(keys(d.images)).toEqual(["i1"]);
    expect(keys(d.videos)).toEqual(["v1"]);
    expect(keys(d.event.media)).toEqual(["i1", "v1"]);
  });

  test("a viewer test runs in the database before the read", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(
        "@authorization(validate: [{ where: { viewer: { staff: { eq: true } } } }])",
      ),
      seed,
    });
    expect(
      keys(
        (
          await t.data<{ mediaFiles: Array<{ key: string }> }>(
            "{ mediaFiles { key } }",
            {},
            as("ann"),
          )
        ).mediaFiles,
      ),
    ).toEqual(["i1", "v1"]);
    expect(codes(await t.run("{ mediaFiles { key } }", {}, as("bob")))).toEqual(
      ["FORBIDDEN"],
    );
  });

  test("@authentication on an interface asks for a token on its roots only", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs("@authentication(operations: [READ])"),
      seed,
    });
    expect(codes(await t.run("{ mediaFiles { key } }"))).toEqual([
      "UNAUTHENTICATED",
    ]);
    expect(
      (await t.data<{ videos: unknown[] }>("{ videos { key } }")).videos,
    ).toHaveLength(1);
    expect(
      (
        await t.data<{ mediaFiles: unknown[] }>(
          "{ mediaFiles { key } }",
          {},
          as("bob"),
        )
      ).mediaFiles,
    ).toHaveLength(2);
  });

  test("works on a union too", async () => {
    const sdl = `${typeDefs("")}
union Anything @plural(value: "anything") ${adminOnly} = Image | Video`;
    const t = await createTestLoraGraphQL({ typeDefs: sdl, seed });
    expect(
      codes(await t.run("{ anything { __typename } }", {}, as("bob"))),
    ).toEqual(["FORBIDDEN"]);
    expect(
      (
        await t.data<{ anything: unknown[] }>(
          "{ anything { __typename } }",
          {},
          admin,
        )
      ).anything,
    ).toHaveLength(2);
  });

  test("the access matrix lists the guard", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: typeDefs(adminOnly),
      seed,
    });
    const rows = t.lora.accessMatrix().filter((e) => e.type === "MediaFile");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.every((r) => r.operation === "READ")).toBe(true);
  });

  test("refuses filter rules, masks and node tests", () => {
    const refused = (guard: string) => {
      try {
        buildModel(typeDefs(guard));
      } catch (err) {
        if (err instanceof ModelError) {
          return err.problems.map((p) => `${p.type}: ${p.message}`);
        }
        throw err;
      }
      return [];
    };
    expect(
      refused(
        '@authorization(filter: [{ where: { jwt: { roles: { includes: "admin" } } } }])',
      ),
    ).toContain(
      "MediaFile: @authorization on an interface guards its root fields, so it takes validate rules: there are no rows to filter",
    );
    expect(
      refused(
        '@authorization(validate: [{ where: { node: { path: { eq: "/a" } } } }])',
      ).some((m) =>
        m.includes("guards its root fields: its rules test jwt and viewer"),
      ),
    ).toBe(true);
  });
});
