// A vector search ranks by every candidate's stored vector, so the vector
// field's @authentication and READ validate rules apply to each node it
// ranks, with `vector` as with `to`.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

const problems = (sdl: string): string[] => {
  try {
    buildModel(sdl);
    return [];
  } catch (err) {
    if (err instanceof ModelError) return err.problems.map((p) => p.message);
    throw err;
  }
};

const vectorTypeDefs = (guard: string) => `type Doc @node @mutation {
  key: String! @key
  visible: Boolean
  embedding: [Float!] @vector(dimensions: 2, similarity: COSINE) ${guard}
}`;
const docs = (...rows: string[]) =>
  `mutation { createDocs(input: [${rows.join(" ")}]) { info { nodesCreated } } }`;
const open = (key: string) =>
  `{ key: "${key}", visible: true, embedding: [1, 0] }`;
const seeded = async (guard: string, ...extra: string[]) => {
  const t = await createTestLoraGraphQL({ typeDefs: vectorTypeDefs(guard) });
  await t.data(
    docs(open("open"), `{ key: "shut", embedding: [1, 0] }`, ...extra),
    {},
    signedIn,
  );
  return t;
};
type Similar = { similarDocs: Array<{ node: { key: string } }> };
const signedIn = { jwt: { sub: "u" } };
const readRule =
  "@authorization(validate: [{ operations: [READ], where: { node: { visible: { eq: true } } } }])";

describe("vector search over a guarded vector field", () => {
  test("`vector:` ranks only nodes passing the field's READ rules", async () => {
    const t = await seeded(readRule);
    const d = await t.data<Similar>(
      "{ similarDocs(vector: [1, 0]) { node { key } } }",
      {},
      signedIn,
    );
    expect(d.similarDocs.map((r) => r.node.key)).toEqual(["open"]);
  });

  test("`to:` ranks only nodes passing the field's READ rules", async () => {
    const t = await seeded(readRule, open("open2"));
    const d = await t.data<Similar>(
      '{ similarDocs(to: "open") { node { key } } }',
      {},
      signedIn,
    );
    expect(d.similarDocs.map((r) => r.node.key)).toEqual(["open2"]);
  });

  test("`vector:` needs the field's @authentication", async () => {
    const t = await seeded("@authentication(operations: [READ])");
    const res = await t.run("{ similarDocs(vector: [1, 0]) { node { key } } }");
    expect(res.errors?.[0]?.extensions?.["code"]).toBe("UNAUTHENTICATED");
  });

  test("a mask on a @vector field is a model error", () => {
    expect(
      problems(
        vectorTypeDefs(
          "@authorization(mask: [{ unless: { node: { visible: { eq: true } } } }])",
        ),
      ).join("\n"),
    ).toMatch(/@vector/);
  });
});
