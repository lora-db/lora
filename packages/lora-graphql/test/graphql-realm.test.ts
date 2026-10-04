// Two copies of graphql break `instanceof GraphQLError`, so a server masks
// the library's errors. The Envelop plugin notices and says so, once.

import { buildSchema, parse, validate } from "graphql";
import * as other from "graphql17";
import { describe, expect, test, vi } from "vitest";
import { envelopPlugin } from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

// Under LORA_GRAPHQL_VERSION=17 "graphql" is aliased to graphql17, so
// there is only one copy to compare.
const twoCopies = process.env["LORA_GRAPHQL_VERSION"] !== "17";

const rulesOf = (plugin: ReturnType<typeof envelopPlugin>) => {
  const rules: Parameters<typeof validate>[2] & unknown[] = [];
  plugin.onValidate({ addValidationRule: (r) => rules.push(r) });
  return rules;
};

describe("graphql realm check", () => {
  test("is silent when the server shares the library's graphql", async () => {
    const report = vi.fn();
    const plugin = envelopPlugin({}, report);
    const t = await createTestLoraGraphQL({
      typeDefs: "type Thing @node { key: String! @key }",
    });
    plugin.onSchemaChange({ schema: t.lora.getSchema() });
    const schema = buildSchema("type Query { a: Int }");
    expect(validate(schema, parse("{ a }"), rulesOf(plugin))).toEqual([]);
    expect(report).not.toHaveBeenCalled();
  });

  test.skipIf(!twoCopies)(
    "reports a schema from another copy once, naming the fix",
    () => {
      const report = vi.fn();
      const plugin = envelopPlugin({}, report);
      const foreign = other.buildSchema("type Query { a: Int }");
      plugin.onSchemaChange({ schema: foreign });
      plugin.onSchemaChange({ schema: foreign });
      expect(report).toHaveBeenCalledTimes(1);
      expect(report.mock.calls[0]![0]).toMatch(
        /different copy of the "graphql"/,
      );
      expect(report.mock.calls[0]![0]).toMatch(/dedupe/);
    },
  );

  test.skipIf(!twoCopies)(
    "reports validation by another copy, then stops adding the probe",
    () => {
      const report = vi.fn();
      const plugin = envelopPlugin({}, report);
      const schema = other.buildSchema("type Query { a: Int }");
      const rules = rulesOf(plugin) as unknown as other.ValidationRule[];
      expect(other.validate(schema, other.parse("{ a }"), rules)).toEqual([]);
      expect(report).toHaveBeenCalledTimes(1);
      expect(report.mock.calls[0]![0]).toMatch(/ValidationContext/);
      const after = rulesOf(plugin) as unknown as other.ValidationRule[];
      expect(after.length).toBe(rules.length - 1);
    },
  );

  test("ignores values that are not graphql objects", () => {
    const report = vi.fn();
    const plugin = envelopPlugin({}, report);
    plugin.onSchemaChange({ schema: {} });
    plugin.onSchemaChange({ schema: null });
    expect(report).not.toHaveBeenCalled();
  });

  test("defaults to console.error", () => {
    if (!twoCopies) return;
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    envelopPlugin().onSchemaChange({
      schema: other.buildSchema("type Query { a: Int }"),
    });
    expect(spy).toHaveBeenCalledTimes(1);
    spy.mockRestore();
  });
});
