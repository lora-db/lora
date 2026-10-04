// `@settable(onCreate: true)` / `onUpdate: true` on a `@timestamp` or
// `@populatedBy` field: the input offers the field, a field-level rule
// decides who may supply it, and the computation fills it when omitted.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const codes = (r: { errors?: ReadonlyArray<{ extensions?: unknown }> }) =>
  r.errors?.map(
    (e) => (e.extensions as Record<string, unknown> | undefined)?.["code"],
  );
const admin = (ops: string) =>
  `@authorization(validate: [{ operations: [${ops}], where: { jwt: { roles: { includes: "admin" } } } }])`;
const typeDefs = `type Claims @jwt { sub: String!  roles: [String!] }
type Post @node @mutation {
  key: String! @key
  title: String
  createdAt: DateTime! @timestamp(operations: [CREATE]) @settable(onCreate: true) ${admin("CREATE")}
  updatedAt: DateTime @timestamp(operations: [UPDATE]) @settable(onUpdate: true) ${admin("UPDATE")}
  slug: String! @populatedBy(callback: "slug", operations: [CREATE, UPDATE])
    @settable(onCreate: true, onUpdate: true) ${admin("CREATE, UPDATE")}
  stamp: DateTime @timestamp
}`;
const as = (roles: string[]) => ({ jwt: { sub: "u", roles } });
const setup = () =>
  createTestLoraGraphQL({
    typeDefs,
    callbacks: {
      slug: ({ input }) => `auto-${String(input["title"] ?? "x")}`,
    },
  });
const create = (fields: string) =>
  `mutation { createPosts(input: [{ key: "p", title: "Hi"${fields} }]) { posts { key createdAt slug } } }`;
const read = `{ post(key: "p") { createdAt updatedAt slug stamp } }`;

describe("computed fields settable by rule", () => {
  test("the inputs offer them; bare @timestamp stays out", async () => {
    const t = await setup();
    const fields = (name: string) =>
      Object.keys(
        (
          t.schema.getType(name) as { getFields: () => Record<string, unknown> }
        ).getFields(),
      );
    expect(fields("PostCreateInput")).toEqual(
      expect.arrayContaining(["createdAt", "slug"]),
    );
    expect(fields("PostCreateInput")).not.toContain("updatedAt");
    expect(fields("PostCreateInput")).not.toContain("stamp");
    expect(fields("PostUpdateInput")).toEqual(
      expect.arrayContaining(["updatedAt", "slug"]),
    );
    expect(fields("PostUpdateInput")).not.toContain("createdAt");
  });

  test("an allowed caller supplies them; the computation is skipped", async () => {
    const t = await setup();
    const r = await t.run(
      create(`, createdAt: "2019-05-01T10:00:00Z", slug: "hand-made"`),
      {},
      as(["admin"]),
    );
    expect(r.errors).toBeUndefined();
    const d = await t.data<{ post: Record<string, string> }>(read);
    expect(d.post["createdAt"]).toMatch(/^2019-05-01T10:00/);
    expect(d.post["slug"]).toBe("hand-made");
    expect(d.post["stamp"]).toBeTruthy();
  });

  test("anyone else is refused; omitted, they are computed as before", async () => {
    const t = await setup();
    expect(
      codes(
        await t.run(create(`, createdAt: "2019-05-01T10:00:00Z"`), {}, as([])),
      ),
    ).toEqual(["FORBIDDEN"]);
    expect(codes(await t.run(create(`, slug: "mine"`), {}, as([])))).toEqual([
      "FORBIDDEN",
    ]);
    expect(codes(await t.run(create(`, slug: "mine"`)))).toEqual([
      "UNAUTHENTICATED",
    ]);
    expect(codes(await t.run(create(""), {}, as([])))).toBeUndefined();
    const d = await t.data<{ post: Record<string, string> }>(read);
    expect(d.post["slug"]).toBe("auto-Hi");
    expect(new Date(d.post["createdAt"]!).getFullYear()).toBeGreaterThan(2020);
  });

  test("updates: a supplied @timestamp is kept, an omitted one is now", async () => {
    const t = await setup();
    await t.run(create(""), {}, as([]));
    const update = (fields: string) =>
      `mutation { updatePost(key: "p", update: { title: "T"${fields} }) { post { key } } }`;
    expect(
      codes(
        await t.run(
          update(`, updatedAt: "2018-01-01T00:00:00Z"`),
          {},
          as(["admin"]),
        ),
      ),
    ).toBeUndefined();
    let d = await t.data<{ post: Record<string, string> }>(read);
    expect(d.post["updatedAt"]).toMatch(/^2018-01-01/);
    expect(d.post["slug"]).toBe("auto-T");
    expect(
      codes(
        await t.run(update(`, updatedAt: "2018-01-01T00:00:00Z"`), {}, as([])),
      ),
    ).toEqual(["FORBIDDEN"]);
    await t.run(update(""), {}, as([]));
    d = await t.data<{ post: Record<string, string> }>(read);
    expect(new Date(d.post["updatedAt"]!).getFullYear()).toBeGreaterThan(2020);
  });

  test("the access matrix lists who may supply them", async () => {
    const t = await setup();
    const m = t.lora.accessMatrix();
    const v = (id: string) =>
      m.find((e) => `${e.type}.${e.field} ${e.operation} ${e.principal}` === id)
        ?.verdict;
    expect(v("Post.createdAt CREATE authenticated")).toBe("denied");
    expect(v("Post.createdAt CREATE roles:admin")).toBe("allowed");
  });

  test("model checks", () => {
    const problems = (sdl: string) => {
      try {
        buildModel(sdl);
        return [];
      } catch (err) {
        if (!(err instanceof ModelError)) throw err;
        return err.problems.map((p) => p.message);
      }
    };
    expect(
      problems(`type Post @node @mutation {
  key: String! @key
  createdAt: DateTime @timestamp @settable(onCreate: true)
}`),
    ).toContain(
      "@settable(onCreate: true) on a @timestamp or @populatedBy field needs a field-level @authorization(validate:) rule for CREATE: it decides who may supply the value (the schema's bypass is no such rule)",
    );
    // A READ rule does not guard the write.
    expect(
      problems(`type Claims @jwt { sub: String!  roles: [String!] }
type Post @node @mutation {
  key: String! @key
  slug: String @populatedBy(callback: "s", operations: [UPDATE]) @settable(onUpdate: true) ${admin("READ")}
}`).some((m) => m.includes("rule for UPDATE")),
    ).toBe(true);
    expect(
      problems(`type Claims @jwt { sub: String!  roles: [String!] }
type Person @node { key: String! @key  trips: [Trip!]! @relationship(type: "M", direction: OUT, properties: "E") }
type Trip @node { key: String! @key }
type E @relationshipProperties {
  at: DateTime @timestamp @settable(onCreate: true) ${admin("CREATE")}
}`),
    ).toContain(
      "a @timestamp or @populatedBy relationship property cannot be made @settable",
    );
  });
});
