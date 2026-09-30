// Rule sugar must compile to exactly what its hand-written form compiles
// to: the same statement text and the same parameters.

import { expect } from "vitest";
import { LoraGraphQL, loraDriver } from "../src/index.js";
import { createDatabase } from "@loradb/lora-node";

export async function expectSameStatements(
  sugared: string,
  plain: string,
  queries: Array<{ query: string; context?: Record<string, unknown> }>,
): Promise<void> {
  const db = await createDatabase();
  const a = new LoraGraphQL({ typeDefs: sugared, driver: loraDriver(db) });
  const b = new LoraGraphQL({ typeDefs: plain, driver: loraDriver(db) });
  for (const { query, context } of queries) {
    const left = a.compile(query, {}, { context });
    const right = b.compile(query, {}, { context });
    expect(
      left.map((f) => f.compiled.statements),
      `${query} as ${JSON.stringify(context ?? null)}`,
    ).toEqual(right.map((f) => f.compiled.statements));
  }
}
