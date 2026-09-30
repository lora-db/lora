// Regressions from a production integration (items G-1 to G-19 of the
// Festimap brief). Each block names its item; the suite runs on graphql
// 16 and 17 (`yarn test`, `yarn test:graphql17`).

import { describe, expect, test } from "vitest";
import { createTestLoraGraphQL, expectSeeks } from "../src/testing.js";

describe("G-1: compile, explain, check and expectSeeks take variables", () => {
  const typeDefs = `type F @node { key: String! @key  name: String @filterable }`;
  const seed =
    "UNWIND range(1, 30) AS i CREATE (:F {key: toString(i), name: 'n' + toString(i)})";

  test("on a root argument", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const [root] = t.lora.compile(
      "query Q($k: String!) { f(key: $k) { key } }",
      { k: "7" },
    );
    expect(Object.values(root!.compiled.statements[0]!.params)).toContain("7");
    await expectSeeks(t.lora, "query Q($k: String!) { f(key: $k) { key } }", {
      k: "7",
    });
    t.close();
  });

  test("inside an input object", async () => {
    const t = await createTestLoraGraphQL({ typeDefs, seed });
    const [root] = t.lora.compile(
      "query Q($n: String) { fs(where: { name: { eq: $n } }) { key } }",
      { n: "n3" },
    );
    expect(root!.compiled.statements[0]!.text).toMatch(/\.name = \$p\d/);
    expect(Object.values(root!.compiled.statements[0]!.params)).toContain("n3");
    const report = await t.lora.check({
      operations: [
        {
          document:
            "query Q($n: String) { fs(where: { name: { eq: $n } }) { key } }",
          variables: { n: "n3" },
        },
      ],
    });
    expect(report.errors).toEqual([]);
    // The filter reached the statement, so the plan seeks the name index.
    expect(report.plans[0]!.reports[0]!.statement.text).toMatch(
      /\.name = \$p\d/,
    );
    expect(
      report.plans.flatMap((p) => p.reports.flatMap((r) => r.findings)),
    ).toEqual([]);
    t.close();
  });
});
