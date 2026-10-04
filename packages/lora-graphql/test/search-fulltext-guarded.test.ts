// A full-text index matches stored values whatever the read rules say, so
// one over a guarded field is a model error: searching it would be an
// oracle for the hidden values.

import { describe, expect, test } from "vitest";
import { buildModel, ModelError } from "../src/index.js";

const problems = (sdl: string): string[] => {
  try {
    buildModel(sdl);
    return [];
  } catch (err) {
    if (err instanceof ModelError) return err.problems.map((p) => p.message);
    throw err;
  }
};

const patient = (guard: string) => `type Patient @node
  @fulltext(indexes: [{ fields: ["name", "diagnosis"] }]) {
  key: String! @key
  visible: Boolean
  name: String!
  diagnosis: String ${guard}
}`;

describe("full-text indexes over guarded fields", () => {
  test("a masked, READ-validated or READ-authenticated field is refused", () => {
    for (const guard of [
      "@authorization(mask: [{ unless: { node: { visible: { eq: true } } } }])",
      "@authorization(validate: [{ operations: [READ], where: { node: { visible: { eq: true } } } }])",
      "@authentication(operations: [READ])",
    ]) {
      const found = problems(patient(guard));
      expect(found.join("\n")).toMatch(
        /@fulltext: diagnosis has field-level read rules/,
      );
    }
  });

  test("write-only rules do not refuse the index", () => {
    expect(
      problems(
        patient(
          "@authorization(validate: [{ operations: [UPDATE], where: { node: { visible: { eq: true } } } }])",
        ),
      ),
    ).toEqual([]);
    expect(problems(patient("@authentication(operations: [UPDATE])"))).toEqual(
      [],
    );
    expect(problems(patient(""))).toEqual([]);
  });
});
