// The runtime list of error codes and the guard that recognizes them.

import { readFileSync, readdirSync } from "node:fs";
import { GraphQLError } from "graphql";
import { describe, expect, test } from "vitest";
import {
  isLoraGraphQLError,
  LORA_GRAPHQL_ERROR_CODES,
  type LoraGraphQLErrorCode,
} from "../src/index.js";
import { createTestLoraGraphQL } from "../src/testing.js";

describe("LORA_GRAPHQL_ERROR_CODES", () => {
  test("is frozen and lists each code once", () => {
    expect(Object.isFrozen(LORA_GRAPHQL_ERROR_CODES)).toBe(true);
    expect(new Set(LORA_GRAPHQL_ERROR_CODES).size).toBe(
      LORA_GRAPHQL_ERROR_CODES.length,
    );
    const code: LoraGraphQLErrorCode = LORA_GRAPHQL_ERROR_CODES[0];
    expect(code).toBe("BAD_USER_INPUT");
  });

  test("covers every code the source raises", () => {
    const dir = new URL("../src/", import.meta.url);
    const files = readdirSync(dir, { recursive: true, encoding: "utf8" });
    const raised = new Set<string>();
    for (const f of files) {
      if (!f.endsWith(".ts")) continue;
      const src = readFileSync(new URL(f, dir), "utf8");
      for (const m of src.matchAll(/requestError\(\s*"([A-Z_]+)"/g))
        raised.add(m[1]!);
    }
    expect(raised.size).toBeGreaterThan(3);
    for (const c of raised) expect(LORA_GRAPHQL_ERROR_CODES).toContain(c);
  });
});

describe("isLoraGraphQLError", () => {
  test("recognizes library errors, live or serialized", async () => {
    const t = await createTestLoraGraphQL({
      typeDefs: "type Thing @node { key: String! @key }",
    });
    const r = await t.run(`{ things(limit: -1) { key } }`);
    expect(r.errors).toHaveLength(1);
    expect(isLoraGraphQLError(r.errors![0])).toBe(true);
    expect(isLoraGraphQLError(JSON.parse(JSON.stringify(r.errors![0])))).toBe(
      true,
    );
  });

  test("rejects anything else", () => {
    expect(isLoraGraphQLError(new Error("x"))).toBe(false);
    expect(isLoraGraphQLError(new GraphQLError("x"))).toBe(false);
    expect(
      isLoraGraphQLError(
        new GraphQLError("x", { extensions: { code: "INTERNAL" } }),
      ),
    ).toBe(false);
    expect(isLoraGraphQLError(null)).toBe(false);
    expect(isLoraGraphQLError("FORBIDDEN")).toBe(false);
    expect(
      isLoraGraphQLError({ message: "x", extensions: { code: "FORBIDDEN" } }),
    ).toBe(true);
  });
});
