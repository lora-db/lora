// `@authorization(mask:)`: a row failing `unless` reads a field as a
// substitute value instead of failing the request, and filters compare
// the value the reader sees, so a mask never leaks through a filter.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const typeDefs = `type Claims @jwt {
  sub: String! @viewer(type: "Person", field: "key")
  roles: [String!]
}
enum ConnectionRequestStatus { PENDING ACCEPTED DECLINED }
type Person @node {
  key: String! @key
  lastSeenAt: String @authorization(mask: [{ unless: { node: { isViewer: true } } }])
}
type ConnectionRequest @node
  @authorization(filter: [{ where: { OR: [{ node: { from: { isViewer: true } } }, { node: { to: { isViewer: true } } }] } }]) {
  key: String! @key
  status: ConnectionRequestStatus! @filterable @sortable
    @authorization(mask: [{ unless: { OR: [
      { node: { status: { in: [PENDING, ACCEPTED] } } }
      { node: { to: { isViewer: true } } } ] }, value: PENDING }])
  from: Person! @relationship(type: "SENT", direction: IN)
  to: Person! @relationship(type: "TO", direction: OUT)
}`;
const seed = [
  "CREATE (lou:Person {key: 'lou', lastSeenAt: 'L'}), (bo:Person {key: 'bo', lastSeenAt: 'B'}), (lou)-[:SENT]->(:ConnectionRequest {key: 'r1', status: 'DECLINED'})-[:TO]->(bo), (lou)-[:SENT]->(:ConnectionRequest {key: 'r2', status: 'ACCEPTED'})-[:TO]->(bo)",
];
const as = (sub: string) => ({ jwt: { sub, roles: [] } });
type Requests = { connectionRequests: Array<{ key: string; status: string }> };

describe("masks", () => {
  test("the sender reads a declined request as PENDING; the recipient as DECLINED", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const q = "{ connectionRequests(sort: [{ key: ASC }]) { key status } }";
    expect(
      (await t.data<Requests>(q, {}, as("lou"))).connectionRequests,
    ).toEqual([
      { key: "r1", status: "PENDING" },
      { key: "r2", status: "ACCEPTED" },
    ]);
    expect(
      (await t.data<Requests>(q, {}, as("bo"))).connectionRequests,
    ).toEqual([
      { key: "r1", status: "DECLINED" },
      { key: "r2", status: "ACCEPTED" },
    ]);
  });

  test("filters compare the masked value", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const by = async (status: string, who: string) =>
      (
        await t.data<Requests>(
          `{ connectionRequests(where: { status: { eq: ${status} } }) { key } }`,
          {},
          as(who),
        )
      ).connectionRequests.map((r) => r.key);
    expect(await by("DECLINED", "lou")).toEqual([]);
    expect(await by("PENDING", "lou")).toEqual(["r1"]);
    expect(await by("DECLINED", "bo")).toEqual(["r1"]);
  });

  test("a nullable field masks to null for everyone else", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const seen = async (who: string) =>
      t.data<{ persons: Array<{ key: string; lastSeenAt: string | null }> }>(
        "{ persons(sort: [{ key: ASC }]) { key lastSeenAt } }",
        {},
        as(who),
      );
    expect((await seen("lou")).persons).toEqual([
      { key: "bo", lastSeenAt: null },
      { key: "lou", lastSeenAt: "L" },
    ]);
  });

  test("sorting by a field masked per row is refused", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const r = await t.run(
      "{ connectionRequests(sort: [{ status: ASC }]) { key } }",
      {},
      as("lou"),
    );
    expect(r.errors?.[0]?.message).toBe(
      "cannot sort by ConnectionRequest.status: it is masked per row",
    );
  });
});

describe("model checks", () => {
  const problems = (sdl: string) => {
    try {
      buildModel(sdl);
      return [];
    } catch (err) {
      if (err instanceof ModelError) return err.problems.map((p) => p.message);
      throw err;
    }
  };

  test("the value fits the field", () => {
    expect(problems(typeDefs.replace(", value: PENDING }", " }"))).toContain(
      "@authorization(mask:) on a non-null field needs a value",
    );
    expect(
      problems(typeDefs.replace("value: PENDING", "value: MAYBE")),
    ).toContain(
      '@authorization(mask:) value "MAYBE" does not fit ConnectionRequestStatus',
    );
  });

  test("masks belong on scalar fields", () => {
    expect(
      problems(
        typeDefs.replace(
          'to: Person! @relationship(type: "TO", direction: OUT)',
          'to: Person! @relationship(type: "TO", direction: OUT) @authorization(mask: [{ unless: { jwt: { sub: { eq: "x" } } } }])',
        ),
      ),
    ).toContain(
      "@authorization(mask:) belongs on a scalar field that is not the @key",
    );
  });
});
