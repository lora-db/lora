// Rule sugar must compile to exactly what its hand-written form compiles
// to: the same statement text and the same parameter values. Parameters
// are compared by first use in the text, and one the text never reads is
// dropped: a hand-written claim branch the claims fold away can leave an
// unused parameter behind, which changes numbering, not the statement.

import { expect } from "vitest";
import { LoraGraphQL, loraDriver } from "../src/index.js";
import { createDatabase } from "@loradb/lora-node";

interface Statement {
  text: string;
  params: Record<string, unknown>;
}

function normalize(s: Statement): Statement {
  const order: string[] = [];
  for (const [, name] of s.text.matchAll(/\$(p\d+)\b/g)) {
    if (!order.includes(name!)) order.push(name!);
  }
  const text = s.text.replace(
    /\$(p\d+)\b/g,
    (_, name: string) => `$q${order.indexOf(name)}`,
  );
  const params = Object.fromEntries(
    order.map((name, i) => [`q${i}`, s.params[name]]),
  );
  return JSON.parse(JSON.stringify({ text, params })) as Statement;
}

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
      left.map((f) => f.compiled.statements.map(normalize)),
      `${query} as ${JSON.stringify(context ?? null)}`,
    ).toEqual(right.map((f) => f.compiled.statements.map(normalize)));
  }
}
